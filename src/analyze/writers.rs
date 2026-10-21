//! A source evaluation replaces its own outputs; skipped sources retain theirs.
//! A source is a body root plus its visiting class/includer, receiver side and view.
//! All paths through that evaluation contribute, including repeated visits and arms.
use super::{
    SendSite,
    body::{ClassInfo, Ctx},
};
use crate::{
    expr::Expr,
    ident::{ClassId, Symbol},
    span::Span,
    ty::Ty,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::LazyLock,
};

static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_WRITERS").as_deref() == Ok("1"));
static TRACE: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_WRITERS_TRACE").as_deref() == Ok("1"));
pub(super) fn on() -> bool {
    *ON
}
pub(super) fn tracing() -> bool {
    *TRACE
}
#[derive(Default)]
struct State {
    stack: Vec<String>,
    source: String,
    declared_owners: BTreeMap<String, (ClassId, Symbol)>,
    root_owners: BTreeMap<(u32, u32, u32), (ClassId, Symbol)>,
    call_owners: BTreeMap<String, (ClassId, Symbol)>,
    current_owner: Option<(ClassId, Symbol)>,
    roots: BTreeSet<(u32, u32, u32)>,
    collections: BTreeMap<Symbol, BTreeSet<ClassId>>,
    capture_calls: bool,
    pending_calls: Vec<SendSite>,
    calls: BTreeMap<String, Vec<SendSite>>,
    changed: BTreeSet<String>,
    // Destination knowledge is conservative and monotone; counts describe live callers.
    // A withdrawn known caller removes its value, not an unknown caller's possible destination.
    targets: BTreeMap<(ClassId, Symbol, super::SiteRecv), usize>,
    unknown_methods: BTreeMap<Symbol, usize>,
    revision: u64,
}
thread_local! { static ST: RefCell<State> = RefCell::new(State::default()); }
pub(super) fn completion_values(f: &mut dyn FnMut(&mut Ty)) {
    if !on() { return; }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        fn site(site: &mut SendSite, f: &mut dyn FnMut(&mut Ty)) {
            site.2.iter_mut().for_each(&mut *f);
            for (_, t) in &mut site.3.keys { f(t); }
            if let Some(t) = &mut site.3.splat { f(t); }
        }
        for calls in s.calls.values_mut() { for call in calls { site(call, f); } }
        for call in &mut s.pending_calls { site(call, f); }
    });
}
pub(super) fn reset(app: &crate::App) {
    let mut state = State::default();
    let includes: HashMap<_, _> = app
        .library_classes
        .iter()
        .map(|c| (c.name.clone(), c.includes.clone()))
        .collect();
    let mut collection = |owner: &ClassId, assoc: &crate::dialect::Association| {
        if matches!(
            assoc,
            crate::dialect::Association::HasMany { .. }
                | crate::dialect::Association::HasAndBelongsToMany { .. }
        ) {
            state
                .collections
                .entry(assoc.name().clone())
                .or_default()
                .insert(owner.clone());
        }
    };
    for model in &app.models {
        for assoc in model.associations() {
            collection(&model.name, assoc);
        }
        let mut queue = super::model_includes(model);
        let mut seen = BTreeSet::new();
        while let Some(module) = queue.pop() {
            if !seen.insert(module.clone()) {
                continue;
            }
            if let Some(nested) = includes.get(&module) {
                queue.extend(nested.iter().cloned());
            }
            if let Some(items) = app.concern_model_items.get(&module) {
                for item in items {
                    if let crate::dialect::ModelBodyItem::Association { assoc, .. } = item {
                        collection(&model.name, assoc);
                    }
                }
            }
        }
    }
    let mut add = |span: Span| {
        if !span.is_synthetic() {
            state.roots.insert(super::fold::site_of(&span));
        }
    };
    for model in &app.models {
        for method in model.methods() {
            add(method.body.span);
        }
        for scope in model.scopes() {
            add(scope.body.span);
            state.root_owners.insert(
                super::fold::site_of(&scope.body.span),
                (model.name.clone(), scope.name.clone()),
            );
        }
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item {
                if let Some(body) = super::block_callback_body(expr) {
                    add(body.span);
                    add(expr.span);
                }
            }
        }
    }
    for class in &app.library_classes {
        for method in &class.methods {
            add(method.body.span);
        }
    }
    for controller in &app.controllers {
        for action in controller.actions() {
            add(action.body.span);
        }
        for method in controller.class_methods() {
            add(method.body.span);
        }
    }
    for view in &app.views {
        add(view.body.span);
    }
    if let Some(seeds) = &app.seeds {
        add(seeds.span);
    }
    for module in &app.test_modules {
        if let Some(setup) = &module.setup {
            add(setup.span);
        }
        for method in &module.helpers {
            add(method.body.span);
            for param in &method.params {
                if let Some(default) = &param.default {
                    add(default.span);
                }
            }
        }
        for test in &module.tests {
            add(test.body.span);
        }
    }
    ST.with(|s| *s.borrow_mut() = state);
}
pub(super) struct Visit(bool);
pub(super) fn enter(span: Span, ctx: &Ctx) -> Visit {
    if !on() && !tracing() {
        return Visit(false);
    }
    // Only nominal scope identity belongs in the key, never evolving input types.
    let need_identity = tracing() || ST.with(|s| s.borrow().stack.is_empty());
    let scope = if need_identity {
        ctx.lexical_self
            .as_ref()
            .or(ctx.self_ty.as_ref())
            .map(super::class_ids_for_call_receiver)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let id = if need_identity {
        format!(
            "{}/{scope:?}/{}/{}/{:?}/phase:{}",
            ctx.writer_source
                .as_deref()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{span:?}")),
            ctx.class_side,
            ctx.in_view,
            ctx.view_name,
            ctx.writer_phase
        )
    } else {
        String::new()
    };
    let source = ST.with(|s| {
        let mut s = s.borrow_mut();
        let first = s.stack.is_empty();
        if first {
            s.source = id.clone();
            s.current_owner = ctx
                .writer_source
                .as_deref()
                .and_then(|name| s.declared_owners.get(name))
                .or_else(|| s.root_owners.get(&super::fold::site_of(&span)))
                .cloned();
            s.pending_calls.clear();
            s.capture_calls =
                ctx.writer_source.is_some() || s.roots.contains(&super::fold::site_of(&span));
        }
        s.stack.push(id);
        first.then(|| s.source.clone())
    });
    if on() {
        if let Some(source) = source {
            super::fold::writer_begin(&source);
        }
    }
    Visit(true)
}
impl Drop for Visit {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let source = ST.with(|s| {
            let mut s = s.borrow_mut();
            s.stack.pop();
            if !s.stack.is_empty() {
                return None;
            }
            let source = s.source.clone();
            if on() && s.capture_calls {
                let calls = std::mem::take(&mut s.pending_calls);
                let known = |calls: &[SendSite]| -> BTreeSet<_> {
                    calls
                        .iter()
                        .filter(|c| !unknown_class(&c.0))
                        .map(|c| (c.0.clone(), c.1.clone(), c.4))
                        .collect()
                };
                let unknown = |calls: &[SendSite]| -> BTreeSet<_> {
                    calls
                        .iter()
                        .filter(|c| unknown_class(&c.0) && !c.2.is_empty())
                        .map(|c| c.1.clone())
                        .collect()
                };
                let old_unknown = s.calls.get(&source).map(|c| unknown(c)).unwrap_or_default();
                let new_unknown = unknown(&calls);
                for method in old_unknown.difference(&new_unknown) {
                    let n = s.unknown_methods.get_mut(method).unwrap();
                    *n -= 1;
                    if *n == 0 {
                        s.unknown_methods.remove(method);
                    }
                }
                for method in new_unknown.difference(&old_unknown) {
                    let n = s.unknown_methods.entry(method.clone()).or_default();
                    if *n == 0 {
                        s.revision += 1;
                    }
                    *s.unknown_methods.get_mut(method).unwrap() += 1;
                }
                let old = s.calls.get(&source).map(|c| known(c)).unwrap_or_default();
                let new = known(&calls);
                for key in old.difference(&new) {
                    let n = s.targets.get_mut(key).unwrap();
                    *n -= 1;
                }
                for key in new.difference(&old) {
                    let fresh = !s.targets.contains_key(key);
                    *s.targets.entry(key.clone()).or_default() += 1;
                    if fresh && s.unknown_methods.contains_key(&key.1) {
                        s.revision += 1;
                    }
                }
                s.calls.insert(source.clone(), calls);
                if let Some(owner) = s.current_owner.clone() {
                    s.call_owners.insert(source.clone(), owner);
                } else {
                    s.call_owners.remove(&source);
                }
                s.changed.insert(source.clone());
            }
            Some(source)
        });
        if on() {
            if let Some(source) = source {
                super::fold::writer_end(&source);
            }
        }
    }
}
pub(super) fn context() -> String {
    ST.with(|s| s.borrow().stack.join(" > "))
}
pub(super) fn source() -> Option<String> {
    ST.with(|s| {
        let s = s.borrow();
        (!s.stack.is_empty()).then(|| s.source.clone())
    })
}
pub(super) fn capture(expr: &Expr, ctx: &Ctx, classes: &HashMap<ClassId, ClassInfo>) {
    if !on() && !tracing() {
        return;
    }
    if let crate::expr::ExprNode::Send {
        method, recv, args, ..
    } = &*expr.node
    {
        if tracing() && matches!(method.as_str(), "preview" | "includes") {
            eprintln!(
                "WRITER typed span={:?} method={method:?} recv={:?} args={:?} context={}",
                expr.span,
                recv.as_ref().and_then(|r| r.ty.as_ref()),
                args.iter()
                    .map(|a| super::parameter_conservation::argument(a))
                    .collect::<Vec<_>>(),
                context()
            );
        }
    } else {
        return;
    }
    if matches!(&*expr.node,crate::expr::ExprNode::Send {args,..} if args.is_empty()) {
        return; // no parameter slot is written; the ordinary walk owns empty rows
    }
    if !on() || !ST.with(|s| s.borrow().capture_calls) {
        return;
    }
    let class = if ctx.in_view {
        None
    } else {
        ctx.self_ty
            .as_ref()
            .and_then(|t| super::class_ids_for_call_receiver(t).into_iter().next())
    };
    let mut calls = Vec::new();
    super::collect_direct_send_sites(
        expr,
        class.as_ref(),
        ctx.class_side,
        &HashMap::new(),
        classes,
        &Default::default(),
        &mut calls,
    );
    ST.with(|s| s.borrow_mut().pending_calls.extend(calls));
}
pub(super) fn declare_owner(source: &str, class: &ClassId, method: &Symbol) {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if !s.declared_owners.contains_key(source) {
            s.declared_owners
                .insert(source.into(), (class.clone(), method.clone()));
        }
    });
}
pub(super) fn owned_calls() -> Vec<(Option<(ClassId, Symbol)>, Vec<SendSite>)> {
    ST.with(|s| {
        let s = s.borrow();
        s.calls
            .iter()
            .map(|(source, calls)| (s.call_owners.get(source).cloned(), calls.clone()))
            .collect()
    })
}
pub(super) fn calls() -> Vec<SendSite> {
    ST.with(|s| s.borrow().calls.values().flatten().cloned().collect())
}
pub(super) fn changed_calls(all: bool) -> Vec<(String, Vec<SendSite>)> {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let changed = std::mem::take(&mut s.changed);
        if all {
            s.calls
                .iter()
                .map(|(key, calls)| (key.clone(), calls.clone()))
                .collect()
        } else {
            changed
                .into_iter()
                .filter_map(|key| s.calls.get(&key).map(|calls| (key, calls.clone())))
                .collect()
        }
    })
}
/// Pending contributes no value. Every other current writer is conserved.
pub(super) fn join(a: Ty, b: Ty) -> Ty {
    if super::strict_producers::is_p(&a) {
        return super::parameter_conservation::normalize(b);
    }
    if super::strict_producers::is_p(&b) {
        return super::parameter_conservation::normalize(a);
    }
    super::parameter_conservation::join(a, b)
}

pub(super) fn unknown_receiver_class() -> ClassId {
    ClassId(Symbol::from("<unknown-receiver>"))
}
pub(super) fn unknown_class(class: &ClassId) -> bool {
    class.0.as_str() == "<unknown-receiver>"
}
pub(super) fn targets() -> Vec<(ClassId, Symbol, super::SiteRecv)> {
    ST.with(|s| s.borrow().targets.keys().cloned().collect())
}
pub(super) fn revision() -> u64 {
    if super::route_static::on() { return 0; }
    ST.with(|s| s.borrow().revision)
}
pub(super) fn unknown_receiver(t: &Ty, classes: &HashMap<ClassId, ClassInfo>) -> bool {
    fn go(t: &Ty, classes: &HashMap<ClassId, ClassInfo>, seen: &mut BTreeSet<u32>) -> bool {
        match t {
            Ty::Var { .. } => true,
            Ty::Untyped { .. } => !super::strict_producers::is_p(t),
            Ty::Union { variants } => variants.iter().any(|v| go(v, classes, seen)),
            Ty::Rec { slot } if seen.insert(*slot) => {
                super::fold::value_of(*slot, classes).is_some_and(|v| go(&v, classes, seen))
            }
            _ => false,
        }
    }
    go(t, classes, &mut BTreeSet::new())
}

/// Source Ruby contracts that the compiled runtime's signatures underapproximate.
/// These are separate producer repairs, not evidence supplied by the writer ledger.
pub(super) fn source_result(
    expr: &Expr,
    ctx: &Ctx,
    classes: &HashMap<ClassId, ClassInfo>,
    value: Ty,
) -> Ty {
    if !on() {
        return value;
    }
    let crate::expr::ExprNode::Send { recv, method, .. } = &*expr.node else {
        return value;
    };
    let collection_target = match &value {
        Ty::Array { elem } => match &**elem {
            Ty::Class { id, .. } => Some(id.clone()),
            _ => None,
        },
        _ => None,
    };
    let owners = collection_target
        .as_ref()
        .and_then(|_| ST.with(|s| s.borrow().collections.get(method).cloned()));
    if !matches!(method.as_str(), "save!" | "zone") && owners.is_none() {
        return value;
    }
    let receiver = recv
        .as_ref()
        .and_then(|r| r.ty.as_ref())
        .or(ctx.self_ty.as_ref());
    let ids = receiver
        .map(|t| {
            super::fold::receiver_classes(t, classes)
                .unwrap_or_else(|| super::class_ids_for_call_receiver(t))
        })
        .unwrap_or_default();
    if let (Some(target), Some(owners)) = (collection_target, owners) {
        let association = ids.iter().any(|id| {
            let mut current = Some(id.clone());
            for _ in 0..32 {
                let Some(id) = current else { break };
                if owners.contains(&id) {
                    return true;
                }
                current = classes.get(&id).and_then(|c| c.parent.clone());
            }
            false
        });
        if association {
            // The backend's Array spelling also denotes an AR CollectionProxy.
            return join(value, Ty::Relation { of: target });
        }
    }
    if method.as_str() == "save!"
        && ids
            .iter()
            .any(|id| classes.get(id).is_some_and(|c| c.table.is_some()))
    {
        // Rails returns true on success; the transpiled runtime currently returns Base.
        return join(value, Ty::Bool);
    }
    if method.as_str() == "zone" && ids.iter().any(|id| id.0.as_str() == "Time") {
        // Time.zone is a configured TimeZone (or nil), not a Time instance.
        let zone = Ty::Class {
            id: ClassId(Symbol::from("ActiveSupport::TimeZone")),
            args: vec![].into(),
        };
        return join(value, join(zone, Ty::Nil));
    }
    value
}

/// A balanced join cache for replaceable rows. Leaves are writer cells; only
/// the path to the root changes on replacement/removal. Bound only the root.
#[derive(Default)]
pub(super) struct RowCells {
    positions: BTreeMap<(String, usize), usize>,
    free: Vec<usize>,
    rows: Vec<Option<Vec<Ty>>>,
    capacity: usize,
    tree: Vec<Option<Vec<Ty>>>,
}
impl RowCells {
    pub(super) fn completion_values(&mut self, f: &mut dyn FnMut(&mut Ty)) {
        for t in self.rows.iter_mut().flatten().flatten() { f(t); }
        self.rebuild();
    }
    fn combine(a: Option<&Vec<Ty>>, b: Option<&Vec<Ty>>) -> Option<Vec<Ty>> {
        match (a, b) {
            (None, None) => None,
            (Some(a), None) | (None, Some(a)) => Some(a.clone()),
            (Some(a), Some(b)) => Some(
                (0..a.len().max(b.len()))
                    .map(|i| match (a.get(i), b.get(i)) {
                        (Some(a), Some(b)) => {
                            super::parameter_conservation::join(a.clone(), b.clone())
                        }
                        (Some(t), None) | (None, Some(t)) => t.clone(),
                        _ => unreachable!(),
                    })
                    .collect(),
            ),
        }
    }
    fn rebuild(&mut self) {
        self.capacity = self.rows.len().max(1).next_power_of_two();
        self.tree = vec![None; self.capacity * 2];
        for (i, row) in self.rows.iter().enumerate() {
            self.tree[self.capacity + i] = row.clone();
        }
        for i in (1..self.capacity).rev() {
            self.tree[i] = Self::combine(self.tree[i * 2].as_ref(), self.tree[i * 2 + 1].as_ref());
        }
    }
    fn update(&mut self, index: usize) {
        if index >= self.capacity || self.tree.is_empty() {
            self.rebuild();
            return;
        }
        let mut i = self.capacity + index;
        self.tree[i] = self.rows[index].clone();
        while i > 1 {
            i /= 2;
            self.tree[i] = Self::combine(self.tree[i * 2].as_ref(), self.tree[i * 2 + 1].as_ref());
        }
    }
    pub(super) fn insert(&mut self, key: (String, usize), row: Vec<Ty>) {
        let row: Vec<_> = row
            .into_iter()
            .map(super::parameter_conservation::initialize)
            .collect();
        let index = if let Some(&index) = self.positions.get(&key) {
            if self.rows[index]
                .as_ref()
                .is_some_and(|old| super::strict_producers::same_types(old, &row))
            {
                return;
            }
            index
        } else {
            let index = self.free.pop().unwrap_or_else(|| {
                self.rows.push(None);
                self.rows.len() - 1
            });
            self.positions.insert(key, index);
            index
        };
        self.rows[index] = Some(row);
        self.update(index);
    }
    pub(super) fn remove(&mut self, key: &(String, usize)) {
        if let Some(index) = self.positions.remove(key) {
            self.rows[index] = None;
            self.free.push(index);
            self.update(index);
        }
    }
    pub(super) fn value(&self) -> Option<Vec<Ty>> {
        self.tree.get(1).cloned().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        expr::{ExprNode, Literal},
        ident::Symbol,
        span::FileId,
    };
    fn ctx(name: &str) -> Ctx {
        Ctx {
            self_ty: Some(Ty::Class {
                id: ClassId(Symbol::from(name)),
                args: vec![].into(),
            }),
            ..Ctx::default()
        }
    }
    fn call(value: Ty) -> Expr {
        let span = Span {
            file: FileId(1),
            start: 10,
            end: 20,
        };
        let mut receiver = Expr::new(span, ExprNode::SelfRef);
        receiver.ty = Some(Ty::Class {
            id: ClassId(Symbol::from("Target")),
            args: vec![].into(),
        });
        let mut arg = Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Nil,
            },
        );
        arg.ty = Some(value);
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(receiver),
                method: Symbol::from("write"),
                args: vec![arg],
                block: None,
                parenthesized: false,
            },
        )
    }
    #[test]
    fn writers_replace_one_source_retain_skipped_context_and_retract_absent_calls() {
        if !on() {
            return;
        }
        ST.with(|s| {
            let mut s = s.borrow_mut();
            *s = State::default();
            s.roots.insert((1, 1, 30));
        });
        let span = Span {
            file: FileId(1),
            start: 1,
            end: 30,
        };
        for (context, value) in [(ctx("First"), Ty::Int), (ctx("Second"), Ty::Str)] {
            let visit = enter(span, &context);
            capture(&call(value), &context, &HashMap::new());
            drop(visit);
        }
        assert_eq!(calls().len(), 2);
        let first = ctx("First");
        let visit = enter(span, &first);
        capture(&call(Ty::Bool), &first, &HashMap::new());
        drop(visit);
        let values: Vec<_> = calls().into_iter().map(|c| c.2[0].clone()).collect();
        assert!(
            values.contains(&Ty::Bool) && values.contains(&Ty::Str) && !values.contains(&Ty::Int)
        );
        let visit = enter(span, &first);
        drop(visit);
        assert_eq!(calls().len(), 1);
        assert_eq!(calls()[0].2[0], Ty::Str);
        assert_eq!(targets().len(), 1);
        let visit = enter(span, &ctx("Second"));
        drop(visit);
        assert!(calls().is_empty());
        assert_eq!(targets().len(), 1); // Possible destinations are not writer values.
        ST.with(|s| *s.borrow_mut() = State::default());
    }
    #[test]
    fn writers_source_contracts_admit_rails_results_and_keep_backend_results() {
        if !on() {
            return;
        }
        let mut expr = call(Ty::Int);
        let ExprNode::Send { recv, method, .. } = &mut *expr.node else {
            unreachable!()
        };
        *method = Symbol::from("save!");
        let mut classes = HashMap::new();
        classes.insert(
            ClassId(Symbol::from("Target")),
            ClassInfo {
                table: Some(crate::ident::TableRef(Symbol::from("targets"))),
                ..ClassInfo::default()
            },
        );
        let runtime = Ty::Class {
            id: ClassId(Symbol::from("ActiveRecord::Base")),
            args: vec![].into(),
        };
        assert_eq!(
            source_result(&expr, &Ctx::default(), &classes, runtime.clone()),
            join(runtime, Ty::Bool)
        );
        let ExprNode::Send { recv, method, .. } = &mut *expr.node else {
            unreachable!()
        };
        *method = Symbol::from("zone");
        recv.as_mut().unwrap().ty = Some(Ty::Class {
            id: ClassId(Symbol::from("Time")),
            args: vec![].into(),
        });
        let zone = Ty::Class {
            id: ClassId(Symbol::from("ActiveSupport::TimeZone")),
            args: vec![].into(),
        };
        assert_eq!(
            source_result(&expr, &Ctx::default(), &classes, Ty::Time),
            join(Ty::Time, join(zone, Ty::Nil))
        );
    }
    #[test]
    fn writers_assignment_keeps_unknown_rhs_instead_of_the_column_type() {
        if !on() {
            return;
        }
        let mut classes = HashMap::new();
        let mut info = ClassInfo::default();
        info.instance_methods.insert(Symbol::from("name="), Ty::Str);
        classes.insert(ClassId(Symbol::from("Target")), info);
        let context = Ctx {
            ivar_bindings: std::iter::once((Symbol::from("@input"), Ty::unresolved())).collect(),
            ..ctx("Target")
        };
        let mut expr = call(Ty::Int);
        let ExprNode::Send { args, method, .. } = &mut *expr.node else {
            unreachable!()
        };
        *method = Symbol::from("name=");
        args[0] = Expr::new(
            args[0].span,
            ExprNode::Ivar {
                name: Symbol::from("@input"),
            },
        );
        let result = super::super::body::BodyTyper::new(&classes).analyze_expr(&mut expr, &context);
        assert!(super::super::strict_producers::same(
            &result,
            &Ty::unresolved()
        ));
    }
    #[test]
    fn writers_cached_rows_match_flat_join_after_replacement_removal_and_resize() {
        if !super::super::parameter_conservation::on() {
            return;
        }
        let key = (
            ClassId(Symbol::from("Target")),
            Symbol::from("write"),
            crate::dialect::MethodReceiver::Instance,
        );
        let mut cache = RowCells::default();
        let mut flat = BTreeMap::new();
        let mut random = 17u64;
        for step in 0..160 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let writer = (
                format!("context{}", random % 19),
                (random as usize / 23) % 3,
            );
            if step % 7 == 0 {
                flat.remove(&writer);
                cache.remove(&writer);
            } else {
                let value = match random % 6 {
                    0 => Ty::Bottom,
                    1 => Ty::Int,
                    2 => Ty::Str,
                    3 => Ty::unresolved(),
                    4 => Ty::Array {
                        elem: std::sync::Arc::new(Ty::Int),
                    },
                    _ => Ty::pending_untyped(),
                };
                let mut row = vec![value];
                if step % 3 == 0 {
                    row.push(Ty::Sym);
                }
                flat.insert(writer.clone(), row.clone());
                cache.insert(writer, row);
            }
            let mut expected = HashMap::new();
            super::super::fold_param_observations(
                &mut expected,
                flat.values()
                    .cloned()
                    .map(|row| (key.clone(), row))
                    .collect(),
            );
            let mut observed = HashMap::new();
            if let Some(row) = cache.value() {
                super::super::fold_param_observations(&mut observed, vec![(key.clone(), row)]);
            }
            assert!(
                super::super::strict_producers::same_row(expected.get(&key), observed.get(&key)),
                "mutation {step}"
            );
        }
    }
    #[test]
    fn writers_association_contract_admits_a_collection_proxy_without_erasing_arrays() {
        if !on() {
            return;
        }
        let owner = ClassId(Symbol::from("Target"));
        ST.with(|s| {
            s.borrow_mut().collections.insert(
                Symbol::from("members"),
                std::iter::once(owner.clone()).collect(),
            );
        });
        let mut expr = call(Ty::Int);
        let ExprNode::Send { method, .. } = &mut *expr.node else {
            unreachable!()
        };
        *method = Symbol::from("members");
        let array = Ty::Array {
            elem: std::sync::Arc::new(Ty::Class {
                id: owner.clone(),
                args: vec![].into(),
            }),
        };
        let expected = join(array.clone(), Ty::Relation { of: owner });
        assert_eq!(
            source_result(&expr, &Ctx::default(), &HashMap::new(), array),
            expected
        );
        ST.with(|s| {
            s.borrow_mut().collections.clear();
        });
    }
    #[test]
    fn writers_generated_action_wrappers_keep_distinct_definition_contexts() {
        if !on() {
            return;
        }
        ST.with(|s| *s.borrow_mut() = State::default());
        let mut first = ctx("Controller");
        first.writer_source = Some("action:Controller:show:context:0".into());
        let mut second = ctx("Controller");
        second.writer_source = Some("action:Controller:show:context:1".into());
        for (context, value) in [(&first, Ty::Int), (&second, Ty::Str)] {
            let visit = enter(Span::synthetic(), context);
            capture(&call(value), context, &HashMap::new());
            drop(visit);
        }
        assert_eq!(calls().len(), 2);
        let visit = enter(Span::synthetic(), &first);
        capture(&call(Ty::Bool), &first, &HashMap::new());
        drop(visit);
        let values: Vec<_> = calls().into_iter().map(|c| c.2[0].clone()).collect();
        assert!(
            values.contains(&Ty::Bool) && values.contains(&Ty::Str) && !values.contains(&Ty::Int)
        );
        ST.with(|s| *s.borrow_mut() = State::default());
    }
    #[test]
    fn writers_saved_pass_a_and_reseeded_pass_b_are_distinct_current_producers() {
        if !on() {
            return;
        }
        ST.with(|s| *s.borrow_mut() = State::default());
        let mut first = ctx("Library");
        first.writer_source = Some("method:Library:call".into());
        declare_owner(
            "method:Library:call",
            &ClassId(Symbol::from("Library")),
            &Symbol::from("call"),
        );
        let mut second = first.clone();
        second.writer_phase = 1;
        for (context, value) in [(&first, Ty::Int), (&second, Ty::Str)] {
            let visit = enter(Span::synthetic(), context);
            capture(&call(value), context, &HashMap::new());
            drop(visit);
        }
        assert_eq!(calls().len(), 2);
        assert!(owned_calls().iter().all(|(owner, _)| owner.as_ref()
            == Some(&(ClassId(Symbol::from("Library")), Symbol::from("call")))));
        let visit = enter(Span::synthetic(), &second);
        capture(&call(Ty::Bool), &second, &HashMap::new());
        drop(visit);
        let values: Vec<_> = calls().into_iter().map(|c| c.2[0].clone()).collect();
        assert!(
            values.contains(&Ty::Int) && values.contains(&Ty::Bool) && !values.contains(&Ty::Str)
        );
        ST.with(|s| *s.borrow_mut() = State::default());
    }
    #[test]
    fn writers_unknown_receiver_is_live_but_pending_is_not_a_runtime_route() {
        assert!(unknown_receiver(
            &Ty::Var {
                var: crate::ident::TyVar(0)
            },
            &HashMap::new()
        ));
        assert!(unknown_receiver(&Ty::unresolved(), &HashMap::new()));
        assert!(unknown_receiver(&Ty::gradual(), &HashMap::new()));
        assert!(!unknown_receiver(&Ty::pending_untyped(), &HashMap::new()));
        assert!(!unknown_receiver(&Ty::Int, &HashMap::new()));
    }
}

//! Program identities for the opt-in fixed equation system (RH_STRUCT).
//! No inferred value selects a syntax identity or a return's reference policy.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;
use crate::{App, expr::{Expr, ExprNode}, ident::{ClassId, Symbol}, ty::Ty};
use super::{body::{ClassInfo, Ctx}, fold};

static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_STRUCT").is_ok_and(|v| v == "1"));
pub(super) fn on() -> bool { *ON && fold::on() }

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Context {
    pub source: u64,
    pub owner: Option<ClassId>,
    pub includer: Option<ClassId>,
    pub class_side: bool,
}
#[derive(Clone, Debug)]
struct Source {
    owner: Option<ClassId>,
    identity: String,
}
#[derive(Default)]
struct State {
    sources: BTreeMap<u64, Source>,
    contracts: HashSet<(ClassId, Symbol, bool)>,
    collisions: u64,
    missing_sources: BTreeSet<fold::SiteId>,
}
thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
    static CURRENT: RefCell<Context> = RefCell::new(Context::default());
}
pub(super) fn reset() {
    if on() {
        ST.with(|s| *s.borrow_mut() = State::default());
        CURRENT.with(|s| *s.borrow_mut() = Context::default());
    }
}
fn hash(value: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h); h.finish().max(1)
}
pub(super) fn derived_receiver_id(parent: u64) -> u64 {
    if on() && parent != 0 { hash((parent, "implicit-self")) } else { 0 }
}

pub(super) struct Guard(Option<Context>);
impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(old) = self.0.take() { CURRENT.with(|s| *s.borrow_mut() = old); }
    }
}
pub(super) fn enter(expr: &Expr, ctx: &Ctx) -> Guard {
    if !on() || !fold::active() { return Guard(None); }
    let owner = ST.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(source) = s.sources.get(&expr.inference_id) { source.owner.clone() }
        else { s.missing_sources.insert(fold::site_of(&expr.span)); None }
    });
    let includer = match &ctx.self_ty { Some(Ty::Class { id, .. }) => Some(id.clone()), _ => owner.clone() };
    let current = Context { source: expr.inference_id, owner, includer, class_side: ctx.class_side };
    Guard(Some(CURRENT.with(|s| std::mem::replace(&mut *s.borrow_mut(), current))))
}
pub(super) fn context() -> Context { CURRENT.with(|s| s.borrow().clone()) }

fn register(root: &mut Expr, owner: Option<&ClassId>, tag: &str) {
    let prefix = format!("{owner:?}:{tag}:{:?}", fold::site_of(&root.span));
    fn walk(e: &mut Expr, owner: Option<&ClassId>, prefix: &str, path: &mut Vec<u32>) {
        let identity = format!("{prefix}:{path:?}");
        let id = hash(&identity);
        let source = Source { owner: owner.cloned(), identity };
        ST.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(old) = s.sources.get(&id) {
                if old.identity != source.identity { s.collisions += 1; }
            }
            s.sources.insert(id, source.clone());
            if matches!(&*e.node, ExprNode::Send { recv: None, .. }) {
                s.sources.insert(derived_receiver_id(id), Source {
                    identity: format!("{}:implicit-self", source.identity), ..source
                });
            }
        });
        e.inference_id = id;
        let mut i = 0;
        e.node.for_each_child_mut(&mut |c| { path.push(i); walk(c, owner, prefix, path); path.pop(); i += 1; });
    }
    walk(root, owner, &prefix, &mut Vec::new());
}
fn method(owner: &ClassId, m: &mut crate::dialect::MethodDef) {
    let tag = format!("method:{:?}:{}", m.receiver, m.name);
    register(&mut m.body, Some(owner), &tag);
    for (i,p) in m.params.iter_mut().enumerate() {
        if let Some(e) = &mut p.default { register(e, Some(owner), &format!("{tag}:default:{i}")); }
    }
}

pub(super) fn capture_sites(app: &mut App) {
    if !on() { return; }
    for model in &mut app.models {
        let owner = model.name.clone();
        for m in model.methods_mut() { method(&owner, m); }
        for scope in model.scopes_mut() { register(&mut scope.body, Some(&owner), &format!("scope:{}", scope.name)); }
        for (name, e) in &mut model.class_attr_defaults { register(e, Some(&owner), &format!("attribute-default:{name}")); }
        for (i,item) in model.body.iter_mut().enumerate() {
            if let crate::dialect::ModelBodyItem::Association { assoc, .. } = item {
                match assoc {
                    crate::dialect::Association::BelongsTo { default: Some(e), .. }
                    | crate::dialect::Association::HasMany { scope: Some(e), .. } => {
                        register(e, Some(&owner), &format!("association:{i}"));
                    }
                    _ => {}
                }
            }
        }
    }
    for lc in app.library_classes.iter_mut().chain(app.rails_application.iter_mut()) {
        for m in &mut lc.methods { method(&lc.name, m); }
        for (n,e) in &mut lc.constants { register(e, Some(&lc.name), &format!("constant:{n}")); }
    }
    for c in &mut app.controllers {
        let owner = c.name.clone();
        for a in c.actions_mut() {
            let tag = format!("action:{}", a.name);
            register(&mut a.body, Some(&owner), &tag);
            for (n,e) in &mut a.opt_params { register(e, Some(&owner), &format!("{tag}:default:{n}")); }
            for (n,e) in &mut a.kw_params { if let Some(e) = e { register(e, Some(&owner), &format!("{tag}:kwdefault:{n}")); } }
        }
        for m in c.class_methods_mut() { method(&owner, m); }
    }
    let mut fallback = 0;
    crate::lower::for_each_owned_hook_body(app, &mut |owner,e| {
        // A retained IR id from a previous analysis is not registered in
        // this analysis's fresh source table. Recapture fallback roots too.
        if !ST.with(|s| s.borrow().sources.contains_key(&e.inference_id)) {
            register(e, owner, &format!("source-root:{fallback}"));
            fallback += 1;
        }
    });
    for v in &mut app.views {
        register(&mut v.body, None, &format!("view:{}", v.name));
        for p in v.strict_locals.iter_mut().flatten() {
            if let Some(e) = &mut p.default { register(e,None,&format!("view:{}:default:{}",v.name,p.name)); }
        }
    }
    for tm in &mut app.test_modules {
        if let Some(e) = &mut tm.setup { register(e,Some(&tm.name),"test-setup"); }
        for (i,t) in tm.tests.iter_mut().enumerate() { register(&mut t.body,Some(&tm.name),&format!("test:{i}")); }
        for m in &mut tm.helpers { method(&tm.name,m); }
        for (n,e) in &mut tm.constants { register(e,Some(&tm.name),&format!("test-constant:{n}")); }
        for lc in &mut tm.inner_classes {
            for m in &mut lc.methods { method(&lc.name,m); }
            for (n,e) in &mut lc.constants { register(e,Some(&lc.name),&format!("constant:{n}")); }
            for (i,e) in lc.unknown_calls.iter_mut().chain(lc.class_ivar_initializers.iter_mut()).enumerate() {
                register(e,Some(&lc.name),&format!("class-root:{i}"));
            }
        }
    }
    for helper in &mut app.routes.direct_helpers { register(&mut helper.body,None,&format!("route:{}",helper.name)); }
    for fixture in &mut app.fixtures {
        let owner = fixture.class_id();
        for (i,e) in fixture.preamble.iter_mut().enumerate() {
            register(e,Some(&owner),&format!("fixture:{}:preamble:{i}",fixture.name));
        }
        for (record, fields) in &mut fixture.records {
            for (field,value) in fields {
                if let crate::dialect::FixtureValue::Ruby(e) = value {
                    register(e,Some(&owner),&format!("fixture:{}:{record}:{field}",fixture.name));
                }
            }
        }
    }
    for f in &mut app.sql_functions {
        let owner = ClassId(Symbol::from("<sql>"));
        match &mut f.kind {
            crate::app::SqlFunctionKind::Scalar { method: m } => method(&owner,m),
            crate::app::SqlFunctionKind::Aggregate { step, finalize } => { method(&owner,step); method(&owner,finalize); }
        }
    }
}

pub(super) fn capture_contracts(classes: &HashMap<ClassId, ClassInfo>) {
    if !on() { return; }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        for (class,info) in classes {
            for (side,table) in [(false,&info.instance_methods),(true,&info.class_methods)] {
                for (m,t) in table {
                    let mut mentions_self = false;
                    fold::visit(t,&mut |t| {
                        mentions_self |= matches!(t,Ty::SelfInstance);
                    });
                    if matches!(t,Ty::Fn { .. }) || mentions_self { s.contracts.insert((class.clone(),m.clone(),side)); }
                }
            }
        }
    });
}
pub(super) fn return_reference(class:&ClassId,method:&Symbol,side:bool)->bool {
    ST.with(|s| !s.borrow().contracts.contains(&(class.clone(),method.clone(),side)))
}

/// Counts and canonical digests only, including when used by private runners.
pub(super) fn summary() -> serde_json::Value {
    ST.with(|s| {
        let s = s.borrow();
        let sources: Vec<_> = s.sources.iter().map(|(id,v)| (id,&v.owner,&v.identity)).collect();
        serde_json::json!({"sources":s.sources.len(), "digest":format!("{:016x}",hash(sources)),
            "collisions":s.collisions, "missing_source_sites":s.missing_sources.len()})
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn nil() -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Lit { value: crate::expr::Literal::Nil })
    }

    #[test]
    fn structure_sites_distinguish_children_and_roots_with_identical_spans() {
        if !on() { return; }
        reset();
        let owner = ClassId(Symbol::from("Source"));
        let mut first = Expr::new(crate::span::Span::synthetic(), ExprNode::Seq { exprs: vec![nil(),nil()] });
        let mut second = first.clone();
        register(&mut first,Some(&owner),"first");
        register(&mut second,Some(&owner),"second");
        assert_ne!(first.inference_id,second.inference_id);
        let ExprNode::Seq { exprs } = &*first.node else { unreachable!() };
        assert_ne!(exprs[0].inference_id,exprs[1].inference_id);
        let ids = exprs.iter().map(|e| e.inference_id).collect::<Vec<_>>();
        first.ty = Some(Ty::Int);
        register(&mut first,Some(&owner),"first");
        let ExprNode::Seq { exprs } = &*first.node else { unreachable!() };
        assert_eq!(ids,exprs.iter().map(|e| e.inference_id).collect::<Vec<_>>());
        assert_eq!(summary()["collisions"],0);
    }

    #[test]
    fn structure_concern_context_retains_definition_and_includer() {
        if !on() { return; }
        reset(); fold::reset();
        let owner = ClassId(Symbol::from("Concern"));
        let mut expr = nil(); register(&mut expr,Some(&owner),"body");
        fold::freeze_structure();
        let mut contexts = Vec::new();
        for host in ["First", "Second"] {
            let mut ctx = Ctx::default();
            ctx.self_ty = Some(Ty::Class { id:ClassId(Symbol::from(host)), args:vec![].into() });
            let guard = enter(&expr,&ctx);
            contexts.push(context());
            drop(guard);
            assert_eq!(context(),Context::default());
        }
        assert_eq!(contexts[0].owner,Some(owner));
        assert_eq!(contexts[0].source,contexts[1].source);
        assert_ne!(contexts[0].includer,contexts[1].includer);
        assert_eq!(summary()["missing_source_sites"],0);
        fold::reset();
    }
}

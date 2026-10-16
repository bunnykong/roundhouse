//! boundary-carry-protocol: preserve the lower model, and decide absence once, over stable slot identities.
use super::{Analyzer, App, ClassId, Expr, ExprNode, LValue, MethodReceiver, ParamKey, Symbol, Ty};
use crate::{
    ident::TyVar,
    ty::{Param, Provenance, Row},
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub(super) fn on() -> bool {
    let on = std::env::var("RH_DEFERRED_CARRY").as_deref() == Ok("1");
    assert!(
        !on || super::strict_producers::deferred_stratify(),
        "carry requires stratification"
    );
    on
}
pub(super) fn second() -> bool {
    on() && STATE.with(|s| s.borrow().frozen)
}
pub(super) fn param_key(c: &ClassId, m: &Symbol, side: MethodReceiver, i: usize) -> String {
    format!("param:{}:{side:?}:{}:{i}", c.0.as_str(), m.as_str())
}
pub(super) fn ret_key(c: &ClassId, m: &Symbol, side: bool) -> String {
    format!("return:{}:{side}:{}", c.0.as_str(), m.as_str())
}
fn side_key(k: &super::fold::SlotKey) -> String {
    format!("side:{k:?}")
}
fn var() -> Ty {
    Ty::Var { var: TyVar(0) }
}
#[derive(Default)]
struct State {
    frozen: bool,
    scope: String,
    writer: String,
    minted: HashMap<String, Ty>,
    observed: HashMap<String, Ty>,
    negative: HashSet<String>,
    carried: HashMap<String, Ty>,
    retracted: HashSet<String>,
    refs: HashMap<u32, Ty>,
    lost: u64,
    reinserted: u64,
    retractions: u64,
    outside: u64,
    negative_size: usize,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
pub(super) fn reset() {
    STATE.with(|s| *s.borrow_mut() = State::default());
}
pub(super) struct Scope(Option<String>);
impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(old) = self.0.take() {
            STATE.with(|s| s.borrow_mut().scope = old);
        }
    }
}
pub(super) fn scope(key: String) -> Scope {
    if !on() {
        return Scope(None);
    }
    Scope(Some(
        STATE.with(|s| std::mem::replace(&mut s.borrow_mut().scope, key)),
    ))
}
pub(super) struct WriterScope {
    scope: Scope,
    previous: Option<String>,
}
impl Drop for WriterScope {
    fn drop(&mut self) {
        if let Some(old) = self.previous.take() {
            STATE.with(|s| s.borrow_mut().writer = old);
        }
    }
}
pub(super) fn writer_scope(key: String) -> WriterScope {
    let previous = if on() {
        Some(STATE.with(|s| std::mem::replace(&mut s.borrow_mut().writer, key.clone())))
    } else {
        None
    };
    WriterScope {
        scope: scope(key),
        previous,
    }
}
pub(super) fn expr_scope(span: crate::span::Span, ctx: &super::body::Ctx) -> Scope {
    if !on() {
        return Scope(None);
    }
    let writer = STATE.with(|s| s.borrow().writer.clone());
    // A producer identity is program structure, never an inferred receiver type.
    scope(format!(
        "expr:{writer}:{}:{}:{}:{}",
        ctx.class_side, span.file.0, span.start, span.end
    ))
}

pub(super) fn observed_expr(t: &Ty) {
    if on() {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if !s.frozen {
                let key = s.scope.clone();
                s.observed.insert(key, t.clone());
            }
        });
    }
}
/// The negative answer is legal only for an input classified at the boundary.
/// A derived input reads the saved lower-stratum value. An unregistered input is a counted defect.
pub(super) fn answer(site: &str) -> Option<Ty> {
    if !on() {
        return None;
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let key = format!("producer:{site}:{}", s.scope);
        if !s.frozen {
            s.minted.insert(key, Ty::pending_untyped());
            return None;
        }
        Some(answer_in(&mut s, &key))
    })
}
fn answer_in(s: &mut State, key: &str) -> Ty {
    if s.negative.contains(key) {
        return Ty::unresolved();
    }
    if let Some(t) = s.carried.get(key) {
        return t.clone();
    }
    s.outside += 1;
    Ty::pending_untyped()
}
#[cfg(test)]
fn answer_for_key(key: &str) -> Ty {
    STATE.with(|s| answer_in(&mut s.borrow_mut(), key))
}
pub(super) fn seed(key: String, value: Option<Ty>) -> Option<Ty> {
    if !second() {
        return value;
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.negative.contains(&key) {
            let value = value.or_else(|| s.carried.get(&key).cloned());
            return value.and_then(|t| {
                let t = resolve_negative(&t, &s.refs, &var());
                if matches!(t, Ty::Var { .. }) {
                    None
                } else {
                    Some(t)
                }
            });
        }
        if value
            .as_ref()
            .is_none_or(|t| has_pending(t) || matches!(t, Ty::Var { .. }))
        {
            if let Some(t) = s.carried.get(&key) {
                return Some(t.clone());
            }
            s.outside += 1;
        }
        value
    })
}
pub(super) fn retracted(k: &super::fold::SlotKey) -> bool {
    second() && STATE.with(|s| s.borrow().retracted.contains(&side_key(k)))
}
pub(super) fn report() -> serde_json::Value {
    STATE.with(|s| {
        let s = s.borrow();
        serde_json::json!({
    "carry":on(),"boundary_rows_lost":s.lost,"boundary_rows_reinserted":s.reinserted,
    "boundary_placeholders_retracted":s.retractions,"stratum2_answers_outside_set":s.outside,
    "never_derived_size":s.negative_size})
    })
}
fn note_lost(n: u64) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.lost += n;
        s.reinserted += n;
    });
}

/// Reference graph inspection: references and unguarded cycles are not independent evidence.
/// Constructors preserve their known shape, and mixed rows keep all concrete components.
fn evidence(t: &Ty, refs: &HashMap<u32, Ty>, seen: &mut HashSet<u32>) -> (bool, bool) {
    match t {
        Ty::Untyped { why } => (why.reporting_bits() & 6 != 0, why.reporting_bits() & 1 != 0),
        Ty::Var { .. } => (false, true),
        Ty::Rec { slot } => {
            if !seen.insert(*slot) {
                return (false, true);
            }
            let answer = refs
                .get(slot)
                .map(|v| evidence(v, refs, seen))
                .unwrap_or((false, true));
            seen.remove(slot);
            answer
        }
        Ty::Union { variants } => variants.iter().fold((false, false), |(a, b), v| {
            let (c, d) = evidence(v, refs, seen);
            (a || c, b || d)
        }),
        _ => {
            let mut pending = false;
            each_child(t, &mut |v| pending |= evidence(v, refs, seen).1);
            (true, pending)
        }
    }
}
fn each_child(t: &Ty, f: &mut impl FnMut(&Ty)) {
    match t {
        Ty::Array { elem } => f(elem),
        Ty::Hash { key, value } => {
            f(key);
            f(value)
        }
        Ty::Tuple { elems } => elems.iter().for_each(f),
        Ty::Union { variants } => variants.iter().for_each(f),
        Ty::Class { args, .. } => args.iter().for_each(f),
        Ty::Record { row } => row.fields.values().for_each(f),
        Ty::Fn {
            params, block, ret, ..
        } => {
            for p in params.iter() {
                f(&p.ty)
            }
            if let Some(b) = block {
                f(b)
            }
            f(ret)
        }
        _ => {}
    }
}
fn has_pending(t: &Ty) -> bool {
    if matches!(t,Ty::Untyped{why} if why.reporting_bits()&1!=0) {
        return true;
    }
    let mut found = false;
    each_child(t, &mut |x| found |= has_pending(x));
    found
}
/// Replace only negative positions. A concrete reference retains its identity.
fn resolve_negative(t: &Ty, refs: &HashMap<u32, Ty>, fallback: &Ty) -> Ty {
    match t {
        Ty::Untyped { why } if why.reporting_bits() & 1 != 0 => {
            let bits = why.reporting_bits();
            if bits & 6 == 0 {
                fallback.clone()
            } else {
                Ty::untyped(if bits & 2 != 0 && bits & 4 != 0 {
                    Provenance::Gradual.union(Provenance::Unresolved)
                } else if bits & 2 != 0 {
                    Provenance::Gradual
                } else {
                    Provenance::Unresolved
                })
            }
        }
        Ty::Rec { .. } if !evidence(t, refs, &mut HashSet::new()).0 => fallback.clone(),
        Ty::Array { elem } => Ty::Array {
            elem: Arc::new(resolve_negative(elem, refs, fallback)),
        },
        Ty::Hash { key, value } => Ty::Hash {
            key: Arc::new(resolve_negative(key, refs, fallback)),
            value: Arc::new(resolve_negative(value, refs, fallback)),
        },
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems
                .iter()
                .map(|x| resolve_negative(x, refs, fallback))
                .collect(),
        },
        Ty::Union { variants } => Ty::Union {
            variants: variants
                .iter()
                .map(|x| resolve_negative(x, refs, fallback))
                .collect(),
        },
        Ty::Class { id, args } => Ty::Class {
            id: id.clone(),
            args: args
                .iter()
                .map(|x| resolve_negative(x, refs, fallback))
                .collect(),
        },
        Ty::Record { row } => Ty::Record {
            row: Row {
                fields: row
                    .fields
                    .iter()
                    .map(|(n, x)| (n.clone(), resolve_negative(x, refs, fallback)))
                    .collect(),
                rest: row.rest.clone(),
            },
        },
        Ty::Fn {
            params,
            block,
            ret,
            effects,
        } => Ty::Fn {
            params: params
                .iter()
                .map(|p| Param {
                    name: p.name.clone(),
                    kind: p.kind.clone(),
                    ty: Arc::new(resolve_negative(&p.ty, refs, &var())),
                })
                .collect(),
            block: block
                .as_ref()
                .map(|x| Arc::new(resolve_negative(x, refs, fallback))),
            ret: Arc::new(resolve_negative(ret, refs, fallback)),
            effects: effects.clone(),
        },
        _ => t.clone(),
    }
}
/// Retraction additionally needs a deferral witness: bare Var or an empty cycle alone is not a minted placeholder.
fn deferred_placeholder(t: &Ty, refs: &HashMap<u32, Ty>, seen: &mut HashSet<u32>) -> bool {
    match t {
        Ty::Untyped { why } => why.reporting_bits() & 1 != 0,
        Ty::Rec { slot } => {
            if !seen.insert(*slot) {
                return false;
            }
            let p = refs
                .get(slot)
                .is_some_and(|t| deferred_placeholder(t, refs, seen));
            seen.remove(slot);
            p
        }
        _ => {
            let mut p = false;
            each_child(t, &mut |c| p |= deferred_placeholder(c, refs, seen));
            p
        }
    }
}
/// Number of frozen negative leaf positions; aliases are counted at each stable holding row.
fn negative_positions(t: &Ty, refs: &HashMap<u32, Ty>, seen: &mut HashSet<u32>) -> usize {
    match t {
        Ty::Untyped { why } => usize::from(why.reporting_bits() & 1 != 0),
        Ty::Var { .. } => 1,
        Ty::Rec { slot } => {
            if !seen.insert(*slot) {
                return 1;
            }
            let n = refs
                .get(slot)
                .map_or(1, |t| negative_positions(t, refs, seen));
            seen.remove(slot);
            n
        }
        _ => {
            let mut n = 0;
            each_child(t, &mut |c| n += negative_positions(c, refs, seen));
            n
        }
    }
}
fn freeze_model(
    rows: &HashMap<String, Ty>,
    universe: HashSet<String>,
    refs: &HashMap<u32, Ty>,
    slots: &[(u32, super::fold::SlotKey, Option<Ty>)],
) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        for key in universe {
            let ev = rows
                .get(&key)
                .map(|t| evidence(t, refs, &mut HashSet::new()))
                .unwrap_or((false, true));
            if ev.1 || !ev.0 {
                s.negative.insert(key.clone());
            }
            if ev.0 {
                if let Some(t) = rows.get(&key) {
                    let fallback = if key.starts_with("param:") || key.starts_with("keyword:") {
                        var()
                    } else {
                        Ty::unresolved()
                    };
                    s.carried.insert(key, resolve_negative(t, refs, &fallback));
                }
            }
        }
        for (_, key, value) in slots {
            if value.as_ref().is_some_and(|t| {
                !evidence(t, refs, &mut HashSet::new()).0
                    && deferred_placeholder(t, refs, &mut HashSet::new())
            }) {
                s.retracted.insert(side_key(key));
            }
        }
        s.refs = refs.clone();
        s.retractions = s.retracted.len() as u64;
        s.negative_size = s
            .negative
            .iter()
            .map(|k| {
                rows.get(k).map_or(1, |t| {
                    negative_positions(t, refs, &mut HashSet::new()).max(1)
                })
            })
            .sum();
        s.frozen = true;
    });
}
#[derive(Default)]
pub(super) struct Boundary {
    params: HashMap<ParamKey, Vec<Ty>>,
    app_params: HashMap<(ClassId, Symbol), Vec<Ty>>,
    bindings: HashMap<(ClassId, Symbol), HashMap<Symbol, Ty>>,
    sides: HashMap<u32, Ty>,
}
fn preserve_vectors<K: Eq + std::hash::Hash + Clone>(
    old: &HashMap<K, Vec<Ty>>,
    now: &mut HashMap<K, Vec<Ty>>,
) -> u64 {
    let mut lost = 0;
    for (key, row) in old {
        let n = now.get(key).map_or(0, Vec::len);
        if n < row.len() {
            lost += (row.len() - n) as u64;
            let dest = now.entry(key.clone()).or_default();
            dest.extend_from_slice(&row[n..]);
        }
    }
    lost
}
impl Analyzer {
    pub(super) fn boundary_carry_freeze(&mut self, app: &mut App) {
        if !on() {
            return;
        }
        let slots = super::fold::boundary_carry_snapshot();
        let mut refs = HashMap::new();
        for (slot, _, _) in &slots {
            if let Some(value) = super::fold::value_of(*slot, &self.classes) {
                refs.insert(*slot, value);
            }
        }
        let mut rows: HashMap<String, Ty> = HashMap::new();
        let mut universe = HashSet::new();
        for (c, m, side) in Self::defined_methods(app) {
            universe.insert(ret_key(&c, &m, side == MethodReceiver::Class));
        }
        for ((c, m, side), shape) in Self::param_shapes(app) {
            for i in 0..shape.slots.len() {
                universe.insert(param_key(&c, &m, side, i));
            }
        }
        for (c, info) in &self.classes {
            for (side, methods) in [(false, &info.instance_methods), (true, &info.class_methods)] {
                for (m, t) in methods {
                    rows.insert(ret_key(c, m, side), t.clone());
                }
            }
        }
        for ((c, m, side), types) in &self.inferred_params {
            for (i, t) in types.iter().enumerate() {
                rows.insert(param_key(c, m, *side, i), t.clone());
            }
        }
        for ((c, m), bindings) in &self.refined_action_bindings {
            for (n, t) in bindings {
                rows.insert(
                    format!("ivar:{}:{}:{}", c.0.as_str(), m.as_str(), n.as_str()),
                    t.clone(),
                );
            }
        }
        fn declared_ivars(
            e: &Expr,
            universe: &mut HashSet<String>,
            rows: &mut HashMap<String, Ty>,
        ) {
            let target = match &*e.node {
                ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => Some(target),
                _ => None,
            };
            if let Some(LValue::Ivar { name }) = target {
                let key = format!(
                    "assigned-ivar:{}:{}:{}:{}",
                    e.span.file.0,
                    e.span.start,
                    e.span.end,
                    name.as_str()
                );
                universe.insert(key.clone());
                if let Some(t) = &e.ty {
                    rows.insert(key, t.clone());
                }
            }
            if let Some(t) = &e.ty {
                if has_pending(t) {
                    let key = format!(
                        "ir:{}:{}:{}:{}",
                        e.span.file.0,
                        e.span.start,
                        e.span.end,
                        format!("{:?}", std::mem::discriminant(&*e.node))
                    );
                    universe.insert(key.clone());
                    rows.insert(key, t.clone());
                }
            }
            e.node
                .for_each_child(&mut |c| declared_ivars(c, universe, rows));
        }
        crate::lower::for_each_emit_body_ref(app, &mut |e| {
            declared_ivars(e, &mut universe, &mut rows)
        });
        for (_, key, value) in &slots {
            let key = side_key(key);
            universe.insert(key.clone());
            if let Some(t) = value {
                rows.insert(key, t.clone());
            }
        }
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            for (key, value) in &s.minted {
                let scope = key.splitn(3, ':').nth(2).unwrap_or("");
                rows.insert(
                    key.clone(),
                    rows.get(scope)
                        .or_else(|| s.observed.get(scope))
                        .cloned()
                        .unwrap_or_else(|| value.clone()),
                );
                universe.insert(key.clone());
                if key.starts_with("producer:param_seed:") {
                    rows.entry(scope.into()).or_insert_with(|| value.clone());
                    universe.insert(scope.into());
                }
            }
            // All observer-backed propagated Pending rows are tracked stratum-1 rows as well.
            universe.extend(
                rows.iter()
                    .filter(|(_, t)| evidence(t, &refs, &mut HashSet::new()).1)
                    .map(|(k, _)| k.clone()),
            );
        });
        freeze_model(&rows, universe, &refs, &slots);

        // Normalize the boundary's negative positions before upper-stratum consumers run.
        let mut boundary = Boundary::default();
        for (key, row) in &mut self.inferred_params {
            let derived = row
                .iter()
                .any(|t| evidence(t, &refs, &mut HashSet::new()).0);
            for t in row.iter_mut() {
                *t = resolve_negative(t, &refs, &var());
            }
            if derived {
                boundary.params.insert(key.clone(), row.clone());
            }
        }
        for (key, row) in &mut app.inferred_method_params {
            let derived = row
                .iter()
                .any(|t| evidence(t, &refs, &mut HashSet::new()).0);
            for t in row.iter_mut() {
                *t = resolve_negative(t, &refs, &var());
            }
            if derived {
                boundary.app_params.insert(key.clone(), row.clone());
            }
        }
        for (key, bindings) in &mut self.refined_action_bindings {
            let mut derived = HashMap::new();
            for (n, t) in bindings {
                let keep = evidence(t, &refs, &mut HashSet::new()).0;
                *t = resolve_negative(t, &refs, &var());
                if keep {
                    derived.insert(n.clone(), t.clone());
                }
            }
            if !derived.is_empty() {
                boundary.bindings.insert(key.clone(), derived);
            }
        }
        let mut updated = HashMap::new();
        for (slot, key, value) in &slots {
            if retracted(key) {
                continue;
            }
            if let Some(t) = value {
                let t = resolve_negative(t, &refs, &Ty::unresolved());
                boundary.sides.insert(*slot, t.clone());
                updated.insert(*slot, t);
            }
        }
        super::fold::boundary_carry_replace_values(updated);
        for info in self.classes.values_mut() {
            for t in info
                .instance_methods
                .values_mut()
                .chain(info.class_methods.values_mut())
            {
                *t = resolve_negative(t, &refs, &Ty::unresolved());
            }
        }
        fn rewrite(e: &mut Expr, refs: &HashMap<u32, Ty>) {
            if let Some(t) = &mut e.ty {
                *t = resolve_negative(t, refs, &Ty::unresolved());
            }
            e.node.for_each_child_mut(&mut |c| rewrite(c, refs));
        }
        crate::lower::for_each_emit_body_mut(app, &mut |e| rewrite(e, &refs));
        for (_, (bindings, bodies)) in &mut self.controller_action_meta_cache {
            for table in bindings.values_mut() {
                for t in table.values_mut() {
                    *t = resolve_negative(t, &refs, &var());
                }
            }
            for body in bodies.values_mut() {
                rewrite(body, &refs);
            }
        }
        if let Some(seeds) = &mut self.view_seeds {
            for channel in [
                &mut seeds.action_ivars_by_view,
                &mut seeds.layout_ivars_by_view,
                &mut seeds.content_partial_ivars,
            ] {
                for bindings in channel.values_mut() {
                    for t in bindings.values_mut() {
                        *t = resolve_negative(t, &refs, &var());
                    }
                }
            }
            for t in seeds.mailer_params_by_view.values_mut() {
                *t = resolve_negative(t, &refs, &var());
            }
        }
        for bindings in app
            .view_ivar_types
            .values_mut()
            .chain(app.partial_local_types.values_mut())
        {
            for t in bindings.values_mut() {
                *t = resolve_negative(t, &refs, &var());
            }
        }
        self.boundary_carry_boundary = Some(boundary);
    }
    pub(super) fn boundary_carry_preserve(&mut self) {
        if !second() {
            return;
        }
        let Some(b) = self.boundary_carry_boundary.as_ref() else {
            return;
        };
        let mut lost = preserve_vectors(&b.params, &mut self.inferred_params);
        for (key, old) in &b.bindings {
            let now = self.refined_action_bindings.entry(key.clone()).or_default();
            for (n, t) in old {
                if !now.contains_key(n) {
                    lost += 1;
                    now.insert(n.clone(), t.clone());
                }
            }
        }
        let existing = super::fold::boundary_carry_snapshot()
            .into_iter()
            .filter_map(|(id, _, v)| v.map(|t| (id, t)))
            .collect::<HashMap<_, _>>();
        for (id, t) in &b.sides {
            if !existing.contains_key(id) {
                lost += 1;
                super::fold::boundary_carry_restore_value(*id, t.clone());
            }
        }
        note_lost(lost);
    }
    pub(super) fn boundary_carry_preserve_app(&mut self, app: &mut App) {
        self.boundary_carry_preserve();
        if let Some(b) = self.boundary_carry_boundary.as_ref() {
            note_lost(preserve_vectors(
                &b.app_params,
                &mut app.inferred_method_params,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> (HashMap<u32, Ty>, HashMap<String, Ty>, HashSet<String>) {
        let refs = HashMap::from([
            (0, Ty::pending_untyped()),
            (1, Ty::Rec { slot: 0 }),
            (2, Ty::Int),
        ]);
        let rows = HashMap::from([
            ("class".into(), refs[&0].clone()),
            ("instance".into(), refs[&1].clone()),
            ("parameter".into(), Ty::Rec { slot: 0 }),
            ("concrete".into(), Ty::Rec { slot: 2 }),
            (
                "helper".into(),
                Ty::Class {
                    id: ClassId(Symbol::from("Room")),
                    args: vec![].into(),
                },
            ),
        ]);
        let mut declared = rows.keys().cloned().collect::<HashSet<_>>();
        declared.insert("absent".into());
        (refs, rows, declared)
    }
    #[test]
    fn boundary_carry_reference_absent_mixed_and_cycle() {
        let (mut refs, rows, declared) = snapshot();
        for n in ["class", "instance", "parameter"] {
            assert_eq!(
                evidence(&rows[n], &refs, &mut HashSet::new()),
                (false, true)
            );
        }
        assert!(evidence(&rows["concrete"], &refs, &mut HashSet::new()).0);
        assert_eq!(
            resolve_negative(&rows["concrete"], &refs, &Ty::unresolved()),
            rows["concrete"]
        );
        let negative = declared
            .iter()
            .filter(|k| {
                rows.get(*k)
                    .is_none_or(|t| evidence(t, &refs, &mut HashSet::new()).1)
            })
            .count();
        assert_eq!(negative, 4);
        let mixed = Ty::Union {
            variants: vec![Ty::Int, Ty::pending_untyped()].into(),
        };
        assert_eq!(evidence(&mixed, &refs, &mut HashSet::new()), (true, true));
        assert!(
            matches!(resolve_negative(&mixed,&refs,&Ty::unresolved()),Ty::Union{variants} if variants[0]==Ty::Int && variants[1].provenance()==Some(Provenance::Unresolved))
        );
        refs.insert(3, Ty::Rec { slot: 3 });
        assert!(!deferred_placeholder(&refs[&3], &refs, &mut HashSet::new()));
        assert!(!deferred_placeholder(&var(), &refs, &mut HashSet::new()));
        assert_eq!(
            evidence(&refs[&3], &refs, &mut HashSet::new()),
            (false, true)
        );
    }
    #[test]
    #[ignore]
    fn boundary_carry_boundary() {
        reset();
        super::super::strict_producers::deferred_reset();
        super::super::fold::reset();
        let carry = on();
        if carry {
            // Changing an inferred self type must not create a new upper-stratum producer identity.
            let _owner = writer_scope("IdentityOwner#method".into());
            let span = crate::span::Span::synthetic();
            {
                let ctx = super::super::body::Ctx {
                    self_ty: Some(Ty::Int),
                    ..Default::default()
                };
                let _site = expr_scope(span, &ctx);
                assert!(answer("timeout").is_none());
            }
            let rows = STATE.with(|s| s.borrow().minted.clone());
            let universe = rows.keys().cloned().collect();
            freeze_model(&rows, universe, &HashMap::new(), &[]);
            {
                let ctx = super::super::body::Ctx {
                    self_ty: Some(Ty::Str),
                    ..Default::default()
                };
                let _site = expr_scope(span, &ctx);
                assert!(
                    matches!(answer("timeout"),Some(Ty::Untyped{why}) if why==Provenance::Unresolved)
                );
            }
            assert_eq!(report()["stratum2_answers_outside_set"], 0);
        }
        reset();

        // Real test-only caller: a production replay used to discard this parameter row.
        let mut app = App::default();
        app.test_modules.push(
            crate::ingest::ingest_test_file(
                br#"
class BoundaryTest < ActiveSupport::TestCase
  def helper(x)
    x
  end
  test "calls helper" do
    helper(17)
  end
end
"#,
                "boundary_carry-boundary.rb",
            )
            .unwrap()
            .unwrap(),
        );
        let owner = ClassId(Symbol::from("BoundaryTest"));
        let method = Symbol::from("helper");
        let key = (owner.clone(), method.clone(), MethodReceiver::Instance);
        let mut analyzer = Analyzer::new(&app);
        analyzer.type_tests_only(&mut app);
        analyzer.overlay_test_params(&app);
        analyzer.type_tests_only(&mut app);
        analyzer.harvest_returns_to_registry(&app, true);
        assert_eq!(analyzer.inferred_params[&key][0], Ty::Int);
        let before = analyzer.classes[&owner].instance_methods[&method].clone();
        analyzer.boundary_carry_freeze(&mut app);
        super::super::strict_producers::deferred_start_second();
        analyzer.unify_params_from_call_sites(&app, super::super::UnifyScope::Production);
        let helper_ok = if carry {
            analyzer
                .inferred_params
                .get(&key)
                .is_some_and(|v| v[0] == Ty::Int)
        } else {
            analyzer.inferred_params.get(&key).is_none()
        };
        if carry {
            analyzer.type_tests_only(&mut app);
            analyzer.harvest_returns_to_registry(&app, true);
            assert_eq!(analyzer.classes[&owner].instance_methods[&method], before);
            let r = report();
            assert_eq!(r["boundary_rows_lost"], 0);
            assert_eq!(r["boundary_rows_reinserted"], 0);
            assert_eq!(r["stratum2_answers_outside_set"], 0);
        }
        if !carry {
            analyzer.type_tests_only(&mut app);
            analyzer.overlay_test_params(&app);
            analyzer.type_tests_only(&mut app);
            analyzer.harvest_returns_to_registry(&app, true);
            analyzer.type_tests_only(&mut app);
        }
        analyzer.settle_pending(&mut app);
        let settled = super::super::strict_producers::deferred_report()["settled_pending"].clone();
        assert!(helper_ok);
        // Exact four-slot graph: both aliases, the existing parameter and one absent declaration.
        reset();
        let (refs, mut rows, mut universe) = snapshot();
        let class = super::super::fold::SlotKey::Param {
            class: ClassId(Symbol::from("Current")),
            method: Symbol::from("request="),
            side: MethodReceiver::Class,
            index: 0,
        };
        let instance = super::super::fold::SlotKey::Param {
            class: ClassId(Symbol::from("Current")),
            method: Symbol::from("request="),
            side: MethodReceiver::Instance,
            index: 0,
        };
        for (name, key) in [("class", &class), ("instance", &instance)] {
            let value = rows.remove(name).unwrap();
            universe.remove(name);
            let k = side_key(key);
            universe.insert(k.clone());
            rows.insert(k, value);
        }
        let slots = vec![
            (0, class.clone(), Some(refs[&0].clone())),
            (1, instance.clone(), Some(refs[&1].clone())),
        ];
        if carry {
            freeze_model(&rows, universe, &refs, &slots);
        }
        let helper = rows["helper"].clone();
        let derived = seed("helper".into(), None);
        let negative = seed("absent".into(), None);
        let parameter = resolve_negative(&rows["parameter"], &refs, &var());
        let derived_ok = if carry {
            derived.as_ref() == Some(&helper)
        } else {
            derived.is_none()
        };
        let neg_answer = if carry {
            let _scope = scope("fixture".into());
            // Use one of the frozen negative identities as the producer input, without adding membership.
            STATE.with(|s| {
                s.borrow_mut().scope = "absent".into();
            });
            answer_for_key("absent")
        } else {
            Ty::unresolved()
        };
        let report = report();
        println!(
            "BOUNDARY_CARRY-BOUNDARY {}",
            serde_json::json!({"selected_tests":["boundary_carry_boundary"],"carry_mode":carry,
            "alias_retracted":!carry || (retracted(&class)&&retracted(&instance)),
            "concrete_reference_survives":evidence(&rows["concrete"],&refs,&mut HashSet::new()).0,
            "mixed_preserved":({let t=Ty::Union{variants:vec![Ty::Int,Ty::pending_untyped()].into()};matches!(resolve_negative(&t,&refs,&Ty::unresolved()),Ty::Union{variants} if variants[0]==Ty::Int)}),
            "absent_in_set":!carry || STATE.with(|s|s.borrow().negative.contains("absent")),
            "parameter_restored":matches!(parameter,Ty::Var{..}),"helper_preserved":helper_ok,"derived_not_answered":derived_ok,
            "negative_answer_unresolved":neg_answer.provenance()==Some(Provenance::Unresolved)&&negative.is_none(),
            "retractions":report["boundary_placeholders_retracted"],"negative_size":report["never_derived_size"],
            "boundary_rows_lost":report["boundary_rows_lost"],"boundary_rows_reinserted":report["boundary_rows_reinserted"],
            "stratum2_answers_outside_set":report["stratum2_answers_outside_set"],"settled_pending":settled})
        );
        assert!(derived_ok);
        assert_eq!(report["stratum2_answers_outside_set"], 0);
        assert_eq!(report["never_derived_size"], if carry { 4 } else { 0 });
    }
}

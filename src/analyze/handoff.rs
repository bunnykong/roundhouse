//! Add-only rules for what one round hands the next (opt-in). With every
//! variable unset nothing here runs.
//!
//! A rule that takes information back can keep a loop flipping while
//! nothing grows, and the fixpoint has nothing to settle to. Two rules
//! that the round loop applies are of that kind:
//!
//! - **First-arm block binding.** A block parameter binds from a union
//!   receiver's first arm that answers: F(Hash) = Hash | Array[Integer],
//!   but F(Hash | Array[Integer]) = Hash. With `RH_BRK_ALLARMS=1` each
//!   parameter joins every arm that answers (`body::send`).
//! - **Last-writer handoffs.** Each harvest rebuilds a method's return and
//!   each unification its parameter row, so a round can drop what the round
//!   before it knew. With `RH_FOLD_JOIN=1` they accumulate: a return joins
//!   with the value it held after the previous harvest, a row with last
//!   round's row, each re-bounded by #584. With `RH_FOLD=1` only the
//!   reference-mode returns and rows accumulate, and they are not
//!   re-bounded: references keep them finite, and that is the
//!   configuration the fixpoint lab measured.
//!
//! The join is a semilattice join on the whole domain, so its result does
//! not depend on the order typings write in: two pending values (no
//! informative core, as in `harvest_return::has_informative_core`) join by
//! `union_of`; a pending value yields to an informative one; informative
//! values join after their unknown arms are stripped, except that a
//! nil-only core keeps its `untyped`, as the harvest's sticky `nil |
//! untyped` does; a value a slot already holds changes nothing. A join that
//! kept the last writer makes two typings of one body flip a slot forever
//! under a worklist.
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use super::body::ClassInfo;
use super::fixpoint_bound::bound;
use crate::ident::{ClassId, Symbol, TyVar};
use crate::ty::{Provenance, Row, Ty};

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

/// All-arms block binding.
pub(crate) static ALLARMS: LazyLock<bool> = LazyLock::new(|| flag("RH_BRK_ALLARMS"));
/// Accumulating handoffs.
static JOIN: LazyLock<bool> = LazyLock::new(|| flag("RH_FOLD_JOIN"));

/// Whether `RH_FOLD_JOIN` is set.
pub(crate) fn join_on() -> bool {
    *JOIN
}

type MethodKey = (ClassId, Symbol);

#[derive(Default)]
struct State {
    /// Each accumulating return and row as the last handoff left it.
    prev_rets: HashMap<(ClassId, Symbol, bool), Ty>,
    prev_params: HashMap<MethodKey, Vec<Ty>>,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

/// Forget the last analysis' handoffs.
pub(crate) fn reset() {
    if *JOIN {
        ST.with(|s| *s.borrow_mut() = State::default());
    }
}

/// Join every inferred return with the value it held after the previous
/// harvest. Called after the harvest and again after the registry copies,
/// so a copy carries the joined value.
pub(crate) fn join_rets(classes: &mut HashMap<ClassId, ClassInfo>) {
    if !*JOIN {
        return;
    }
    if super::fold::on() {
        return join_reference_rets(classes);
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        for (class, cls) in classes.iter_mut() {
            for class_side in [false, true] {
                let table = if class_side { &mut cls.class_methods } else { &mut cls.instance_methods };
                for (method, cur) in table.iter_mut() {
                    if matches!(cur, Ty::Fn { .. }) {
                        continue;
                    }
                    let key = (class.clone(), method.clone(), class_side);
                    if let Some(old) = s.prev_rets.get(&key) {
                        let joined = bound(join_slot(old.clone(), cur.clone()));
                        if &joined != cur {
                            *cur = joined;
                        }
                    }
                    s.prev_rets.insert(key, cur.clone());
                }
            }
        }
    });
}

/// With the fold: only reference-mode returns join, unbounded.
fn join_reference_rets(classes: &mut HashMap<ClassId, ClassInfo>) {
    if !super::fold::active() {
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        for (class, cls) in classes.iter_mut() {
            for class_side in [false, true] {
                let table = if class_side { &mut cls.class_methods } else { &mut cls.instance_methods };
                for (method, cur) in table.iter_mut() {
                    if matches!(cur, Ty::Fn { .. }) || !super::fold::in_reference_mode(class, method) {
                        continue;
                    }
                    let key = (class.clone(), method.clone(), class_side);
                    let joined = match s.prev_rets.get(&key) {
                        Some(old) => join_slot(old.clone(), cur.clone()),
                        None => cur.clone(),
                    };
                    if &joined != cur {
                        *cur = joined.clone();
                    }
                    s.prev_rets.insert(key, joined);
                }
            }
        }
    });
}

/// Join every parameter row with the row it had last round.
pub(crate) fn join_params(params: &mut HashMap<MethodKey, Vec<Ty>>) {
    if !*JOIN {
        return;
    }
    if super::fold::on() {
        return join_reference_params(params);
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let keys: Vec<MethodKey> = s.prev_params.keys().cloned().collect();
        for key in keys {
            let prev = s.prev_params[&key].clone();
            let row = params.entry(key).or_default();
            if row.len() < prev.len() {
                row.resize(prev.len(), Ty::Var { var: TyVar(0) });
            }
            for (slot, old) in row.iter_mut().zip(prev) {
                *slot = bound(join_slot(old, slot.clone()));
            }
        }
        params.retain(|_, row| !row.is_empty());
        s.prev_params.clone_from(params);
    });
}

/// With the fold: only reference-mode rows join, unbounded.
fn join_reference_params(params: &mut HashMap<MethodKey, Vec<Ty>>) {
    if !super::fold::active() {
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        for key in super::fold::reference_mode_methods() {
            let prev = s.prev_params.get(&key).cloned();
            let row = params.entry(key.clone()).or_default();
            if let Some(prev) = prev {
                if row.len() < prev.len() {
                    row.resize(prev.len(), Ty::Var { var: TyVar(0) });
                }
                for (slot, old) in row.iter_mut().zip(prev) {
                    *slot = join_slot(old, slot.clone());
                }
            }
            if row.is_empty() {
                params.remove(&key);
            } else {
                s.prev_params.insert(key, row.clone());
            }
        }
    });
}

/// [`join_rets`] for one reference-mode method (both sides), after the
/// worklist harvested it.
pub(crate) fn join_ret_one(classes: &mut HashMap<ClassId, ClassInfo>, class: &ClassId, method: &Symbol) {
    if !super::fold::active() || !*JOIN || !super::fold::in_reference_mode(class, method) {
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let Some(cls) = classes.get_mut(class) else { return };
        for class_side in [false, true] {
            let table = if class_side { &mut cls.class_methods } else { &mut cls.instance_methods };
            let Some(cur) = table.get(method) else { continue };
            if matches!(cur, Ty::Fn { .. }) {
                continue;
            }
            let key = (class.clone(), method.clone(), class_side);
            let joined = match s.prev_rets.get(&key) {
                Some(old) => join_slot(old.clone(), cur.clone()),
                None => cur.clone(),
            };
            if &joined != cur {
                table.insert(method.clone(), joined.clone());
            }
            s.prev_rets.insert(key, joined);
        }
    });
}

/// [`join_params`] for one row the worklist recomputed: a reference-mode
/// method's row joins with its previous row (`commit` records it).
pub(crate) fn join_param_row(key: &MethodKey, raw: Option<Vec<Ty>>, commit: bool) -> Option<Vec<Ty>> {
    if !super::fold::active() || !*JOIN || !super::fold::in_reference_mode(&key.0, &key.1) {
        return raw;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let mut row = raw.unwrap_or_default();
        if let Some(prev) = s.prev_params.get(key).cloned() {
            if row.len() < prev.len() {
                row.resize(prev.len(), Ty::Var { var: TyVar(0) });
            }
            for (slot, old) in row.iter_mut().zip(prev) {
                *slot = join_slot(old, slot.clone());
            }
        }
        if row.is_empty() {
            None
        } else {
            if commit {
                s.prev_params.insert(key.clone(), row.clone());
            }
            Some(row)
        }
    })
}

/// Join of two values of one slot when one is pending: the pending side
/// (`Var`/`untyped` arms only) yields to the other's informative core, as
/// the harvest's own rule does (#521); two informative values join after
/// their unknown arms are stripped.
pub(crate) fn join(old: Ty, new: Ty) -> Ty {
    if old == new {
        return new;
    }
    if !informative(&old) {
        return new;
    }
    if !informative(&new) {
        return old;
    }
    let unknown = old.has_unknown_arm() || new.has_unknown_arm();
    let old = if old.has_unknown_arm() { old.strip_unknown() } else { old };
    let new = if new.has_unknown_arm() { new.strip_unknown() } else { new };
    let joined = super::body::union_of(old, new);
    // An `untyped` beside a nil-only core is a value read from outside
    // (Campfire's URI helpers return a response header, or `nil` from a
    // `rescue`): stripped, the method claims it always returns `nil`. The
    // harvest keeps that pair for the same reason (`harvest_return`).
    if unknown && joined == Ty::Nil {
        return super::body::union_of(Ty::Nil, Ty::unresolved());
    }
    joined
}

/// The join of a slot several typings write: [`join`], except that two
/// pending values join by `union_of`, so the result does not depend on
/// which typing wrote last, and a value the slot already holds changes
/// nothing (commutative and idempotent on the whole domain).
pub(crate) fn join_slot(old: Ty, new: Ty) -> Ty {
    if contains_arm(&old, &new) {
        return old;
    }
    if !informative(&old) && !informative(&new) {
        return super::body::union_of(old, new);
    }
    join(old, new)
}

/// `old == new`, or `old` is a union with `new` as an arm (or with every
/// arm of a union `new`).
pub(crate) fn contains_arm(old: &Ty, new: &Ty) -> bool {
    if old == new {
        return true;
    }
    let Ty::Union { variants } = old else { return false };
    match new {
        Ty::Union { variants: nv } => nv.iter().all(|a| variants.contains(a)),
        other => variants.contains(other),
    }
}

/// Whether what is still pending when analysis ends goes through
/// [`settle_pending`]: with the add-only rules, pending values are kept
/// pending rather than replaced, so some can outlive the loops.
pub(crate) fn settle_on() -> bool {
    *JOIN
}

/// The entry-point policy for what is still pending when analysis ends. A
/// value no caller ever reached (a parameter nobody calls, a method whose
/// only callers the analysis cannot see) is whatever the outside world
/// passes: gradual `untyped`. Pending is never `Ty::Bottom`, which means
/// "never returns" and is emitted as Rust `!`, TypeScript `never`, Python
/// `Never` and RBS `bot`.
///
/// Returns the rewritten type, or `None` when `t` holds nothing pending.
/// `memo` is keyed by node address, so it must not outlive the borrow of
/// `t`: one memo per top-level type.
pub(crate) fn settle_pending(t: &Ty, memo: &mut HashMap<usize, Option<Ty>>) -> Option<Ty> {
    /// The settled form of each item, or `None` when none changes.
    fn each<'a>(items: impl Iterator<Item = &'a Ty>, memo: &mut HashMap<usize, Option<Ty>>) -> Option<Vec<Option<Ty>>> {
        let settled: Vec<Option<Ty>> = items.map(|item| settle_pending(item, memo)).collect();
        settled.iter().any(Option::is_some).then_some(settled)
    }
    /// Write `settled` into `items` in place: rebuilding a shared payload
    /// would intern it, and interning compares on the wire, where the tag is
    /// invisible, so it could hand back the pending original.
    fn apply<'a>(items: impl Iterator<Item = &'a mut Ty>, settled: Vec<Option<Ty>>) {
        for (item, settled) in items.zip(settled) {
            if let Some(settled) = settled {
                *item = settled;
            }
        }
    }
    let key = t as *const Ty as usize;
    if let Some(done) = memo.get(&key) {
        return done.clone();
    }
    let out = match t {
        Ty::Untyped { why: Provenance::Pending } => Some(Ty::gradual()),
        Ty::Array { elem } => settle_pending(elem, memo).map(|e| Ty::Array { elem: Arc::new(e) }),
        Ty::Hash { key, value } => {
            let (k, v) = (settle_pending(key, memo), settle_pending(value, memo));
            (k.is_some() || v.is_some()).then(|| Ty::Hash {
                key: Arc::new(k.unwrap_or_else(|| (**key).clone())),
                value: Arc::new(v.unwrap_or_else(|| (**value).clone())),
            })
        }
        Ty::Tuple { elems } => each(elems.iter(), memo).map(|settled| {
            let mut elems = elems.clone();
            apply(elems.iter_mut(), settled);
            Ty::Tuple { elems }
        }),
        Ty::Union { variants } => each(variants.iter(), memo).map(|settled| {
            let mut variants = variants.clone();
            apply(variants.iter_mut(), settled);
            Ty::Union { variants }
        }),
        Ty::Class { id, args } => each(args.iter(), memo).map(|settled| {
            let mut args = args.clone();
            apply(args.iter_mut(), settled);
            Ty::Class { id: id.clone(), args }
        }),
        Ty::Record { row } => each(row.fields.values(), memo).map(|settled| {
            let mut fields = row.fields.clone();
            apply(fields.values_mut(), settled);
            Ty::Record { row: Row { fields, rest: row.rest.clone() } }
        }),
        Ty::Fn { params, block, ret, effects } => {
            let settled_params = each(params.iter().map(|p| &*p.ty), memo);
            let settled_block = block.as_ref().and_then(|b| settle_pending(b, memo));
            let settled_ret = settle_pending(ret, memo);
            (settled_params.is_some() || settled_block.is_some() || settled_ret.is_some()).then(|| {
                let mut params = params.clone();
                if let Some(settled) = settled_params {
                    for (p, settled) in params.iter_mut().zip(settled) {
                        if let Some(settled) = settled {
                            p.ty = Arc::new(settled);
                        }
                    }
                }
                Ty::Fn {
                    params,
                    block: match (block, settled_block) {
                        (_, Some(b)) => Some(Arc::new(b)),
                        (b, None) => b.clone(),
                    },
                    ret: settled_ret.map(Arc::new).unwrap_or_else(|| ret.clone()),
                    effects: effects.clone(),
                }
            })
        }
        _ => None,
    };
    memo.insert(key, out.clone());
    out
}

fn informative(t: &Ty) -> bool {
    match t {
        Ty::Untyped { .. } | Ty::Var { .. } | Ty::Bottom => false,
        Ty::Union { variants } => variants.iter().any(informative),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn union(v: Vec<Ty>) -> Ty {
        Ty::Union { variants: v.into() }
    }

    /// Slot joins must not depend on which typing wrote last: commutative
    /// and idempotent, pending values included (a join that kept the last
    /// writer flipped one slot 701,502 times under a worklist).
    #[test]
    fn slot_joins_are_semilattice_joins_on_pending_values() {
        let values = [
            Ty::unresolved(),
            Ty::Var { var: TyVar(0) },
            union(vec![Ty::unresolved(), Ty::Var { var: TyVar(0) }]),
            Ty::Int,
            union(vec![Ty::Int, Ty::Str]),
        ];
        for a in &values {
            assert_eq!(join_slot(a.clone(), a.clone()), *a);
            for b in &values {
                let ab = join_slot(a.clone(), b.clone());
                let ba = join_slot(b.clone(), a.clone());
                assert_eq!(
                    crate::analyze::fixpoint_check::type_hash(&ab),
                    crate::analyze::fixpoint_check::type_hash(&ba),
                    "{a:?} ⊔ {b:?}"
                );
            }
        }
    }

    /// Stripped, the `untyped` of Campfire's URI helpers left them claiming
    /// to always return `nil` (`tests/pending_split_uri_helpers.rs`).
    #[test]
    fn a_nil_core_keeps_its_untyped_arm() {
        let pending_nil = union(vec![Ty::Var { var: TyVar(0) }, Ty::Nil]);
        let gradual_nil = union(vec![Ty::unresolved(), Ty::Nil]);
        let joined = join_slot(pending_nil.clone(), gradual_nil.clone());
        assert!(joined != Ty::Nil && joined.has_unknown_arm(), "{joined:?}");
        assert_eq!(
            crate::analyze::fixpoint_check::type_hash(&join_slot(gradual_nil, pending_nil)),
            crate::analyze::fixpoint_check::type_hash(&joined)
        );
    }

    /// Leftover pending settles as gradual, wherever it sits, and a type
    /// with nothing pending is left alone.
    #[test]
    fn leftover_pending_settles_as_gradual() {
        let pending = Ty::pending_untyped();
        let t = union(vec![Ty::Str, Ty::Array { elem: Arc::new(pending.clone()) }]);
        let settled = settle_pending(&t, &mut HashMap::new()).expect("rewritten");
        let Ty::Union { variants } = &settled else { panic!("{settled:?}") };
        let Ty::Array { elem } = &variants[1] else { panic!("{settled:?}") };
        assert_eq!(elem.provenance(), Some(Provenance::Gradual));
        assert!(settle_pending(&union(vec![Ty::Str, Ty::unresolved()]), &mut HashMap::new()).is_none());
    }

    #[test]
    fn a_pending_value_yields_to_an_informative_one() {
        assert_eq!(join_slot(Ty::Var { var: TyVar(0) }, Ty::Int), Ty::Int);
        assert_eq!(join_slot(Ty::Int, Ty::unresolved()), Ty::Int);
        assert_eq!(join_slot(union(vec![Ty::Int, Ty::unresolved()]), Ty::Str), union(vec![Ty::Int, Ty::Str]));
    }
}

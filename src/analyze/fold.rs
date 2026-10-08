//! Recursive types by origin, for the whole-program fixpoint. Off unless
//! `RH_FOLD=1`; with it unset nothing here runs and no [`Ty::Rec`] is
//! ever built.
//!
//! A method whose result reaches its own input re-types its body with the
//! previous round's answer pasted in, so after k rounds its type is unfolded
//! k levels deep, until the round cap or #584's bound stops it. Its true
//! type is a cycle (`J = Hash[String, J] | Array[J] | Integer`), and a cycle
//! is finite when it is stored as a reference to where the value came from.
//!
//! In a recursive component, reading a slot yields a reference,
//! `Ty::Rec { slot }`, instead of a copy of the slot's current tree. A slot
//! is a method's return or one of its parameters, or a site-keyed set
//! variable below them:
//!
//! - **Reference mode.** A method is read by reference once it sits in a
//!   non-trivial strongly connected component of the call graph, or, with
//!   `RH_FOLD_SLOTS=1`, of the slot-read graph (`slots.rs`), which also sees
//!   recursion through ivars, memos, constructors and dynamic sends. The set
//!   only grows.
//! - **Unfolding.** A site that needs structure (a send's receiver, a
//!   projection, a block's receiver, `merge`, narrowing, destructuring, the
//!   receiver classes of a call site) unfolds a reference one level
//!   ([`head`]). Each compound child becomes a reference to its position at
//!   that site, `SlotKey::At { site, step }`, whose set variable collects
//!   the child; narrowing a reference yields a reference to a narrowed slot,
//!   `SlotKey::Narrow`. Positions and narrowings are keyed by syntax site,
//!   so the set of slots is finite whatever paths the program takes
//!   (Heintze's set variables per expression). Every other site sees
//!   `untyped`.
//! - **Handoffs.** With `RH_FOLD_JOIN=1`, every slot accumulates with
//!   `handoff`'s semilattice join: a reference-mode return with the value it
//!   held after the last harvest, its parameter row with last round's row,
//!   a site set across typing passes. Under the fold only reference-mode
//!   returns and rows accumulate (`handoff` narrows to them); the others
//!   are rebuilt each round, as on main.
//! - **The side table** (parameter, position and narrowing slots) is state
//!   the registry cannot see: a return that holds a reference compares
//!   equal by slot id when the referenced value moves. A loop is not
//!   converged while it moves ([`side_stable`]), and a round in which it
//!   moved retypes every class, since the dirty frontier cannot see a
//!   referenced value move.
//! - **Expansion.** When analysis ends, every reference is expanded
//!   ([`Expander`]) before anything downstream sees a type: a reference to
//!   a slot outside a cycle fully, a cyclic one down to its back edge,
//!   which reads `untyped`. With `RH_FOLD_TAIL=1` an unguarded back edge
//!   (`X = X | A`, a tail call) contributes nothing, its least solution.
//!   Emitters never meet a reference.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use super::body::ClassInfo;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

static ON: LazyLock<bool> = LazyLock::new(|| flag("RH_FOLD"));
static TAIL: LazyLock<bool> = LazyLock::new(|| flag("RH_FOLD_TAIL"));

/// Node budget of one top-level expansion; past it a reference reads
/// `untyped`.
const EXPAND_NODES: usize = 256;

/// The published S2c already walks maps by key. This opt-in completes the
/// canonical expansion by ordering direct reference arms by slot key too.
static CANON_EXPAND: LazyLock<bool> = LazyLock::new(|| flag("RH_CANON_EXPAND"));

fn reference_arm_order(variants: &[Ty]) -> Vec<usize> {
    let mut order: Vec<(u8, String, usize)> = variants.iter().enumerate().map(|(i, v)| match v {
        Ty::Rec { slot } => (1, format!("{:?}", key_of(*slot)), i),
        _ => (0, String::new(), i),
    }).collect();
    order.sort();
    order.into_iter().map(|(_, _, i)| i).collect()
}


/// A syntax site: the span of the expression at which a reference was
/// unfolded or narrowed.
pub(crate) type SiteId = (u32, u32, u32);

pub(crate) fn site_of(span: &crate::span::Span) -> SiteId {
    (span.file.0, span.start, span.end)
}

/// A site for an unfold with no expression at hand (a dispatch reached
/// from outside a send), keyed by the method name.
pub(crate) fn pseudo_site(tag: &str) -> SiteId {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tag.hash(&mut h);
    (u32::MAX, h.finish() as u32, 0)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SlotKey {
    Ret { class: ClassId, method: Symbol, class_side: bool },
    Param { class: ClassId, method: Symbol, index: usize },
    /// A reference narrowed by a type test (`is_a?`, `case`/`when`, nil
    /// checks) at a narrowing site: the union of everything narrowed there
    /// through `filter`.
    Narrow { site: SiteId, filter: String },
    /// The position `step` below every reference unfolded at `site`; its
    /// value is the union of those children.
    At { site: SiteId, step: Step },
    /// A concern copy retains both its source definition and the self context
    /// on which the copy is read. Never alias these values by module alone.
    CopyRet { owner: ClassId, includer: ClassId, method: Symbol, class_side: bool },
    CopyParam { owner: ClassId, includer: ClassId, method: Symbol, class_side: bool, index: usize },
    Site { context: super::equations::Context, site: SiteId, role: SiteRole },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SiteRole {
    Position(Step),
    Narrow(String),
    ClosureResult,
    ClosureParam(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Step {
    Elem,
    Key,
    Val,
    Idx(u32),
    Field(Symbol),
    Arg(u32),
}

type MethodKey = (ClassId, Symbol);

#[derive(Default)]
struct State {
    /// References are produced only while active: set once the first
    /// recursive component is known, cleared by the final expansion.
    active: bool,
    structure_frozen: bool,
    keys: Vec<SlotKey>,
    ids: HashMap<SlotKey, u32>,
    /// Methods in reference mode. Only grows.
    rec_methods: HashSet<MethodKey>,
    /// Call graph of the current unify pass.
    edges: HashMap<MethodKey, HashSet<MethodKey>>,
    /// The side table: the current value of each parameter, position and
    /// narrowing slot.
    values: HashMap<u32, Ty>,
    /// Typing-pass counter, and the pass of each site slot's last write.
    epoch: u64,
    site_epoch: HashMap<u32, u64>,
    /// Registry copies: `fold_concern_surfaces` copies a concern's entries
    /// onto every includer each round, and dispatch on the includer finds
    /// the copy first. A copy of a reference-mode return is read as a
    /// reference to the module's slot; read by value, the concern re-embeds
    /// its own previous return every round.
    aliases: HashMap<MethodKey, ClassId>,
    static_aliases: HashMap<(ClassId, Symbol, bool), ClassId>,
    late_aliases: HashSet<(ClassId, Symbol, bool)>,
    /// The side table's fingerprint at the last convergence test.
    side_fp: Option<u64>,
    /// Final expansion: back edges cut to `untyped`, tail edges resolved,
    /// budget cuts.
    expansion: [u64; 3],
    /// Fingerprints of the values each site slot has joined since it last
    /// started over, and how many accumulations they made unnecessary.
    joined: HashMap<u32, HashSet<u64>>,
    skipped: u64,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

/// Whether `RH_FOLD` is set.
#[inline]
pub(crate) fn on() -> bool {
    *ON
}

/// Whether references are being produced (`RH_FOLD`, after the first
/// recursive component is known and before the final expansion).
#[inline]
pub(crate) fn active() -> bool {
    *ON && ST.with(|s| s.borrow().active)
}

fn intern(key: SlotKey) -> u32 {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(id) = s.ids.get(&key) {
            return *id;
        }
        let id = s.keys.len() as u32;
        super::det::note_slot_key(id, &key);
        s.keys.push(key.clone());
        s.ids.insert(key, id);
        id
    })
}

pub(crate) fn key_of(slot: u32) -> Option<SlotKey> {
    ST.with(|s| s.borrow().keys.get(slot as usize).cloned())
}

/// Forget the last analysis' slots and reference mode.
pub(crate) fn reset() {
    if *ON {
        ST.with(|s| *s.borrow_mut() = State::default());
    }
}

/// Whether `class#method` itself is in reference mode (not through a
/// registry copy): its return and row are the ones that accumulate.
pub(crate) fn in_reference_mode(class: &ClassId, method: &Symbol) -> bool {
    ST.with(|s| s.borrow().rec_methods.contains(&(class.clone(), method.clone())))
}

/// The methods in reference mode.
pub(crate) fn reference_mode_methods() -> Vec<MethodKey> {
    ST.with(|s| s.borrow().rec_methods.iter().cloned().collect())
}

pub(crate) fn is_rec_method(class: &ClassId, method: &Symbol) -> bool {
    ST.with(|s| {
        let s = s.borrow();
        let key = (class.clone(), method.clone());
        s.rec_methods.contains(&key)
            || (super::equations::on() && [false,true].iter().any(|side| {
                s.static_aliases.get(&(class.clone(),method.clone(),*side))
                    .is_some_and(|src| s.rec_methods.contains(&(src.clone(),method.clone())))
            }))
            || s.aliases.get(&key).is_some_and(|src| s.rec_methods.contains(&(src.clone(), method.clone())))
    })
}

/// `fold_concern_surfaces` copied `module#method` onto `class`.
pub(crate) fn note_alias(class: &ClassId, method: &Symbol, module: &ClassId, class_side: bool) {
    if !*ON {
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if super::equations::on() {
            let key = (class.clone(),method.clone(),class_side);
            if s.structure_frozen {
                if s.static_aliases.get(&key) != Some(module) { s.late_aliases.insert(key); }
            } else {
                s.static_aliases.insert(key,module.clone());
            }
        } else {
            s.aliases.insert((class.clone(), method.clone()), module.clone());
        }
    });
}

/// The module a registry copy on `class` came from, if any.
pub(crate) fn alias_of(class: &ClassId, method: &Symbol) -> Option<ClassId> {
    if super::equations::on() { return alias_of_side(class,method,false); }
    ST.with(|s| s.borrow().aliases.get(&(class.clone(), method.clone())).cloned())
}

fn alias_of_side(class: &ClassId, method: &Symbol, side: bool) -> Option<ClassId> {
    ST.with(|s| s.borrow().static_aliases.get(&(class.clone(),method.clone(),side)).cloned())
}

/// A read of a return slot, when it is in reference mode.
pub(crate) fn ret_ref(class: &ClassId, method: &Symbol, class_side: bool) -> Option<Ty> {
    if !active() || !is_rec_method(class, method) {
        return None;
    }
    if super::equations::on() {
        let key = match alias_of_side(class, method, class_side) {
            Some(owner) => SlotKey::CopyRet { owner, includer: class.clone(), method: method.clone(), class_side },
            None => SlotKey::Ret { class: class.clone(), method: method.clone(), class_side },
        };
        return Some(Ty::Rec { slot: intern(key) });
    }
    // A registry copy reads the defining module's slot.
    let class = ST.with(|s| {
        let s = s.borrow();
        let key = (class.clone(), method.clone());
        if s.rec_methods.contains(&key) {
            class.clone()
        } else {
            s.aliases.get(&key).cloned().unwrap_or_else(|| class.clone())
        }
    });
    let slot = intern(SlotKey::Ret { class, method: method.clone(), class_side });
    Some(Ty::Rec { slot })
}

/// A read of a parameter slot, when it is in reference mode. `value` is
/// what the body would otherwise have been seeded with.
pub(crate) fn param_ref(class: &ClassId, method: &Symbol, class_side: bool, index: usize, value: Ty) -> Ty {
    if !active() || !is_rec_method(class, method) {
        return value;
    }
    let key = if super::equations::on() {
        let owner = alias_of_side(class, method, class_side).unwrap_or_else(|| class.clone());
        SlotKey::CopyParam { owner, includer: class.clone(), method: method.clone(), class_side, index }
    } else { SlotKey::Param { class: class.clone(), method: method.clone(), index } };
    super::structure::fold_write(&key, "parameter-seed");
    let slot = intern(key);
    // A slot's own top-level reference contributes nothing (X = X | A is A).
    let value = strip_self(value, slot);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        // Every typing that seeds this parameter flows into one set
        // variable: two bodies can share the key, and a model's first and
        // reseeded passes both seed it.
        let value = match s.values.get(&slot) {
            Some(old) if super::handoff::join_on() => super::handoff::join_slot(old.clone(), value),
            _ => value,
        };
        if s.values.get(&slot) != Some(&value) {
            note_moved(slot, s.values.get(&slot));
        }
        s.joined.remove(&slot);
        super::det::note("fold.param_seed", s.values.get(&slot), &value);
        s.values.insert(slot, value);
    });
    Ty::Rec { slot }
}

fn strip_self(t: Ty, slot: u32) -> Ty {
    match t {
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Rec { slot: s } if *s == slot)) => {
            let kept: Vec<Ty> =
                variants.into_iter().filter(|v| !matches!(v, Ty::Rec { slot: s } if *s == slot)).collect();
            match kept.len() {
                0 => Ty::pending_untyped(),
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept.into() },
            }
        }
        Ty::Rec { slot: s } if s == slot => Ty::pending_untyped(),
        other => other,
    }
}

/// The current value of a slot: the registry entry for a return, the side
/// table for the others.
pub(crate) fn value_of(slot: u32, classes: &HashMap<ClassId, ClassInfo>) -> Option<Ty> {
    // A typing that reads a slot's value depends on it: the worklist
    // re-types the reader when the value moves.
    super::sccq::rec_fold_slot(slot);
    match key_of(slot)? {
        SlotKey::Ret { class, method, class_side }
        | SlotKey::CopyRet { includer: class, method, class_side, .. } => {
            let value = classes.get(&class).and_then(|cls| {
                let table = if class_side { &cls.class_methods } else { &cls.instance_methods };
                table.get(&method).map(|t| match t {
                    // A function inferred after the declaration snapshot is
                    // the returned closure itself, not a method signature.
                    Ty::Fn { ret, .. } if !super::equations::on()
                        || !super::equations::return_reference(&class, &method, class_side) => (**ret).clone(),
                    other => other.clone(),
                })
            });
            value.or_else(|| super::equations::on().then(super::body::unknown))
        }
        SlotKey::Param { .. } | SlotKey::Narrow { .. } | SlotKey::At { .. }
        | SlotKey::Site { .. } | SlotKey::CopyParam { .. } => {
            ST.with(|s| s.borrow().values.get(&slot).cloned())
                .or_else(|| super::equations::on().then(super::body::unknown))
        }
    }
}

/// A structural fingerprint of `t`, blind to provenance as equality is.
fn fingerprint(t: &Ty) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// Accumulate `value` into a site-keyed slot (its set variable) and return
/// the reference to it. Within one typing pass a site's values join. With
/// `RH_FOLD_JOIN` they also join across passes; without it, the first
/// write in a new pass replaces the last pass's value.
fn accumulate(key: SlotKey, value: Ty) -> Ty {
    if super::fixpoint_check::stats_on() { super::structure::fold_write(&key, "site-transfer"); }
    let slot = intern(key);
    let value = strip_self(value, slot);
    let print = fingerprint(&value);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let epoch = s.epoch;
        let fresh = s.site_epoch.get(&slot).is_none_or(|e| *e < epoch);
        s.site_epoch.insert(slot, epoch);
        // Joins are idempotent: a value this slot has already joined since it
        // last started over changes nothing. Unfolding re-accumulates the same
        // children at every visit, and re-joining them was most of the cost.
        let accumulating = s.values.contains_key(&slot) && (super::handoff::join_on() || !fresh);
        if accumulating && s.joined.get(&slot).is_some_and(|seen| seen.contains(&print)) {
            s.skipped += 1;
            return;
        }
        {
            let seen = s.joined.entry(slot).or_default();
            if !accumulating {
                seen.clear();
            }
            seen.insert(print);
        }
        let joined = match s.values.get(&slot) {
            Some(old) if super::handoff::join_on() => super::handoff::join_slot(old.clone(), value),
            Some(old) if !fresh => super::handoff::join(old.clone(), value),
            _ => value,
        };
        if s.values.get(&slot) != Some(&joined) {
            note_moved(slot, s.values.get(&slot));
        }
        super::det::note("fold.site_slot", s.values.get(&slot), &joined);
        s.values.insert(slot, joined);
    });
    Ty::Rec { slot }
}

/// Whether the state a loop's signature check cannot see is unchanged
/// since the last call: the side table, and `carried`, a fingerprint of
/// other state carried between rounds (the controller bindings). A return
/// or row that holds a reference compares equal by slot id, so the
/// registry alone cannot see a referenced value move. Call once per round,
/// at the convergence test.
pub(crate) fn side_stable(carried: impl FnOnce() -> u64) -> bool {
    if !active() {
        return true;
    }
    let fp = side_fp() ^ carried().rotate_left(17);
    let prev = ST.with(|s| s.borrow_mut().side_fp.replace(fp));
    prev == Some(fp)
}

/// A fingerprint of the controller ivar bindings Phase B refines and the
/// next round reads back, independent of map order.
pub(crate) fn bindings_fp(bindings: &HashMap<(ClassId, Symbol), HashMap<Symbol, Ty>>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut rows: Vec<(&str, &str, Vec<(&str, u64)>)> = bindings
        .iter()
        .map(|((c, m), ivars)| {
            let mut ivars: Vec<(&str, u64)> = ivars
                .iter()
                .map(|(k, t)| {
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    t.hash(&mut h);
                    (k.as_str(), h.finish())
                })
                .collect();
            ivars.sort_unstable();
            (c.0.as_str(), m.as_str(), ivars)
        })
        .collect();
    rows.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    rows.hash(&mut h);
    h.finish()
}

/// Retype every class this round: the side table moved.
pub(crate) fn side_full_retype(side_stable: bool) -> bool {
    active() && !side_stable
}

/// The side table's fingerprint, without recording it.
pub(crate) fn side_fp_peek() -> u64 {
    side_fp()
}

fn side_fp() -> u64 {
    use std::hash::{Hash, Hasher};
    ST.with(|s| {
        let s = s.borrow();
        let mut keys: Vec<&u32> = s.values.keys().collect();
        keys.sort_unstable();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for k in keys {
            k.hash(&mut h);
            s.values[k].hash(&mut h);
        }
        h.finish()
    })
}

/// The side table as one hash per slot, keyed by the slot's key so two
/// runs compare (the fixpoint canaries' carried state).
pub(crate) fn side_table_hashes(h: &mut dyn FnMut(&Ty) -> u64) -> BTreeMap<String, u64> {
    let entries: Vec<(String, Ty)> = ST.with(|s| {
        let s = s.borrow();
        s.values.iter().map(|(slot, t)| (format!("{:?}", s.keys.get(*slot as usize)), t.clone())).collect()
    });
    entries.into_iter().map(|(k, t)| (k, h(&t))).collect()
}

/// A new typing pass begins.
pub(crate) fn new_epoch() {
    if *ON {
        ST.with(|s| s.borrow_mut().epoch += 1);
    }
}

/// The arms a reference denotes: its term's non-union, non-reference arms,
/// following unions and references, each slot once. An unguarded cycle
/// contributes nothing, the least solution of `X = X | ...`. `seen` is a
/// visited set for the whole top-level call, not a path set: re-expanding a
/// slot once per simple path is exponential on rings of references.
pub(crate) fn resolve(slot: u32, classes: &HashMap<ClassId, ClassInfo>, seen: &mut HashSet<u32>) -> Vec<Ty> {
    let mut out = Vec::new();
    if !seen.insert(slot) {
        return out;
    }
    if let Some(v) = value_of(slot, classes) {
        flatten_into(&v, classes, seen, &mut out);
    }
    out
}

fn flatten_into(t: &Ty, classes: &HashMap<ClassId, ClassInfo>, seen: &mut HashSet<u32>, out: &mut Vec<Ty>) {
    match t {
        Ty::Bottom => {}
        Ty::Union { variants } => variants.iter().for_each(|v| flatten_into(v, classes, seen, out)),
        Ty::Rec { slot } => {
            for a in resolve(*slot, classes, seen) {
                if !out.contains(&a) {
                    out.push(a);
                }
            }
        }
        other => {
            if !out.contains(other) {
                out.push(other.clone());
            }
        }
    }
}

/// `arm` with every child that is not a scalar leaf replaced by a
/// reference to its position at `site`, whose set variable accumulates the
/// child. Scalar leaves and unknowns stay in place.
fn shallow(arm: Ty, site: SiteId) -> Ty {
    fn keep(t: &Ty) -> bool {
        matches!(
            t,
            Ty::Int
                | Ty::Float
                | Ty::Bool
                | Ty::Str
                | Ty::Sym
                | Ty::Date
                | Ty::Time
                | Ty::Nil
                | Ty::Var { .. }
                | Ty::Untyped { .. }
                | Ty::Bottom
                | Ty::Relation { .. }
                | Ty::SelfInstance
        ) || matches!(t, Ty::Class { args, .. } if args.is_empty())
    }
    let pos = |child: Ty, step: Step| -> Ty {
        if keep(&child) && !super::equations::on() { child }
        else { accumulate(site_key(site, SiteRole::Position(step)), child) }
    };
    use std::sync::Arc;
    match arm {
        Ty::Array { elem } => Ty::Array { elem: Arc::new(pos(Arc::unwrap_or_clone(elem), Step::Elem)) },
        Ty::Hash { key, value } => Ty::Hash {
            key: Arc::new(pos(Arc::unwrap_or_clone(key), Step::Key)),
            value: Arc::new(pos(Arc::unwrap_or_clone(value), Step::Val)),
        },
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems.into_iter().enumerate().map(|(i, e)| pos(e, Step::Idx(i as u32))).collect(),
        },
        Ty::Record { row } => Ty::Record {
            row: crate::ty::Row {
                fields: row
                    .fields
                    .into_iter()
                    .map(|(k, v)| {
                        let step = Step::Field(k.clone());
                        (k, pos(v, step))
                    })
                    .collect(),
                rest: row.rest,
            },
        },
        Ty::Class { id, args } => Ty::Class {
            id,
            args: args.into_iter().enumerate().map(|(i, a)| pos(a, Step::Arg(i as u32))).collect(),
        },
        other => other,
    }
}

/// The receiver classes a call site reaches through references, each slot
/// visited once (linear in the reference graph). `None` when `t` holds no
/// reference.
pub(crate) fn receiver_classes(t: &Ty, classes: &HashMap<ClassId, ClassInfo>) -> Option<Vec<ClassId>> {
    if !*ON || !contains_rec(t) {
        return None;
    }
    let mut out: Vec<ClassId> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    fn walk(t: &Ty, classes: &HashMap<ClassId, ClassInfo>, seen: &mut HashSet<u32>, out: &mut Vec<ClassId>) {
        match t {
            Ty::Class { id, .. } | Ty::Relation { of: id } => {
                if !out.contains(id) {
                    out.push(id.clone())
                }
            }
            Ty::Array { elem } => walk(elem, classes, seen, out),
            Ty::Union { variants } => variants.iter().for_each(|v| walk(v, classes, seen, out)),
            Ty::Rec { slot } => {
                if seen.insert(*slot) {
                    if let Some(v) = value_of(*slot, classes) {
                        walk(&v, classes, seen, out);
                    }
                }
            }
            _ => {}
        }
    }
    walk(t, classes, &mut seen, &mut out);
    Some(out)
}

/// A reference narrowed at `site` through `filter`: a reference to that
/// site's narrowed slot, whose value accumulates `value`.
fn site_key(site: SiteId, role: SiteRole) -> SlotKey {
    if super::equations::on() {
        SlotKey::Site { context: super::equations::context(), site, role }
    } else {
        match role {
            SiteRole::Position(step) => SlotKey::At { site, step },
            SiteRole::Narrow(filter) => SlotKey::Narrow { site, filter },
            _ => unreachable!("closure slots require RH_STRUCT"),
        }
    }
}

pub(crate) fn narrowed_ref(site: SiteId, filter: String, value: Ty) -> Ty {
    accumulate(site_key(site, SiteRole::Narrow(filter)), value)
}

pub(crate) fn closure_result(site: SiteId, value: Ty) -> Ty {
    if super::equations::on() && active() {
        accumulate(site_key(site, SiteRole::ClosureResult), value)
    } else { value }
}

pub(crate) fn closure_param(site: SiteId, index: usize, value: Ty) -> Ty {
    if super::equations::on() && active() {
        accumulate(site_key(site, SiteRole::ClosureParam(index as u32)), value)
    } else { value }
}

pub(crate) fn return_slot_ids(class: &ClassId, method: &Symbol, class_side: bool) -> Vec<u32> {
    let mut ids = Vec::new();
    if let Some(id) = slot_id(&SlotKey::Ret { class: class.clone(), method: method.clone(), class_side }) {
        ids.push(id);
    }
    if super::equations::on() {
        if let Some(owner) = alias_of_side(class, method, class_side) {
            if let Some(id) = slot_id(&SlotKey::CopyRet { owner, includer: class.clone(), method: method.clone(), class_side }) {
                ids.push(id);
            }
        }
    }
    ids
}

fn has_top_rec(t: &Ty) -> bool {
    match t {
        Ty::Rec { .. } => true,
        Ty::Union { variants } => variants.iter().any(|v| matches!(v, Ty::Rec { .. })),
        _ => false,
    }
}

pub(crate) fn contains_rec(t: &Ty) -> bool {
    let mut found = false;
    visit(t, &mut |c| found |= matches!(c, Ty::Rec { .. }));
    found
}

/// `t` and every type nested in it, in pre-order.
pub(crate) fn visit(t: &Ty, f: &mut dyn FnMut(&Ty)) {
    f(t);
    match t {
        Ty::Array { elem } => visit(elem, f),
        Ty::Hash { key, value } => {
            visit(key, f);
            visit(value, f)
        }
        Ty::Tuple { elems } | Ty::Union { variants: elems } => elems.iter().for_each(|e| visit(e, f)),
        Ty::Record { row } => row.fields.values().for_each(|v| visit(v, f)),
        Ty::Class { args, .. } => args.iter().for_each(|a| visit(a, f)),
        Ty::Fn { params, block, ret, .. } => {
            params.iter().for_each(|p| visit(&p.ty, f));
            if let Some(b) = block {
                visit(b, f)
            }
            visit(ret, f)
        }
        _ => {}
    }
}

/// Unfold the top-level references of `t` by one level, at the syntax site
/// `at`: the arms each denotes, with compound children as references to
/// their positions at `at`. `None` when `t` has none. A reference with no
/// value yet reads `untyped`.
pub(crate) fn head(t: &Ty, at: SiteId, classes: &HashMap<ClassId, ClassInfo>) -> Option<Ty> {
    if !*ON || !has_top_rec(t) {
        return None;
    }
    let mut out: Vec<Ty> = Vec::new();
    push_head(t, at, classes, &mut out);
    Some(match out.len() {
        0 if super::equations::on() => super::body::unknown(),
        0 => Ty::pending_untyped(),
        _ => super::body::union_many(out),
    })
}

fn push_head(t: &Ty, at: SiteId, classes: &HashMap<ClassId, ClassInfo>, out: &mut Vec<Ty>) {
    match t {
        Ty::Rec { slot } => {
            let arms = resolve(*slot, classes, &mut HashSet::new());
            if arms.is_empty() {
                out.push(if super::equations::on() { super::body::unknown() } else { Ty::pending_untyped() });
            }
            for arm in arms {
                out.push(shallow(arm, at));
            }
        }
        Ty::Union { variants } => variants.iter().for_each(|v| push_head(v, at, classes, out)),
        other => out.push(if super::equations::on() && active() { shallow(other.clone(), at) } else { other.clone() }),
    }
}

// ---------------------------------------------------------------- call graph

pub(crate) fn begin_calls() {
    if *ON && !super::equations::on() {
        ST.with(|s| s.borrow_mut().edges.clear());
    }
}

pub(crate) fn record_call(caller: &ClassId, method: &Symbol, callee: MethodKey) {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if s.structure_frozen { return; }
        s.edges.entry((caller.clone(), method.clone())).or_default().insert(callee);
    });
}

pub(crate) fn freeze_structure() {
    ST.with(|s| { let mut s = s.borrow_mut(); s.structure_frozen = true; s.active = true; });
}

/// Close a unify pass: every method in a non-trivial SCC of the call graph
/// enters reference mode.
pub(crate) fn end_calls() {
    if !*ON || super::equations::on() {
        return;
    }
    let sccs = ST.with(|s| tarjan(&s.borrow().edges));
    let mut add = Vec::new();
    ST.with(|s| {
        let s = s.borrow();
        for comp in sccs {
            let nontrivial = comp.len() > 1 || s.edges.get(&comp[0]).is_some_and(|out| out.contains(&comp[0]));
            if nontrivial {
                add.extend(comp);
            }
        }
    });
    add_rec_methods(add);
}

/// Put `methods` into reference mode.
pub(crate) fn add_rec_methods(methods: Vec<MethodKey>) {
    if !*ON || methods.is_empty() {
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        for m in methods {
            if s.rec_methods.insert(m.clone()) {
                s.active = true;
                note_new_rec(m);
            }
        }
    });
}

fn tarjan(edges: &HashMap<MethodKey, HashSet<MethodKey>>) -> Vec<Vec<MethodKey>> {
    // Deterministic node order.
    let mut nodes: Vec<&MethodKey> = edges.keys().chain(edges.values().flatten()).collect();
    nodes.sort_by(|a, b| (a.0.0.as_str(), a.1.as_str()).cmp(&(b.0.0.as_str(), b.1.as_str())));
    nodes.dedup();
    let index_of: HashMap<&MethodKey, usize> = nodes.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let succ: Vec<Vec<usize>> = nodes
        .iter()
        .map(|n| {
            let mut v: Vec<usize> =
                edges.get(*n).map(|out| out.iter().map(|m| index_of[m]).collect()).unwrap_or_default();
            v.sort_unstable();
            v
        })
        .collect();
    scc(&succ).into_iter().map(|c| c.into_iter().map(|i| nodes[i].clone()).collect()).collect()
}

/// Iterative Tarjan: the strongly connected components of a graph given as
/// successor lists.
pub(crate) fn scc(succ: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = succ.len();
    let (mut index, mut low, mut on_stack) = (vec![usize::MAX; n], vec![0usize; n], vec![false; n]);
    let (mut stack, mut out, mut next) = (Vec::new(), Vec::new(), 0usize);
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while !work.is_empty() {
            let top = work.len() - 1;
            let (v, i) = work[top];
            if i < succ[v].len() {
                work[top].1 += 1;
                let w = succ[v][i];
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut comp = Vec::new();
                    loop {
                        let w = stack.pop().unwrap();
                        on_stack[w] = false;
                        comp.push(w);
                        if w == v {
                            break;
                        }
                    }
                    out.push(comp);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- worklist

thread_local! {
    /// Side-table slots whose value changed since the worklist last asked,
    /// with their value before the first change (collected only while
    /// `TRACK` is on: the worklist is running).
    static MOVED: RefCell<HashMap<u32, Option<Ty>>> = RefCell::new(HashMap::new());
    /// Methods that entered reference mode since the worklist last asked.
    static NEW_REC: RefCell<Vec<MethodKey>> = const { RefCell::new(Vec::new()) };
    static TRACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The worklist asks for moved slots and new reference-mode methods from
/// now on (or no longer).
pub(crate) fn track_moves(on: bool) {
    TRACK.with(|t| t.set(on));
}

/// A write changed `slot`; `old` is its value before this write. Only net
/// changes count: a slot written back and forth within one evaluation is
/// not a move.
fn note_moved(slot: u32, old: Option<&Ty>) {
    if TRACK.with(|t| t.get()) {
        MOVED.with(|m| {
            m.borrow_mut().entry(slot).or_insert_with(|| old.cloned());
        });
    }
}

fn note_new_rec(m: MethodKey) {
    if TRACK.with(|t| t.get()) {
        NEW_REC.with(|n| n.borrow_mut().push(m));
    }
}

/// Side-table slots whose value differs from before their first write since
/// the last call, in slot order.
pub(crate) fn take_moved() -> Vec<u32> {
    let firsts = MOVED.with(|m| std::mem::take(&mut *m.borrow_mut()));
    let mut v: Vec<u32> = ST.with(|s| {
        let s = s.borrow();
        firsts.into_iter().filter(|(slot, before)| s.values.get(slot) != before.as_ref()).map(|(k, _)| k).collect()
    });
    v.sort_unstable();
    v
}

/// Methods that entered reference mode since the last call.
pub(crate) fn take_new_rec() -> Vec<MethodKey> {
    NEW_REC.with(|n| std::mem::take(&mut *n.borrow_mut()))
}

/// The id of an already interned slot (no interning).
pub(crate) fn slot_id(key: &SlotKey) -> Option<u32> {
    ST.with(|s| s.borrow().ids.get(key).copied())
}

/// Includers whose registry copy of `module#method` reads the module's slot.
pub(crate) fn alias_includers(module: &ClassId, method: &Symbol) -> Vec<ClassId> {
    ST.with(|s| {
        s.borrow()
            .aliases
            .iter()
            .filter(|((_, m), src)| m == method && src == &module)
            .map(|((c, _), _)| c.clone())
            .collect()
    })
}

// ---------------------------------------------------------------- expansion

/// Expansion of every reference for the analyzer's consumers: references
/// to slots outside a cycle expand fully; a reference back to a slot
/// already being expanded (the back edge of a cycle) becomes `untyped`, or
/// with `RH_FOLD_TAIL` contributes nothing when it is unguarded.
pub(crate) struct Expander<'a> {
    classes: &'a HashMap<ClassId, ClassInfo>,
    memo: HashMap<u32, Ty>,
    stack: Vec<u32>,
    /// Constructor depth when each stack entry was pushed, and the current
    /// depth: a back edge met at the same depth is unguarded.
    depth_at: Vec<usize>,
    ctor: usize,
    cyclic: HashSet<u32>,
}

impl<'a> Expander<'a> {
    pub(crate) fn new(classes: &'a HashMap<ClassId, ClassInfo>) -> Self {
        Expander {
            classes,
            memo: HashMap::new(),
            stack: Vec::new(),
            depth_at: Vec::new(),
            ctor: 0,
            cyclic: HashSet::new(),
        }
    }

    pub(crate) fn expand(&mut self, t: &Ty) -> Ty {
        if !contains_rec(t) {
            return t.clone();
        }
        let mut budget = EXPAND_NODES;
        let r = self.go(t, &mut budget);
        // A slot made only of unguarded back edges (`X = X`) never returns;
        // keep main's `untyped` for it rather than `Bottom`.
        if matches!(r, Ty::Bottom) && !matches!(t, Ty::Bottom) { Ty::unresolved() } else { r }
    }

    fn go(&mut self, t: &Ty, budget: &mut usize) -> Ty {
        if *budget == 0 {
            if contains_rec(t) {
                note_expansion(2);
                return Ty::unresolved();
            }
            return t.clone();
        }
        *budget -= 1;
        match t {
            Ty::Rec { slot } => {
                if let Some(p) = self.stack.iter().position(|s| s == slot) {
                    for s in self.stack.iter().skip_while(|s| *s != slot) {
                        self.cyclic.insert(*s);
                    }
                    if *TAIL && self.depth_at[p] == self.ctor {
                        // Unguarded (`X = X | A`): its least solution adds
                        // nothing here.
                        note_expansion(1);
                        return Ty::Bottom;
                    }
                    note_expansion(0);
                    return Ty::unresolved();
                }
                if let Some(m) = self.memo.get(slot) {
                    return m.clone();
                }
                let Some(v) = value_of(*slot, self.classes) else { return Ty::pending_untyped() };
                self.stack.push(*slot);
                self.depth_at.push(self.ctor);
                let cyc_before = self.cyclic.len();
                let r = self.go(&v, budget);
                // A slot made only of unguarded back edges never returns a
                // value; keep main's `untyped` rather than leaking `Bottom`
                // (a `Bottom` receiver reads as known to the diagnostics).
                let r = if matches!(r, Ty::Bottom) && !matches!(v, Ty::Bottom) { Ty::unresolved() } else { r };
                self.stack.pop();
                self.depth_at.pop();
                // Only an expansion that met no back edge is context-free.
                if self.cyclic.len() == cyc_before && !self.cyclic.contains(slot) {
                    self.memo.insert(*slot, r.clone());
                }
                r
            }
            Ty::Union { variants } => {
                if *CANON_EXPAND && variants.iter().filter(|v| matches!(v, Ty::Rec { .. })).count() > 1 {
                    let order = reference_arm_order(variants);
                    let vs: Vec<Ty> = order.iter().map(|i| self.go(&variants[*i], budget)).collect();
                    return super::body::union_many(vs);
                }
                let vs: Vec<Ty> = variants.iter().map(|v| self.go(v, budget)).collect();
                super::body::union_many(vs)
            }
            other => {
                self.ctor += 1;
                let r = self.go_ctor(other, budget);
                self.ctor -= 1;
                r
            }
        }
    }

    fn go_ctor(&mut self, t: &Ty, budget: &mut usize) -> Ty {
        use std::sync::Arc;
        match t {
            Ty::Array { elem } => Ty::Array { elem: Arc::new(self.go(elem, budget)) },
            Ty::Hash { key, value } => {
                Ty::Hash { key: Arc::new(self.go(key, budget)), value: Arc::new(self.go(value, budget)) }
            }
            Ty::Tuple { elems } => Ty::Tuple { elems: elems.iter().map(|e| self.go(e, budget)).collect() },
            Ty::Record { row } => Ty::Record {
                row: crate::ty::Row {
                    fields: row.fields.iter().map(|(k, v)| (k.clone(), self.go(v, budget))).collect(),
                    rest: row.rest,
                },
            },
            Ty::Class { id, args } => {
                Ty::Class { id: id.clone(), args: args.iter().map(|a| self.go(a, budget)).collect() }
            }
            Ty::Fn { params, block, ret, effects } => Ty::Fn {
                params: params
                    .iter()
                    .map(|p| crate::ty::Param {
                        name: p.name.clone(),
                        ty: Arc::new(self.go(&p.ty, budget)),
                        kind: p.kind.clone(),
                    })
                    .collect(),
                block: block.as_ref().map(|b| Arc::new(self.go(b, budget))),
                ret: Arc::new(self.go(ret, budget)),
                effects: effects.clone(),
            },
            other => other.clone(),
        }
    }
}

fn note_expansion(kind: usize) {
    ST.with(|s| s.borrow_mut().expansion[kind] += 1);
}

/// Stop producing references: the final expansion has run.
pub(crate) fn deactivate() {
    ST.with(|s| s.borrow_mut().active = false);
}

/// Aggregates for the fixpoint canaries' stats line.
pub(crate) fn stats() -> serde_json::Value {
    ST.with(|s| {
        let s = s.borrow();
        let mut kinds = [0u64; 6];
        for k in &s.keys {
            kinds[match k {
                SlotKey::Ret { .. } => 0,
                SlotKey::Param { .. } => 1,
                SlotKey::Narrow { .. } => 2,
                SlotKey::At { .. } => 3,
                SlotKey::CopyRet { .. } => 0,
                SlotKey::CopyParam { .. } => 1,
                SlotKey::Site { role: SiteRole::Position(_), .. } => 3,
                SlotKey::Site { role: SiteRole::Narrow(_), .. } => 2,
                SlotKey::Site { role: SiteRole::ClosureResult, .. } => 4,
                SlotKey::Site { role: SiteRole::ClosureParam(_), .. } => 5,
            }] += 1;
        }
        let nodes: u64 = s.values.values().map(|t| {
            let mut n = 0u64;
            visit(t, &mut |_| n += 1);
            n
        }).sum();
        serde_json::json!({
            "methods_in_reference_mode": s.rec_methods.len(),
            "slots": {"ret": kinds[0], "param": kinds[1], "narrow": kinds[2], "position": kinds[3],
                      "closure_result": kinds[4], "closure_param": kinds[5]},
            "side_table_nodes": nodes,
            "expansion": {"back_edges": s.expansion[0], "tail_edges": s.expansion[1], "budget_cuts": s.expansion[2]},
            "accumulations_skipped": s.skipped,
            "late_alias_sites": s.late_aliases.len(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_return_slots_preserve_inferred_closures() {
        if !super::super::equations::on() { return; }
        reset(); super::super::equations::reset();
        let class = ClassId(Symbol::from("Factory"));
        let declared = Symbol::from("declared");
        let inferred = Symbol::from("inferred");
        let closure = Ty::Fn {
            params: vec![].into(), ret: std::sync::Arc::new(Ty::Str),
            block: None, effects: crate::effect::EffectSet::default(),
        };
        let mut info = ClassInfo::default();
        info.instance_methods.insert(declared.clone(), closure.clone());
        let mut classes = HashMap::from([(class.clone(), info)]);
        super::super::equations::capture_contracts(&classes);
        classes.get_mut(&class).unwrap().instance_methods.insert(inferred.clone(), closure.clone());
        let a = intern(SlotKey::Ret { class:class.clone(), method:declared, class_side:false });
        let b = intern(SlotKey::Ret { class:class.clone(), method:inferred.clone(), class_side:false });
        let c = intern(SlotKey::CopyRet { owner:ClassId(Symbol::from("Concern")),
            includer:class, method:inferred, class_side:false });
        assert_eq!(value_of(a, &classes), Some(Ty::Str));
        assert_eq!(value_of(b, &classes), Some(closure.clone()));
        assert_eq!(value_of(c, &classes), Some(closure));
        reset(); super::super::equations::reset();
    }

    #[test]
    fn structure_missing_slots_read_pending_and_keep_their_identity() {
        if !super::super::equations::on() { return; }
        reset();
        let key = SlotKey::Site {
            context: Default::default(), site:(1,2,3), role:SiteRole::ClosureResult,
        };
        let slot = intern(key.clone());
        assert_eq!(value_of(slot,&HashMap::new()),Some(super::super::body::unknown()));
        assert_eq!(intern(key.clone()),slot);
        assert_eq!(key_of(slot),Some(key));
        reset();
    }

    #[test]
    fn structure_scalar_children_have_position_slots_and_closures_have_results() {
        if !super::super::equations::on() { return; }
        reset(); freeze_structure();
        let site=(1,2,3);
        let opened=shallow(Ty::Array { elem:std::sync::Arc::new(Ty::Int) },site);
        let Ty::Array { elem } = opened else { unreachable!() };
        let Ty::Rec { slot } = *elem else { panic!("scalar child must keep a position identity") };
        assert_eq!(value_of(slot,&HashMap::new()),Some(Ty::Int));
        let result=closure_result(site,Ty::Str);
        assert_ne!(result,Ty::Rec { slot });
        let Ty::Rec { slot:result }=result else { unreachable!() };
        assert!(matches!(key_of(result),Some(SlotKey::Site { role:SiteRole::ClosureResult,.. })));
        assert_eq!(value_of(result,&HashMap::new()),Some(Ty::Str));
        // `head` promises an actual unfold. Returning Some for an inline
        // scalar would make dispatch's retry recurse without progress.
        assert_eq!(head(&Ty::Int,site,&HashMap::new()),None);
        reset();
    }

    #[test]
    fn structure_copy_parameters_distinguish_owner_host_and_side() {
        if !super::super::equations::on() { return; }
        reset();
        let host=ClassId(Symbol::from("Host"));
        let first=ClassId(Symbol::from("First"));
        let second=ClassId(Symbol::from("Second"));
        let method=Symbol::from("same_name");
        note_alias(&host,&method,&first,false);
        note_alias(&host,&method,&second,true);
        add_rec_methods(vec![(first.clone(),method.clone()),(second.clone(),method.clone())]);
        freeze_structure();
        let a=param_ref(&host,&method,false,0,Ty::Int);
        let b=param_ref(&host,&method,true,0,Ty::Str);
        assert_ne!(a,b);
        let Ty::Rec { slot:a }=a else { unreachable!() };
        let Ty::Rec { slot:b }=b else { unreachable!() };
        assert_eq!(key_of(a),Some(SlotKey::CopyParam { owner:first.clone(),includer:host.clone(),
            method:method.clone(),class_side:false,index:0 }));
        assert_eq!(value_of(a,&HashMap::new()),Some(Ty::Int));
        assert_eq!(value_of(b,&HashMap::new()),Some(Ty::Str));
        note_alias(&host,&method,&second,false);
        assert_eq!(alias_of_side(&host,&method,false),Some(first));
        assert_eq!(stats()["late_alias_sites"],1);
        reset();
    }

    fn union(v: Vec<Ty>) -> Ty {
        Ty::Union { variants: v.into() }
    }

    #[test]
    fn a_slot_does_not_contain_itself() {
        assert_eq!(strip_self(Ty::Rec { slot: 3 }, 3).provenance(), Some(crate::ty::Provenance::Pending));
        assert_eq!(strip_self(union(vec![Ty::Rec { slot: 3 }, Ty::Int]), 3), Ty::Int);
        assert_eq!(strip_self(Ty::Rec { slot: 4 }, 3), Ty::Rec { slot: 4 });
    }

    #[test]
    fn reference_arm_order_uses_keys_across_opposite_allocations() {
        let key = |name: &str| SlotKey::Ret { class: ClassId(Symbol::from("Order")), method: Symbol::from(name), class_side: false };
        ST.with(|s| *s.borrow_mut() = State::default());
        let z = intern(key("z")); let a = intern(key("a"));
        let first = vec![Ty::Rec { slot:z }, Ty::Int, Ty::Rec { slot:a }];
        let keys1: Vec<_> = reference_arm_order(&first).iter().map(|i| match &first[*i] {
            Ty::Rec { slot } => format!("{:?}", key_of(*slot)), _ => "value".into(),
        }).collect();
        ST.with(|s| *s.borrow_mut() = State::default());
        let a = intern(key("a")); let z = intern(key("z"));
        let second = vec![Ty::Rec { slot:a }, Ty::Rec { slot:z }, Ty::Int];
        let keys2: Vec<_> = reference_arm_order(&second).iter().map(|i| match &second[*i] {
            Ty::Rec { slot } => format!("{:?}", key_of(*slot)), _ => "value".into(),
        }).collect();
        assert_eq!(keys1, keys2);
        assert_eq!(keys1[0], "value");
        ST.with(|s| *s.borrow_mut() = State::default());
    }

    #[test]
    fn tarjan_finds_cycles_and_self_loops() {
        let succ = vec![vec![1], vec![0], vec![2], vec![]];
        let mut comps: Vec<Vec<usize>> = scc(&succ)
            .into_iter()
            .map(|mut c| {
                c.sort();
                c
            })
            .collect();
        comps.sort();
        assert_eq!(comps, vec![vec![0, 1], vec![2], vec![3]]);
    }
}

/// Canonical structure records; inferred values and allocation ids are absent.
pub(super) fn structure_parts() -> (Vec<SlotKey>, Vec<String>, Vec<String>) {
    ST.with(|s| {
        let s = s.borrow();
        let refs = s.rec_methods.iter().map(|(c, m)| format!("{}#{m}", c.0)).collect();
        let mut routing = Vec::new();
        for ((c, m), module) in &s.aliases { routing.push(format!("alias:{}#{m}->{}#{m}", c.0, module.0)); }
        for ((c,m,side),module) in &s.static_aliases {
            routing.push(format!("alias:{}:{side}#{m}->{}:{side}#{m}",c.0,module.0));
        }
        for ((c, m), targets) in &s.edges {
            for (t, n) in targets { routing.push(format!("edge:{}#{m}->{}#{n}", c.0, t.0)); }
        }
        (s.keys.clone(), refs, routing)
    })
}

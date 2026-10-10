//! prototype research prototype (origin-aware inference): fold by origin. Env-gated and
//! inert by default: with `RH_FOLD` unset nothing here runs and no
//! `Ty::Rec` is ever built.
//!
//! In a recursive component (the methods of a non-trivial strongly connected
//! component of the resolved call graph), reading a slot (a method's return,
//! or one of its parameter rows) yields a reference `Ty::Rec { slot }`
//! instead of a copy of the slot's current tree. Sites that need structure
//! unfold a reference one level (`head`); every other site treats it as
//! `Untyped`. At the end of analysis every reference is expanded
//! (`expand`): references to slots outside a cycle fully, cyclic ones down
//! to the back edge, which becomes `Untyped`.
//!
//! - `RH_FOLD=1`: turn the fold on.
//! - `RH_FOLD_SITES=dispatch,proj,block,merge,narrow,destruct,unify` (default
//!   all; `none` is dial zero, M1 with an origin certificate): the sites that
//!   unfold. A site that is off sees `Untyped`.
//! - `RH_FOLD_LEVEL=1|2` (default 2): unfold by copying one level, or by
//!   position references (`Rec(slot, path)`, moonshot-fable's level 2).
//! - `RH_FOLD_JOIN=1`: handoffs of reference-mode slots join monotonically
//!   (returns: new = old | body; parameter rows: new = old | sites).
//! - `RH_FOLD_PRINT=1`: print each reference-mode slot's grammar as RBS-style
//!   aliases. Prints class and method names: public reproductions only.
//! - `RH_FOLD_QUIET=1`: no probe lines (for suites that compare stderr).
//! - `RH_BRK_ALLARMS=1`, `RH_BRK_NOABSORB=1`: monotone typer rules
//!   (block parameters over every union arm; no absorption of answered arms).
//!
//! Probe lines on stderr start with `rh-fold:` and carry aggregates only.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use super::body::ClassInfo;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub(crate) static ON: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD").is_ok_and(|s| s == "1"));
pub(crate) static JOIN: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_JOIN").is_ok_and(|s| s == "1"));
/// `RH_FOLD_LEVEL=1|2` (default 2). Level 1: unfolding a reference copies
/// one level of the slot's term. Level 2: it yields the term's top level
/// with every compound child a reference to that position,
/// `Rec(At { site, step })`, so a projection is a reference too.
pub(crate) static LEVEL: LazyLock<u8> = LazyLock::new(|| {
    std::env::var("RH_FOLD_LEVEL").ok().and_then(|s| s.parse().ok()).unwrap_or(2)
});
pub(crate) static PRINT: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_PRINT").is_ok_and(|s| s == "1"));
/// two monotone typer rules (experiment), carried here behind their own
/// names so the private run can combine them with the fold; off by default.
pub(crate) static BRK_ALLARMS: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_BRK_ALLARMS").is_ok_and(|s| s == "1"));
pub(crate) static BRK_NOABSORB: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_BRK_NOABSORB").is_ok_and(|s| s == "1"));
/// `RH_FOLD_QUIET=1`: no `rh-fold:` lines (test suites that compare stderr).
pub(crate) static QUIET: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_QUIET").is_ok_and(|s| s == "1"));
/// `RH_FOLD_PATHSEEN=1` restores round 1's path-based `resolve`
/// (exponential on rings of top-level references; ablation only).
static PATHSEEN: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_PATHSEEN").is_ok_and(|s| s == "1"));
/// prototype emission policy, `RH_FOLD_TAIL=1`: at the final expansion an
/// unguarded back edge (`X = X | A`, a tail call) contributes nothing, its
/// least solution, instead of `untyped`; a guarded one stays `untyped`.
pub(crate) static TAIL: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_TAIL").is_ok_and(|s| s == "1"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Site {
    /// The receiver of a send that is none of the kinds below.
    Dispatch = 0,
    /// Element and value projection: `[]`, `dig`, `fetch`, `first`, ...
    Proj = 1,
    /// A send with a block: its parameters read the receiver's structure.
    Block = 2,
    /// `merge` and its family.
    Merge = 3,
    /// `is_a?`, `nil?` and truthiness narrowing of a binding.
    Narrow = 4,
    /// Multiple assignment from a reference.
    Destruct = 5,
    /// The receiver classes a call site reaches, for parameter unification.
    Unify = 6,
}
const NSITES: usize = 7;
const SITE_NAMES: [&str; NSITES] = ["dispatch", "proj", "block", "merge", "narrow", "destruct", "unify"];

static SITES: LazyLock<u32> = LazyLock::new(|| {
    let Ok(spec) = std::env::var("RH_FOLD_SITES") else { return u32::MAX };
    let mut mask = 0u32;
    for part in spec.split(',').map(str::trim) {
        match part {
            "all" => mask = u32::MAX,
            "none" | "" => {}
            name => {
                if let Some(i) = SITE_NAMES.iter().position(|n| *n == name) {
                    mask |= 1 << i;
                } else if !std::env::var("RH_FOLD_QUIET").is_ok_and(|s| s == "1") {
                    eprintln!("rh-fold: unknown site {name:?} ignored");
                }
            }
        }
    }
    mask
});

pub(crate) fn site_on(site: Site) -> bool {
    *SITES & (1 << site as u32) != 0
}

/// The site a send's receiver is read at.
pub(crate) fn send_site(method: &str, has_block: bool) -> Site {
    match method {
        "merge" | "merge!" | "deep_merge" | "deep_merge!" | "reverse_merge" | "reverse_merge!"
        | "update" | "with_defaults" => Site::Merge,
        "[]" | "dig" | "fetch" | "values_at" | "first" | "last" | "values" | "keys" | "to_a"
        | "sample" | "min" | "max" | "flatten" | "compact" | "slice" | "except" => Site::Proj,
        _ if has_block => Site::Block,
        _ => Site::Dispatch,
    }
}

/// A syntax site: the span of the expression at which a reference was
/// unfolded or narrowed. Positions and narrowings are keyed by site, so
/// the set of references is finite by construction (Heintze's set
/// variables per expression), whatever paths the program takes.
pub(crate) type SiteId = (u32, u32, u32);

pub(crate) fn site_of(span: &crate::span::Span) -> SiteId {
    (span.file.0, span.start, span.end)
}

/// A site for an unfold with no expression at hand (a dispatch reached
/// from outside a send); keyed by the method name.
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
    /// Level 2: the position `step` below every reference unfolded at
    /// `site`; its value is the union of those children.
    At { site: SiteId, step: Step },
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

impl Step {
    fn name(&self) -> String {
        match self {
            Step::Elem => "elem".into(),
            Step::Key => "key".into(),
            Step::Val => "val".into(),
            Step::Idx(i) => format!("t{i}"),
            Step::Field(f) => format!("f_{}", f.as_str()),
            Step::Arg(i) => format!("a{i}"),
        }
    }
}

type MethodKey = (ClassId, Symbol);

#[derive(Default)]
struct State {
    /// References are produced only while active: set once the first
    /// recursive component is known, cleared by the final expansion.
    active: bool,
    keys: Vec<SlotKey>,
    ids: HashMap<SlotKey, u32>,
    /// Methods in a non-trivial SCC of the call graph. Monotone.
    rec_methods: HashSet<MethodKey>,
    /// Call graph of the current unify pass.
    edges: HashMap<MethodKey, HashSet<MethodKey>>,
    /// Current value of each parameter slot in reference mode, as the
    /// body that reads it was seeded (sites, defaults, declarations).
    param_values: HashMap<u32, Ty>,
    /// Previous parameter rows of reference-mode methods (`RH_FOLD_JOIN`).
    prev_params: HashMap<MethodKey, Vec<Ty>>,
    /// Previous returns of reference-mode methods (`RH_FOLD_JOIN`).
    prev_rets: HashMap<(ClassId, Symbol, bool), Ty>,
    /// Typing-pass counter, and the pass of each site slot's last write.
    epoch: u64,
    site_epoch: HashMap<u32, u64>,
    unfold_on: [u64; NSITES],
    unfold_off: [u64; NSITES],
    refs_ret: u64,
    refs_param: u64,
    refs_narrowed: u64,
    counts: BTreeMap<&'static str, u64>,
    /// Per send span: did its last typing unfold the receiver and fail?
    exposed: HashMap<(u32, u32, u32), bool>,
    loops: Vec<serde_json::Value>,
    /// registry copies. `fold_concern_surfaces` copies a
    /// concern's entries onto every includer each round, and dispatch on
    /// the includer finds the copy first. A copy of a reference-mode
    /// return must be read as a reference to the module's slot, or it is
    /// read by value and the concern re-embeds its own previous return
    /// (the private app's absorb loop doubled every round through these).
    aliases: HashMap<MethodKey, ClassId>,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

#[inline]
pub(crate) fn on() -> bool {
    *ON
}

#[inline]
pub(crate) fn active() -> bool {
    *ON && ST.with(|s| s.borrow().active)
}

pub(crate) fn count_n(name: &'static str, n: u64) {
    if *ON {
        ST.with(|s| *s.borrow_mut().counts.entry(name).or_default() += n);
    }
}

pub(crate) fn count(name: &'static str) {
    if *ON {
        ST.with(|s| *s.borrow_mut().counts.entry(name).or_default() += 1);
    }
}

fn intern(key: SlotKey) -> u32 {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(id) = s.ids.get(&key) {
            return *id;
        }
        let id = s.keys.len() as u32;
        s.keys.push(key.clone());
        s.ids.insert(key, id);
        id
    })
}

pub(crate) fn key_of(slot: u32) -> Option<SlotKey> {
    ST.with(|s| s.borrow().keys.get(slot as usize).cloned())
}

pub(crate) fn is_rec_method(class: &ClassId, method: &Symbol) -> bool {
    ST.with(|s| {
        let s = s.borrow();
        let key = (class.clone(), method.clone());
        s.rec_methods.contains(&key)
            || (*ALIASES && s.aliases.get(&key).is_some_and(|src| s.rec_methods.contains(&(src.clone(), method.clone()))))
    })
}

/// `RH_FOLD_ALIASES=0` reads registry copies by value again
/// (ablation of the concern-copy fix).
static ALIASES: LazyLock<bool> = LazyLock::new(|| !std::env::var("RH_FOLD_ALIASES").is_ok_and(|s| s == "0"));

/// `fold_concern_surfaces` copied `module#method` onto `class`.
pub(crate) fn note_alias(class: &ClassId, method: &Symbol, module: &ClassId) {
    if !*ON && !super::slots::on() {
        return;
    }
    ST.with(|s| {
        s.borrow_mut().aliases.insert((class.clone(), method.clone()), module.clone());
    });
}

/// The module a registry copy on `class` came from, if any.
pub(crate) fn alias_of(class: &ClassId, method: &Symbol) -> Option<ClassId> {
    ST.with(|s| s.borrow().aliases.get(&(class.clone(), method.clone())).cloned())
}

/// A read of a return slot, when it is in reference mode.
pub(crate) fn ret_ref(class: &ClassId, method: &Symbol, class_side: bool) -> Option<Ty> {
    if !active() || !is_rec_method(class, method) {
        return None;
    }
    // A registry copy reads the defining module's slot.
    let class = &ST.with(|s| {
        let s = s.borrow();
        let key = (class.clone(), method.clone());
        if s.rec_methods.contains(&key) {
            class.clone()
        } else {
            s.aliases.get(&key).cloned().unwrap_or_else(|| class.clone())
        }
    });
    let slot = intern(SlotKey::Ret { class: class.clone(), method: method.clone(), class_side });
    ST.with(|s| s.borrow_mut().refs_ret += 1);
    Some(Ty::Rec { slot })
}

/// A read of a parameter slot, when it is in reference mode. `value` is
/// what the body would otherwise have been seeded with.
pub(crate) fn param_ref(class: &ClassId, method: &Symbol, index: usize, value: Ty) -> Ty {
    if !active() || !is_rec_method(class, method) {
        return value;
    }
    let slot = intern(SlotKey::Param { class: class.clone(), method: method.clone(), index });
    // A slot's own top-level reference contributes nothing (X = X | A is A).
    let value = strip_self(value, slot);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        s.refs_param += 1;
        // every typing that seeds this parameter flows into
        // one set variable (two bodies can share the key, and Pass A and the
        // reseeded pass both seed it); join instead of last-write-wins.
        let value = match s.param_values.get(&slot) {
            Some(old) if *PENDJOIN && *JOIN => join_slot(old.clone(), value),
            _ => value,
        };
        if s.param_values.get(&slot) != Some(&value) {
            note_moved(slot, s.param_values.get(&slot));
        }
        s.param_values.insert(slot, value);
    });
    Ty::Rec { slot }
}

fn strip_self(t: Ty, slot: u32) -> Ty {
    match t {
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Rec { slot: s } if *s == slot)) => {
            let kept: Vec<Ty> =
                variants.into_iter().filter(|v| !matches!(v, Ty::Rec { slot: s } if *s == slot)).collect();
            match kept.len() {
                0 => Ty::Untyped,
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept.into() },
            }
        }
        Ty::Rec { slot: s } if s == slot => Ty::Untyped,
        other => other,
    }
}

/// The current value of a slot: the registry entry for a return, the
/// side table for a parameter.
pub(crate) fn value_of(slot: u32, classes: &HashMap<ClassId, ClassInfo>) -> Option<Ty> {
    // a typing that reads a slot's value depends on it; the
    // query engine re-types the reader when the value moves.
    super::sccq::rec_fold_slot(slot);
    match key_of(slot)? {
        SlotKey::Ret { class, method, class_side } => {
            let cls = classes.get(&class)?;
            let table = if class_side { &cls.class_methods } else { &cls.instance_methods };
            let t = table.get(&method)?;
            Some(match t {
                Ty::Fn { ret, .. } => (**ret).clone(),
                other => other.clone(),
            })
        }
        SlotKey::Param { .. } | SlotKey::Narrow { .. } | SlotKey::At { .. } => {
            ST.with(|s| s.borrow().param_values.get(&slot).cloned())
        }
    }
}

/// Accumulate `value` into a site-keyed slot (its set variable) and
/// return the reference to it. Within one typing pass a site's values
/// join; the first write in a new pass replaces last pass's value, so
/// placeholders from early rounds wash out (with `RH_FOLD_JOIN` they
/// accumulate across passes too: every step monotone).
fn accumulate(key: SlotKey, value: Ty) -> Ty {
    let slot = intern(key);
    let value = strip_self(value, slot);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let epoch = s.epoch;
        let fresh = s.site_epoch.get(&slot).is_none_or(|e| *e < epoch);
        // an arm the slot already holds changes nothing.
        if *PENDJOIN && (!fresh || (*JOIN && !*RESTART)) {
            if let Some(old) = s.param_values.get(&slot) {
                if contains_arm(old, &value) {
                    s.site_epoch.insert(slot, epoch);
                    return;
                }
            }
        }
        let value_dbg = if *DEBUG_SLOT { Some(value.clone()) } else { None };
        let joined = match s.param_values.get(&slot) {
            // a site slot is shared by every typing of its
            // body (a concern method typed as a module unit and spliced into
            // controllers keeps its spans). Two pending values must join
            // order-independently; `join` keeps the last one, so two units
            // writing `Var` and `Var | untyped` ping-pong forever.
            Some(old) if (!fresh || (*JOIN && !*RESTART)) && *PENDJOIN => join_slot(old.clone(), value),
            Some(old) if !fresh || (*JOIN && !*RESTART) => join(old.clone(), value),
            _ => value,
        };
        s.site_epoch.insert(slot, epoch);
        if s.param_values.get(&slot) != Some(&joined) {
            note_moved(slot, s.param_values.get(&slot));
            // Public inputs only (`RH_C1_DEBUG_SLOT=<key substring>`).
            if let Some(value) = value_dbg {
                if std::env::var("RH_C1_DEBUG_SLOT").is_ok_and(|v| format!("{:?}", s.keys[slot as usize]).contains(&v)) {
                    eprintln!("c1-debug-slot {slot} old={:?} value={:?} joined={:?}", s.param_values.get(&slot).map(|t| format!("{t:?}").chars().take(400).collect::<String>()), format!("{value:?}").chars().take(400).collect::<String>(), format!("{joined:?}").chars().take(400).collect::<String>());
                }
            }
        }
        s.param_values.insert(slot, joined);
    });
    Ty::Rec { slot }
}

/// `RH_C1_PENDJOIN=1|0`: pending values at a site slot join by
/// `union_of` (commutative) instead of last-write-wins. Default: on with the
/// query engine (`RH_SCHED=sccq`), off otherwise (fold2's semantics).
pub(crate) static PENDJOIN: LazyLock<bool> = LazyLock::new(|| match std::env::var("RH_C1_PENDJOIN").as_deref() {
    Ok("1") => true,
    Ok("0") => false,
    _ => std::env::var("RH_SCHED").is_ok_and(|v| v == "sccq"),
});

/// `RH_C1_JOINORDER=1|0`: main's harvest joins reference-mode
/// returns before the registry copies as well as after, the order the query
/// engine applies per unit. Default: on with `RH_SCHED=sccq`.
static JOINORDER: LazyLock<bool> = LazyLock::new(|| match std::env::var("RH_C1_JOINORDER").as_deref() {
    Ok("1") => true,
    Ok("0") => false,
    _ => std::env::var("RH_SCHED").is_ok_and(|v| v == "sccq"),
});

pub(crate) fn join_order() -> bool {
    *JOINORDER
}

/// `RH_C1_SORTED=1|0`: folds that apply non-commutative
/// steps iterate classes in name order, not hash-map order (repeatable
/// runs). Default: on with `RH_SCHED=sccq`.
static SORTED: LazyLock<bool> = LazyLock::new(|| match std::env::var("RH_C1_SORTED").as_deref() {
    Ok("1") => true,
    Ok("0") => false,
    _ => std::env::var("RH_SCHED").is_ok_and(|v| v == "sccq"),
});

pub(crate) fn sorted_order() -> bool {
    *SORTED
}

/// `RH_FOLD_SITES_RESTART=1` keeps round 1's per-pass restart of
/// site sets (`At`, `Narrow`) even under `RH_FOLD_JOIN`: they are derived
/// state, recomputed each pass from the joined handoffs, so placeholders
/// from early passes wash out. Only returns and parameter rows join.
static RESTART: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_SITES_RESTART").is_ok_and(|s| s == "1"));
/// `RH_FOLD_SIDECHECK=0` drops the side table from the loop's
/// convergence test (ablation; round 1 never checked it).
static SIDECHECK: LazyLock<bool> =
    LazyLock::new(|| !std::env::var("RH_FOLD_SIDECHECK").is_ok_and(|s| s == "0"));

thread_local! {
    static SIDE_FP: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// is the fold's side table (parameter, position and narrowing
/// slots) unchanged since the last call? A return or row that holds a
/// reference compares equal by slot id, so the registry alone cannot see
/// a referenced value move; a loop has reached a true fixpoint only when
/// this is stable too. Call once per round, at the convergence test.
pub(crate) fn side_stable() -> bool {
    if !active() || !*SIDECHECK {
        return true;
    }
    use std::hash::{Hash, Hasher};
    let fp = ST.with(|s| {
        let s = s.borrow();
        let mut keys: Vec<&u32> = s.param_values.keys().collect();
        keys.sort_unstable();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for k in keys {
            k.hash(&mut h);
            format!("{:?}", s.param_values[k]).hash(&mut h);
        }
        h.finish()
    });
    let prev = SIDE_FP.with(|c| c.replace(Some(fp)));
    let stable = prev == Some(fp);
    if !stable {
        count("side_table_moved");
    }
    stable
}

/// retype every class this round because the side table moved
/// (`RH_FOLD_SIDEDIRTY=0` keeps the plain dirty frontier; ablation).
pub(crate) fn side_full_retype(side_stable: bool) -> bool {
    static ON_: LazyLock<bool> = LazyLock::new(|| !std::env::var("RH_FOLD_SIDEDIRTY").is_ok_and(|s| s == "0"));
    active() && *SIDECHECK && *ON_ && !side_stable
}

/// The side table's fingerprint, without recording it (prototype verify).
pub(crate) fn side_fp_peek() -> u64 {
    use std::hash::{Hash, Hasher};
    ST.with(|s| {
        let s = s.borrow();
        let mut keys: Vec<&u32> = s.param_values.keys().collect();
        keys.sort_unstable();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for k in keys {
            k.hash(&mut h);
            format!("{:?}", s.param_values[k]).hash(&mut h);
        }
        h.finish()
    })
}

/// prototype (`RH_FOLD_VERIFY=1`): the result of one extra round after a
/// loop converged (prototype 1940Z: hidden inputs such as stamps, typing
/// sequences and harvest history are not in the convergence test, so a
/// converged loop is only a true fixpoint if one more round moves nothing).
pub(crate) fn note_verify(stage: &str, moved: u64, side_moved: bool) {
    if !*QUIET {
        let line = serde_json::json!({"verify": stage, "moved": moved, "side_moved": side_moved});
        eprintln!("rh-fold: {line}");
        ST.with(|s| s.borrow_mut().loops.push(line));
    }
}

pub(crate) static VERIFY: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_VERIFY").is_ok_and(|s| s == "1"));

/// A new typing pass begins (site sets restart on their first write).
pub(crate) fn new_epoch() {
    if *ON {
        ST.with(|s| s.borrow_mut().epoch += 1);
    }
}

/// The arms a reference denotes: its term's non-union, non-reference
/// arms, following unions and references (each slot once; an unguarded
/// cycle contributes nothing, the least solution of `X = X | ...`).
pub(crate) fn resolve(slot: u32, classes: &HashMap<ClassId, ClassInfo>, seen: &mut HashSet<u32>) -> Vec<Ty> {
    let mut out = Vec::new();
    if !seen.insert(slot) {
        return out;
    }
    if let Some(v) = value_of(slot, classes) {
        flatten_into(&v, classes, seen, &mut out);
    }
    // `seen` is a visited set for the whole top-level call,
    // not a path set. Removing the slot again re-expanded it once per simple
    // path (Fibonacci on rings of top-level references); every arm it
    // contributes is already in the caller's output after the first visit.
    if *PATHSEEN {
        seen.remove(&slot);
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

/// Level 2: `arm` with every child that is not a scalar leaf replaced by a
/// reference to its position at `site`, whose set variable accumulates
/// the child. Scalar leaves and unknowns stay in place.
fn shallow(arm: Ty, site: SiteId) -> Ty {
    fn keep(t: &Ty) -> bool {
        matches!(
            t,
            Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Sym | Ty::Date | Ty::Time | Ty::Nil
                | Ty::Var { .. } | Ty::Untyped | Ty::Bottom | Ty::Relation { .. } | Ty::SelfInstance
        ) || matches!(t, Ty::Class { args, .. } if args.is_empty())
    }
    let pos = |child: Ty, step: Step| -> Ty {
        if keep(&child) {
            child
        } else {
            accumulate(SlotKey::At { site, step }, child)
        }
    };
    match arm {
        Ty::Array { elem } => Ty::Array { elem: std::sync::Arc::new(pos(std::sync::Arc::unwrap_or_clone(elem), Step::Elem)) },
        Ty::Hash { key, value } => {
            Ty::Hash {
                key: std::sync::Arc::new(pos(std::sync::Arc::unwrap_or_clone(key), Step::Key)),
                value: std::sync::Arc::new(pos(std::sync::Arc::unwrap_or_clone(value), Step::Val)),
            }
        }
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems.into_iter().enumerate().map(|(i, e)| pos(e, Step::Idx(i as u32))).collect(),
        },
        Ty::Record { row } => Ty::Record {
            row: crate::ty::Row {
                fields: row.fields.into_iter().map(|(k, v)| {
                    let step = Step::Field(k.clone());
                    (k, pos(v, step))
                }).collect(),
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

/// Receiver classes a call site reaches through references (the unify
/// site), each slot visited once: linear in the reference graph
/// (ring critique of the path-expanding first version).
pub(crate) fn receiver_classes(t: &Ty, classes: &HashMap<ClassId, ClassInfo>) -> Option<Vec<ClassId>> {
    if !*ON || !contains_rec(t) {
        return None;
    }
    let on = site_on(Site::Unify);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if on { s.unfold_on[Site::Unify as usize] += 1 } else { s.unfold_off[Site::Unify as usize] += 1 }
    });
    let mut out: Vec<ClassId> = Vec::new();
    if !on {
        return Some(out);
    }
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
pub(crate) fn narrowed_ref(site: SiteId, filter: String, value: Ty) -> Ty {
    ST.with(|s| s.borrow_mut().refs_narrowed += 1);
    accumulate(SlotKey::Narrow { site, filter }, value)
}

pub(crate) fn has_top_rec(t: &Ty) -> bool {
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

pub(crate) fn visit_pub(t: &Ty, f: &mut dyn FnMut(&Ty)) {
    visit(t, f)
}

fn visit(t: &Ty, f: &mut dyn FnMut(&Ty)) {
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

/// Unfold the top-level references of `t` by one level, at `site`. `None`
/// when `t` has none. A site that is off sees `Untyped` in their place. A
/// reference reached again through top-level arms only (an unguarded
/// cycle, `X = Y | A; Y = X | B`) contributes nothing.
pub(crate) fn head(t: &Ty, site: Site, at: SiteId, classes: &HashMap<ClassId, ClassInfo>) -> Option<Ty> {
    if !*ON || !has_top_rec(t) {
        return None;
    }
    let on = site_on(site);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if on { s.unfold_on[site as usize] += 1 } else { s.unfold_off[site as usize] += 1 }
    });
    let mut out: Vec<Ty> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    push_head(t, on, at, classes, &mut seen, &mut out);
    Some(match out.len() {
        0 => Ty::Untyped,
        _ => super::body::union_many(out),
    })
}

fn push_head(t: &Ty, on: bool, at: SiteId, classes: &HashMap<ClassId, ClassInfo>, seen: &mut HashSet<u32>, out: &mut Vec<Ty>) {
    match t {
        Ty::Rec { slot } if on && *LEVEL >= 2 => {
            // Level 2: the arms the reference denotes, children by position
            // at this site.
            let arms = resolve(*slot, classes, &mut HashSet::new());
            if arms.is_empty() {
                count("unfold_empty");
                out.push(Ty::Untyped);
            }
            for arm in arms {
                out.push(shallow(arm, at));
            }
        }
        Ty::Rec { slot } => {
            if !on {
                out.push(Ty::Untyped);
            } else if seen.insert(*slot) {
                match value_of(*slot, classes) {
                    Some(v) => push_head(&v, on, at, classes, seen, out),
                    None => {
                        count("unfold_empty");
                        out.push(Ty::Untyped)
                    }
                }
            }
        }
        Ty::Union { variants } => variants.iter().for_each(|v| push_head(v, on, at, classes, seen, out)),
        other => out.push(other.clone()),
    }
}

/// Record that typing a send at `span` unfolded its receiver and whether
/// dispatch then failed (`Var`): a precision-exposed candidate.
pub(crate) fn note_send(span: &crate::span::Span, unfolded: bool, result: &Ty) {
    if !*ON {
        return;
    }
    let key = (span.file.0, span.start, span.end);
    let failed = unfolded && matches!(result, Ty::Var { .. });
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if failed {
            s.exposed.insert(key, true);
        } else {
            s.exposed.remove(&key);
        }
    });
}

pub(crate) fn exposed(span: &crate::span::Span) -> bool {
    if !*ON {
        return false;
    }
    ST.with(|s| s.borrow().exposed.get(&(span.file.0, span.start, span.end)).copied().unwrap_or(false))
}

/// prototype emission policy, `RH_FOLD_EXPOSED_WARN=1`: a precision-exposed
/// error (the receiver was unfolded from a reference and dispatch then
/// failed on every arm; counterexample: these are true positives) and its
/// follow-on `ivar_unresolved` are reported as warnings, so they no longer
/// block `--target` emission.
pub(crate) static EXPOSED_WARN: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_EXPOSED_WARN").is_ok_and(|s| s == "1"));

thread_local! {
    static EXPOSED_IVARS: RefCell<HashSet<(u32, Symbol)>> = RefCell::new(HashSet::new());
}

pub(crate) fn note_exposed_ivar(span: &crate::span::Span, name: &Symbol) {
    EXPOSED_IVARS.with(|s| s.borrow_mut().insert((span.file.0, name.clone())));
}

/// The diagnostic at `span` is precision exposed (`ivar` names the ivar of
/// an `ivar_unresolved`), and the policy flag is on.
pub(crate) fn exposed_as_warning(span: &crate::span::Span, ivar: Option<&Symbol>) -> bool {
    if !*ON || !*EXPOSED_WARN {
        return false;
    }
    match ivar {
        None => exposed(span),
        Some(n) => EXPOSED_IVARS.with(|s| s.borrow().contains(&(span.file.0, n.clone()))),
    }
}

// ---------------------------------------------------------------- call graph

pub(crate) fn begin_calls() {
    if *ON {
        ST.with(|s| s.borrow_mut().edges.clear());
    }
}

pub(crate) fn record_call(caller: &ClassId, method: &Symbol, callee: MethodKey) {
    ST.with(|s| {
        s.borrow_mut().edges.entry((caller.clone(), method.clone())).or_default().insert(callee);
    });
}

/// Close a unify pass: every method in a non-trivial SCC of the call graph
/// joins the reference-mode set, which only grows.
pub(crate) fn end_calls() {
    if !*ON {
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let sccs = tarjan(&s.edges);
        let before = s.rec_methods.len();
        for comp in sccs {
            let nontrivial = comp.len() > 1
                || s.edges.get(&comp[0]).is_some_and(|out| out.contains(&comp[0]));
            if nontrivial {
                for m in comp {
                    if s.rec_methods.insert(m.clone()) {
                        note_new_rec(m);
                    }
                }
            }
        }
        if s.rec_methods.len() > before && !s.active {
            s.active = true;
        }
        let grew = (s.rec_methods.len() - before) as u64;
        *s.counts.entry("scc_methods_added").or_default() += grew;
    });
}

/// prototype (RH_FOLD_SLOTS): methods the slot-read graph puts into
/// reference mode (a monotone set, like the call graph's).
pub(crate) fn add_rec_methods(methods: Vec<MethodKey>) {
    if !*ON || methods.is_empty() {
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let before = s.rec_methods.len();
        for m in methods {
            if s.rec_methods.insert(m.clone()) {
                note_new_rec(m);
            }
        }
        let grew = (s.rec_methods.len() - before) as u64;
        if grew > 0 {
            s.active = true;
        }
        *s.counts.entry("slot_graph_methods_added").or_default() += grew;
    });
}

pub(crate) fn scc_pub(succ: &[Vec<usize>]) -> Vec<Vec<usize>> {
    scc(succ)
}

/// The direct children of a type.
pub(crate) fn visit_children(t: &Ty, f: &mut dyn FnMut(&Ty)) {
    match t {
        Ty::Array { elem } => f(elem),
        Ty::Hash { key, value } => {
            f(key);
            f(value)
        }
        Ty::Tuple { elems } | Ty::Union { variants: elems } => elems.iter().for_each(|e| f(e)),
        Ty::Record { row } => row.fields.values().for_each(|v| f(v)),
        Ty::Class { args, .. } => args.iter().for_each(|a| f(a)),
        Ty::Fn { params, block, ret, .. } => {
            params.iter().for_each(|p| f(&p.ty));
            if let Some(b) = block {
                f(b)
            }
            f(ret)
        }
        _ => {}
    }
}

fn tarjan(edges: &HashMap<MethodKey, HashSet<MethodKey>>) -> Vec<Vec<MethodKey>> {
    // Deterministic node order.
    let mut nodes: Vec<&MethodKey> = edges.keys().chain(edges.values().flatten()).collect();
    nodes.sort_by(|a, b| (a.0 .0.as_str(), a.1.as_str()).cmp(&(b.0 .0.as_str(), b.1.as_str())));
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

/// Iterative Tarjan: the strongly connected components of a graph given
/// as successor lists.
fn scc(succ: &[Vec<usize>]) -> Vec<Vec<usize>> {
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

// ---------------------------------------------------------------- handoffs

/// `RH_FOLD_JOIN`: join a reference-mode method's parameter row with the
/// row it had last round.
pub(crate) fn join_params(params: &mut HashMap<(ClassId, Symbol), Vec<Ty>>) {
    if !active() {
        return;
    }
    if !*JOIN {
        // Transfer-risk count only: a rebuilt row that does not cover
        // last round's row is a non-monotone step.
        ST.with(|s| {
            let mut s = s.borrow_mut();
            let rec: Vec<MethodKey> = s.rec_methods.iter().cloned().collect();
            for key in rec {
                let cur = params.get(&key).cloned().unwrap_or_default();
                if let Some(prev) = s.prev_params.get(&key) {
                    let shrank = prev.iter().enumerate().any(|(i, old)| match cur.get(i) {
                        Some(new) => old != new && super::body::union_of(old.clone(), new.clone()) != *new,
                        None => !old.is_unknown(),
                    });
                    if shrank {
                        *s.counts.entry("param_nonmonotone").or_default() += 1;
                    }
                }
                s.prev_params.insert(key, cur);
            }
        });
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let rec: Vec<MethodKey> = s.rec_methods.iter().cloned().collect();
        for key in rec {
            let prev = s.prev_params.get(&key).cloned();
            let row = params.entry(key.clone()).or_default();
            if let Some(prev) = prev {
                if row.len() < prev.len() {
                    row.resize(prev.len(), Ty::Var { var: crate::ident::TyVar(0) });
                }
                for (slot, old) in row.iter_mut().zip(prev) {
                    let joined = join_handoff(old, slot.clone());
                    if &joined != slot {
                        *s.counts.entry("join_param_widened").or_default() += 1;
                    }
                    *slot = joined;
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

/// The handoff join: pending `Var`/`Untyped` arms of either side yield to
/// the other side's informative core (#521's rule), everything else joins.
pub(crate) fn join(old: Ty, new: Ty) -> Ty {
    if old == new {
        return new;
    }
    // a value with no informative core (`untyped | Var`, not only
    // the bare forms) is pending on either side, as the harvest's own rule
    // already says (`has_informative_core`). Round 1 tested `is_unknown`
    // only, so `X` joined with `untyped | Var` kept the `untyped` arm, and
    // the next join stripped it again: a period-2 flip inside reference
    // mode (Discourse, `filter_allowed_tags`).
    if old.is_unknown() || (*FIXJOIN && !informative(&old)) {
        return new;
    }
    if new.is_unknown() || (*FIXJOIN && !informative(&new)) {
        return old;
    }
    if *GRADUAL {
        // Split roles (`RH_FOLD_GRADUAL=1`): `Var` (pending) arms yield,
        // `untyped` (gradual) arms are kept, so the join is monotone.
        return super::body::union_of(strip_var(old), strip_var(new));
    }
    let old = if old.has_unknown_arm() { old.strip_unknown() } else { old };
    let new = if new.has_unknown_arm() { new.strip_unknown() } else { new };
    super::body::union_of(old, new)
}

/// `RH_FOLD_JOIN_V1=1` restores round 1's join (ablation).
static FIXJOIN: LazyLock<bool> = LazyLock::new(|| !std::env::var("RH_FOLD_JOIN_V1").is_ok_and(|s| s == "1"));
/// prototype (`RH_PREC_PENDBOT=1`, off by default): S2's producer split
/// for method returns. A body whose type is still pending (`Var`, or none)
/// stores ⊥ (`Ty::Bottom`) in the return table instead of minting
/// `untyped`; ⊥ is the identity of `union_of` and is not informative, so
/// the first concrete or gradual write replaces it, and a send whose
/// receiver is ⊥ types as ⊥ instead of failing dispatch.
pub(crate) static PENDBOT: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_PREC_PENDBOT").is_ok_and(|s| s == "1"));
/// `RH_FOLD_GRADUAL=1`, the handoff join keeps gradual arms.
pub(crate) static GRADUAL: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_FOLD_GRADUAL").is_ok_and(|s| s == "1"));

/// the join for slots several typings write: `join`, except
/// that two values with no informative core join by `union_of`, so the
/// result does not depend on which typing wrote last.
fn join_slot(old: Ty, new: Ty) -> Ty {
    // Fast path (prototype): a value the slot already holds, whole or as one
    // arm, changes nothing; level-2 unfolding re-accumulates the same
    // children at every visit, and `union_of` on the accumulated union was
    // 80% of the private engine's first drain.
    if contains_arm(&old, &new) {
        return old;
    }
    if !informative(&old) && !informative(&new) {
        return super::body::union_of(old, new);
    }
    join(old, new)
}

/// `old == new`, or `old` is a union with `new` as an arm (or with every arm
/// of a union `new`).
fn contains_arm(old: &Ty, new: &Ty) -> bool {
    if old == new {
        return true;
    }
    let Ty::Union { variants } = old else { return false };
    match new {
        Ty::Union { variants: nv } => nv.iter().all(|a| variants.contains(a)),
        other => variants.contains(other),
    }
}

/// the handoff join (returns and rows). With `RH_C1_PENDJOIN`
/// two pending values join by `union_of`: `join` keeps the last writer, so
/// a body whose pending result alternates (`Var` against `untyped`)
/// flips its handoff forever under a worklist (no round boundary hides it).
fn join_handoff(old: Ty, new: Ty) -> Ty {
    if *PENDJOIN {
        join_slot(old, new)
    } else {
        join(old, new)
    }
}

fn informative(t: &Ty) -> bool {
    match t {
        Ty::Untyped | Ty::Var { .. } | Ty::Bottom => false,
        Ty::Union { variants } => variants.iter().any(informative),
        _ => true,
    }
}

fn strip_var(t: Ty) -> Ty {
    match t {
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Var { .. })) => {
            let kept: Vec<Ty> = variants.into_iter().filter(|v| !matches!(v, Ty::Var { .. })).collect();
            match kept.len() {
                0 => Ty::Var { var: crate::ident::TyVar(0) },
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept.into() },
            }
        }
        other => other,
    }
}

/// `RH_FOLD_JOIN` for harvested returns: every reference-mode return slot
/// joins with the value it held after the previous harvest.
pub(crate) fn join_rets(classes: &mut HashMap<ClassId, ClassInfo>) {
    if !active() {
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let rec: Vec<MethodKey> = s.rec_methods.iter().cloned().collect();
        for (class, method) in rec {
            let Some(cls) = classes.get_mut(&class) else { continue };
            for class_side in [false, true] {
                let table = if class_side { &mut cls.class_methods } else { &mut cls.instance_methods };
                let Some(cur) = table.get(&method) else { continue };
                if matches!(cur, Ty::Fn { .. }) {
                    continue;
                }
                let key = (class.clone(), method.clone(), class_side);
                // Transfer-risk count (Fable, point 5): a harvest that does
                // not cover the value it replaces is a non-monotone step.
                if let Some(old) = s.prev_rets.get(&key) {
                    if old != cur && &super::body::union_of(old.clone(), cur.clone()) != cur {
                        *s.counts.entry("harvest_nonmonotone").or_default() += 1;
                    }
                }
                if !*JOIN {
                    s.prev_rets.insert(key, cur.clone());
                    continue;
                }
                let joined = match s.prev_rets.get(&key) {
                    Some(old) => join_handoff(old.clone(), cur.clone()),
                    None => cur.clone(),
                };
                // prototype debugging (public apps only): RH_FOLD_TRACE=<method>.
                if std::env::var("RH_FOLD_TRACE").is_ok_and(|t| t == method.as_str()) {
                    eprintln!("rh-fold-trace: join_rets {}{}{} old={:?} cur={:?} joined={:?}", class.0.as_str(), if class_side { "." } else { "#" }, method.as_str(), s.prev_rets.get(&key).map(|t| format!("{t:?}").chars().take(300).collect::<String>()), format!("{cur:?}").chars().take(300).collect::<String>(), format!("{joined:?}").chars().take(300).collect::<String>());
                }
                if &joined != cur {
                    *s.counts.entry("join_ret_widened").or_default() += 1;
                    table.insert(method.clone(), joined.clone());
                }
                s.prev_rets.insert(key, joined);
            }
        }
    });
}

// ------------------------------------------------- query engine

thread_local! {
    /// Side-table slots whose value changed since the engine last drained
    /// them (only collected while `TRACK` is on: the engine is running).
    static MOVED: RefCell<HashMap<u32, Option<Ty>>> = RefCell::new(HashMap::new());
    /// Methods that entered reference mode since the engine last drained.
    static NEW_REC: RefCell<Vec<MethodKey>> = const { RefCell::new(Vec::new()) };
    static TRACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// the query engine asks for moved slots and new
/// reference-mode methods from now on.
pub(crate) fn track_moves(on: bool) {
    TRACK.with(|t| t.set(on));
}

/// A write changed `slot`; `old` is its value before this write. The engine
/// asks for net changes only: a slot written back and forth within one
/// evaluation (two typings of a body seeding different pending values,
/// whose join keeps the last one) is not a move.
fn note_moved(slot: u32, old: Option<&Ty>) {
    if TRACK.with(|t| t.get()) {
        MOVED.with(|m| {
            m.borrow_mut().entry(slot).or_insert_with(|| old.cloned());
        });
    }
}

static DEBUG_SLOT: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_C1_DEBUG_SLOT").is_ok());

/// Public inputs only: name every moved slot as it is drained
/// (`RH_C1_DEBUG_MOVES=1`).
static DEBUG_MOVES: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_C1_DEBUG_MOVES").is_ok_and(|s| s == "1"));

fn note_new_rec(m: MethodKey) {
    if TRACK.with(|t| t.get()) {
        NEW_REC.with(|n| n.borrow_mut().push(m));
    }
}

/// the side table as per-slot hashes keyed by slot key (so
/// they compare across runs), for the complete-state fingerprint.
pub(crate) fn side_table_hashes(h: &mut dyn FnMut(&Ty) -> u64) -> BTreeMap<String, u64> {
    let entries: Vec<(String, Ty)> = ST.with(|s| {
        let s = s.borrow();
        s.param_values
            .iter()
            .map(|(slot, t)| (format!("{:?}", s.keys.get(*slot as usize)), t.clone()))
            .collect()
    });
    entries.into_iter().map(|(k, t)| (k, h(&t))).collect()
}

/// total type nodes in the side table (diagnosis).
pub(crate) fn side_table_nodes() -> u64 {
    fn nodes(t: &Ty, n: &mut u64) {
        *n += 1;
        visit_children(t, &mut |c| nodes(c, n));
    }
    ST.with(|s| {
        let s = s.borrow();
        let mut n = 0u64;
        for t in s.param_values.values() {
            nodes(t, &mut n);
        }
        n
    })
}

/// Side-table slots that moved since the last call (deduplicated).
pub(crate) fn take_moved() -> Vec<u32> {
    let firsts = MOVED.with(|m| std::mem::take(&mut *m.borrow_mut()));
    let mut dbg: Vec<(u32, String)> = Vec::new();
    let mut v: Vec<u32> = ST.with(|s| {
        let s = s.borrow();
        firsts
            .into_iter()
            .filter(|(slot, before)| {
                let moved = s.param_values.get(slot) != before.as_ref();
                if moved && *DEBUG_MOVES {
                    let cut = |t: Option<&Ty>| format!("{t:?}").chars().take(240).collect::<String>();
                    dbg.push((*slot, format!("{} -> {}", cut(before.as_ref()), cut(s.param_values.get(slot)))));
                }
                moved
            })
            .map(|(k, _)| k)
            .collect()
    });
    for (slot, what) in dbg {
        eprintln!("c1-debug-move {} {what}", slot_name(slot));
    }
    v.sort_unstable();
    v.dedup();
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

/// `join_rets` for one method (both sides), after the engine harvested it.
pub(crate) fn join_ret_one(classes: &mut HashMap<ClassId, ClassInfo>, class: &ClassId, method: &Symbol) {
    if !active() || !*JOIN {
        return;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if !s.rec_methods.contains(&(class.clone(), method.clone())) {
            return;
        }
        let Some(cls) = classes.get_mut(class) else { return };
        for class_side in [false, true] {
            let table = if class_side { &mut cls.class_methods } else { &mut cls.instance_methods };
            let Some(cur) = table.get(method) else { continue };
            if matches!(cur, Ty::Fn { .. }) {
                continue;
            }
            let key = (class.clone(), method.clone(), class_side);
            let joined = match s.prev_rets.get(&key) {
                Some(old) => join_handoff(old.clone(), cur.clone()),
                None => cur.clone(),
            };
            if &joined != cur {
                *s.counts.entry("join_ret_widened").or_default() += 1;
                table.insert(method.clone(), joined.clone());
            }
            s.prev_rets.insert(key, joined);
        }
    });
}

/// `join_params` for one row the engine recomputed: a reference-mode
/// method's row joins with its previous row (`commit` records it).
pub(crate) fn join_param_row(key: &MethodKey, raw: Option<Vec<Ty>>, commit: bool) -> Option<Vec<Ty>> {
    if !active() || !*JOIN {
        return raw;
    }
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if !s.rec_methods.contains(key) {
            return raw;
        }
        let mut row = raw.unwrap_or_default();
        if let Some(prev) = s.prev_params.get(key).cloned() {
            if row.len() < prev.len() {
                row.resize(prev.len(), Ty::Var { var: crate::ident::TyVar(0) });
            }
            for (slot, old) in row.iter_mut().zip(prev) {
                *slot = join_handoff(old, slot.clone());
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

// ---------------------------------------------------------------- loops

pub(crate) fn note_loop(name: &str, rounds: usize, converged: bool) {
    if (*ON || std::env::var("RH_FOLD_LOOPS").is_ok_and(|s| s == "1")) && !*QUIET {
        let line = serde_json::json!({"loop": name, "rounds": rounds, "converged": converged});
        eprintln!("rh-fold: {line}");
        ST.with(|s| s.borrow_mut().loops.push(line));
    }
}

// ---------------------------------------------------------------- expansion

/// Expansion of every reference-mode slot for the analyzer's consumers:
/// references to slots outside a cycle expand fully; a reference back to a
/// slot already being expanded (the back edge of a cycle) becomes `Untyped`.
pub(crate) struct Expander<'a> {
    classes: &'a HashMap<ClassId, ClassInfo>,
    memo: HashMap<u32, Ty>,
    stack: Vec<u32>,
    /// prototype (`RH_FOLD_TAIL`): constructor depth when each stack entry
    /// was pushed, and the current depth. A back edge met at the same
    /// depth is unguarded.
    depth_at: Vec<usize>,
    ctor: usize,
    pub cyclic: HashSet<u32>,
    pub back_edges: u64,
    pub budget_cuts: u64,
    pub tail_edges: u64,
}

/// Node budget per top-level expansion (`RH_FOLD_EXPAND=N`, default 256).
static EXPAND_NODES: LazyLock<usize> =
    LazyLock::new(|| std::env::var("RH_FOLD_EXPAND").ok().and_then(|s| s.parse().ok()).unwrap_or(256));

impl<'a> Expander<'a> {
    pub(crate) fn new(classes: &'a HashMap<ClassId, ClassInfo>) -> Self {
        Expander {
            classes,
            memo: HashMap::new(),
            stack: Vec::new(),
            depth_at: Vec::new(),
            ctor: 0,
            cyclic: HashSet::new(),
            back_edges: 0,
            budget_cuts: 0,
            tail_edges: 0,
        }
    }

    pub(crate) fn expand(&mut self, t: &Ty) -> Ty {
        if !contains_rec(t) {
            return t.clone();
        }
        let mut budget = *EXPAND_NODES;
        let r = self.go(t, &mut budget);
        // A slot made only of unguarded back edges (`X = X`) never
        // returns; keep main's `untyped` for it rather than `Bottom`.
        if matches!(r, Ty::Bottom) && !matches!(t, Ty::Bottom) { Ty::Untyped } else { r }
    }

    fn go(&mut self, t: &Ty, budget: &mut usize) -> Ty {
        if *budget == 0 {
            if contains_rec(t) {
                self.budget_cuts += 1;
                return Ty::Untyped;
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
                        self.tail_edges += 1;
                        return Ty::Bottom;
                    }
                    self.back_edges += 1;
                    return Ty::Untyped;
                }
                if let Some(m) = self.memo.get(slot) {
                    return m.clone();
                }
                let Some(v) = value_of(*slot, self.classes) else { return Ty::Untyped };
                self.stack.push(*slot);
                self.depth_at.push(self.ctor);
                let cyc_before = self.cyclic.len();
                let r = self.go(&v, budget);
                // A slot made only of unguarded back edges never returns a
                // value; keep main's `untyped` rather than leaking `Bottom`
                // (a `Bottom` receiver reads as known to the diagnostics).
                let r = if matches!(r, Ty::Bottom) && !matches!(v, Ty::Bottom) { Ty::Untyped } else { r };
                self.stack.pop();
                self.depth_at.pop();
                // Only an expansion that met no back edge is context-free.
                if self.cyclic.len() == cyc_before && !self.cyclic.contains(slot) {
                    self.memo.insert(*slot, r.clone());
                }
                r
            }
            Ty::Union { variants } => {
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
        match t {
            Ty::Array { elem } => Ty::Array { elem: std::sync::Arc::new(self.go(elem, budget)) },
            Ty::Hash { key, value } => {
                Ty::Hash { key: std::sync::Arc::new(self.go(key, budget)), value: std::sync::Arc::new(self.go(value, budget)) }
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
                    .map(|p| crate::ty::Param { name: p.name.clone(), ty: self.go(&p.ty, budget).into(), kind: p.kind.clone() })
                    .collect(),
                block: block.as_ref().map(|b| std::sync::Arc::new(self.go(b, budget))),
                ret: std::sync::Arc::new(self.go(ret, budget)),
                effects: effects.clone(),
            },
            other => other.clone(),
        }
    }
}

pub(crate) fn deactivate() {
    ST.with(|s| s.borrow_mut().active = false);
}

/// Reference graph over slots (slot -> slots its value mentions) and its
/// cyclic slots, for the report.
fn slot_graph(classes: &HashMap<ClassId, ClassInfo>) -> (Vec<u32>, HashMap<u32, Vec<u32>>, HashMap<u32, Ty>) {
    let n = ST.with(|s| s.borrow().keys.len() as u32);
    let mut out = HashMap::new();
    let mut values = HashMap::new();
    let mut slots = Vec::new();
    for slot in 0..n {
        let Some(v) = value_of(slot, classes) else { continue };
        let mut refs = Vec::new();
        visit(&v, &mut |c| {
            if let Ty::Rec { slot: s } = c {
                if !refs.contains(s) {
                    refs.push(*s)
                }
            }
        });
        slots.push(slot);
        out.insert(slot, refs);
        values.insert(slot, v);
    }
    (slots, out, values)
}

fn size_of(t: &Ty) -> (u64, u32) {
    let mut n = 0u64;
    fn depth(t: &Ty) -> u32 {
        let mut d = 0;
        let mut f = |c: &Ty| d = d.max(depth(c));
        match t {
            Ty::Array { elem } => f(elem),
            Ty::Hash { key, value } => {
                f(key);
                f(value)
            }
            Ty::Tuple { elems } | Ty::Union { variants: elems } => elems.iter().for_each(|e| f(e)),
            Ty::Record { row } => row.fields.values().for_each(|v| f(v)),
            Ty::Class { args, .. } => args.iter().for_each(|a| f(a)),
            Ty::Fn { params, block, ret, .. } => {
                params.iter().for_each(|p| f(&p.ty));
                if let Some(b) = block {
                    f(b)
                }
                f(ret)
            }
            _ => {}
        }
        d + 1
    }
    visit(t, &mut |_| n += 1);
    (n, depth(t))
}

/// The final `rh-fold:` summary (aggregates) and, under `RH_FOLD_PRINT`,
/// the grammar of every reference-mode slot as RBS-style aliases.
pub(crate) fn report(classes: &HashMap<ClassId, ClassInfo>, expander: &Expander, extra: serde_json::Value) {
    if !*ON || *QUIET {
        return;
    }
    let (slots, graph, values) = slot_graph(classes);
    // Cyclic slots: on a cycle of the slot reference graph.
    let ix: HashMap<u32, usize> = slots.iter().enumerate().map(|(i, s)| (*s, i)).collect();
    let succ: Vec<Vec<usize>> = slots
        .iter()
        .map(|s| graph[s].iter().filter_map(|t| ix.get(t).copied()).collect())
        .collect();
    let mut cyclic: HashSet<u32> = HashSet::new();
    for comp in scc(&succ) {
        let ids: Vec<u32> = comp.iter().map(|i| slots[*i]).collect();
        let nontrivial = ids.len() > 1 || graph.get(&ids[0]).is_some_and(|o| o.contains(&ids[0]));
        if nontrivial {
            cyclic.extend(ids);
        }
    }
    let (mut ret_slots, mut param_slots, mut narrowed_slots, mut pos_slots) = (0u64, 0u64, 0u64, 0u64);
    let (mut nodes, mut max_nodes, mut max_depth) = (0u64, 0u64, 0u32);
    let (mut cyc_nodes, mut cyc_max) = (0u64, 0u64);
    for s in &slots {
        match key_of(*s) {
            Some(SlotKey::Ret { .. }) => ret_slots += 1,
            Some(SlotKey::Param { .. }) => param_slots += 1,
            Some(SlotKey::Narrow { .. }) => narrowed_slots += 1,
            Some(SlotKey::At { .. }) => pos_slots += 1,
            None => {}
        }
        let (n, d) = size_of(&values[s]);
        nodes += n;
        max_nodes = max_nodes.max(n);
        max_depth = max_depth.max(d);
        if cyclic.contains(s) {
            cyc_nodes += n;
            cyc_max = cyc_max.max(n);
        }
    }
    let line = ST.with(|st| {
        let st = st.borrow();
        let site_json: BTreeMap<&str, serde_json::Value> = (0..NSITES)
            .map(|i| (SITE_NAMES[i], serde_json::json!({"on": site_on_ix(i), "unfolds": st.unfold_on[i], "as_untyped": st.unfold_off[i]})))
            .collect();
        serde_json::json!({
            "summary": true,
            "rec_methods": st.rec_methods.len(),
            "slots_interned": st.keys.len(),
            "slots_with_value": slots.len(),
            "ret_slots": ret_slots, "param_slots": param_slots, "narrowed_slots": narrowed_slots,
            "pos_slots": pos_slots, "level": *LEVEL,
            "cyclic_slots": cyclic.len(),
            "grammar_nodes": nodes, "grammar_max_nodes": max_nodes, "grammar_max_depth": max_depth,
            "cyclic_grammar_nodes": cyc_nodes, "cyclic_grammar_max_nodes": cyc_max,
            "refs_made": {"ret": st.refs_ret, "param": st.refs_param, "narrowed": st.refs_narrowed},
            "sites": site_json,
            "expansion": {"back_edges": expander.back_edges, "budget_cuts": expander.budget_cuts, "cyclic_seen": expander.cyclic.len(), "tail_edges": expander.tail_edges},
            "counts": st.counts,
            "loops": st.loops,
            "extra": extra,
            "join": *JOIN,
        })
    });
    eprintln!("rh-fold: {line}");
    if *PRINT {
        // Narrowed slots outside a cycle are an implementation detail:
        // inline them where they are used. Empty (unreachable) arms drop.
        let inline: HashSet<u32> = slots
            .iter()
            .copied()
            .filter(|s| matches!(key_of(*s), Some(SlotKey::Narrow { .. } | SlotKey::At { .. })) && !cyclic.contains(s))
            .collect();
        // Every body is printed from `resolve`: top-level references are
        // flattened into their arms (an unguarded `X = X | A` prints as
        // `A`, its least solution), so no alias-only cycle remains.
        let resolved = |slot: u32| -> Ty {
            let arms = resolve(slot, classes, &mut HashSet::new());
            if arms.is_empty() { Ty::Bottom } else { super::body::union_many(arms) }
        };
        let mut order: Vec<u32> = slots.iter().copied().filter(|s| !inline.contains(s)).collect();
        order.sort_by_key(|s| slot_name(*s));
        for s in order {
            let tag = if cyclic.contains(&s) { "cyclic" } else { "acyclic" };
            let body = rbs_with(&resolved(s), &|slot| inline.contains(&slot).then(|| resolved(slot)));
            eprintln!("rh-fold-rbs: type {} = {}   # {tag}", slot_name(s), body);
        }
    }
}

/// `rbs`, with some references printed inline and `Bottom` arms dropped.
fn rbs_with(t: &Ty, inline: &dyn Fn(u32) -> Option<Ty>) -> String {
    fn flat(t: &Ty, inline: &dyn Fn(u32) -> Option<Ty>, out: &mut Vec<String>) {
        match t {
            Ty::Bottom => {}
            Ty::Union { variants } => variants.iter().for_each(|v| flat(v, inline, out)),
            Ty::Rec { slot } => match inline(*slot) {
                Some(v) => flat(&v, inline, out),
                None => out.push(slot_name(*slot)),
            },
            other => {
                let s = go(other, inline);
                if !out.contains(&s) {
                    out.push(s)
                }
            }
        }
    }
    fn go(t: &Ty, inline: &dyn Fn(u32) -> Option<Ty>) -> String {
        match t {
            Ty::Array { elem } => format!("Array[{}]", top(elem, inline)),
            Ty::Hash { key, value } => format!("Hash[{}, {}]", top(key, inline), top(value, inline)),
            Ty::Tuple { elems } => format!("[{}]", elems.iter().map(|e| top(e, inline)).collect::<Vec<_>>().join(", ")),
            Ty::Record { row } => format!(
                "{{ {} }}",
                row.fields.iter().map(|(k, v)| format!("{}: {}", k.as_str(), top(v, inline))).collect::<Vec<_>>().join(", ")
            ),
            Ty::Class { id, args } if !args.is_empty() => {
                format!("{}[{}]", id.0.as_str(), args.iter().map(|a| top(a, inline)).collect::<Vec<_>>().join(", "))
            }
            Ty::Union { .. } | Ty::Rec { .. } | Ty::Bottom => top(t, inline),
            other => rbs(other),
        }
    }
    fn top(t: &Ty, inline: &dyn Fn(u32) -> Option<Ty>) -> String {
        let mut out = Vec::new();
        flat(t, inline, &mut out);
        if out.is_empty() { "bot".into() } else { out.join(" | ") }
    }
    top(t, inline)
}

fn site_on_ix(i: usize) -> bool {
    *SITES & (1 << i) != 0
}

fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch == ':' {
            if !out.ends_with('_') {
                out.push('_');
            }
            continue;
        }
        if ch.is_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else if ch.is_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn site_tag(site: SiteId) -> String {
    if site.0 == u32::MAX { format!("m{:x}", site.1) } else { format!("{}_{}", site.0, site.1) }
}

pub(crate) fn slot_name(slot: u32) -> String {
    match key_of(slot) {
        Some(SlotKey::Ret { class, method, class_side }) => {
            format!("{}_{}{}_ret", snake(class.0.as_str()), if class_side { "self_" } else { "" }, snake(method.as_str()))
        }
        Some(SlotKey::Param { class, method, index }) => {
            format!("{}_{}_arg{}", snake(class.0.as_str()), snake(method.as_str()), index)
        }
        Some(SlotKey::Narrow { site, filter }) => format!("narrow_{}_{}_{}", site_tag(site), snake(&filter), slot),
        Some(SlotKey::At { site, step }) => format!("at_{}_{}_{}", site_tag(site), step.name(), slot),
        None => format!("slot{slot}"),
    }
}

/// RBS-style rendering, references by alias name.
pub(crate) fn rbs(t: &Ty) -> String {
    match t {
        Ty::Int => "Integer".into(),
        Ty::Float => "Float".into(),
        Ty::Bool => "bool".into(),
        Ty::Str => "String".into(),
        Ty::Sym => "Symbol".into(),
        Ty::Date => "Date".into(),
        Ty::Time => "Time".into(),
        Ty::Nil => "nil".into(),
        Ty::Relation { of } => format!("ActiveRecord::Relation[{}]", of.0.as_str()),
        Ty::Array { elem } => format!("Array[{}]", rbs(elem)),
        Ty::Hash { key, value } => format!("Hash[{}, {}]", rbs(key), rbs(value)),
        Ty::Tuple { elems } => format!("[{}]", elems.iter().map(rbs).collect::<Vec<_>>().join(", ")),
        Ty::Record { row } => format!(
            "{{ {} }}",
            row.fields.iter().map(|(k, v)| format!("{}: {}", k.as_str(), rbs(v))).collect::<Vec<_>>().join(", ")
        ),
        Ty::Union { variants } => variants.iter().map(rbs).collect::<Vec<_>>().join(" | "),
        Ty::SelfInstance => "instance".into(),
        Ty::Class { id, args } if args.is_empty() => id.0.as_str().to_string(),
        Ty::Class { id, args } => {
            format!("{}[{}]", id.0.as_str(), args.iter().map(rbs).collect::<Vec<_>>().join(", "))
        }
        Ty::Fn { .. } => "Proc".into(),
        Ty::Var { .. } => "untyped".into(),
        Ty::Untyped => "untyped".into(),
        Ty::Bottom => "bot".into(),
        Ty::Rec { slot } => slot_name(*slot),
    }
}

//! Determinism detectors, off by default.
//!
//! - `RH_DESCENT=1`, the descent sanitizer. Every instrumented slot write is
//!   checked against the join order: `Var` and `Bottom` are ⊥, a union is
//!   the set of its arms, `untyped` is one arm (gradual evidence, not ⊤),
//!   constructors compare pointwise and records field by field. A write
//!   whose new value is not above or equal to the old one is a descent. A
//!   monotone run from ⊥ has none, so each hit names a non-monotone rule
//!   site. `rh-descent:` prints, per site: writes, descents, and the
//!   descents that only drop `untyped` arms, that replace a value by one
//!   with no informative core, and the rest. Aggregates only.
//! - `RH_SHUFFLE=<seed>`, the shuffled-order control: a seeded permutation of
//!   the worklist's pop order within each drain (`shuffle_key`). HashMap
//!   keys retain the platform default; no platform-specific entropy hook
//!   is installed. An order-independent typer reaches the same state for
//!   every worklist seed.
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use crate::ty::Ty;

static DESCENT: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_DESCENT").is_ok_and(|v| v == "1"));
static SHUFFLE: LazyLock<Option<u64>> = LazyLock::new(|| {
    std::env::var("RH_SHUFFLE")
        .ok()
        .and_then(|v| v.trim().parse().ok())
});

/// Per site: writes, descents, untyped-only drops, drops of fold-reference
/// arms (with or without `untyped`), to-pending/untyped, other.
static COUNTS: Mutex<BTreeMap<&'static str, [u64; 6]>> = Mutex::new(BTreeMap::new());
static SHUFFLED_POPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static VALS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());
/// The same digests per harvest round (`next_round`), to find where two runs
/// first diverge.
static BY_ROUND: Mutex<BTreeMap<(u32, &'static str), u64>> = Mutex::new(BTreeMap::new());
static ROUND: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A fold slot was interned (`fold::intern`, inside the fold's state borrow):
/// remember its key's hash the way `c1fp::type_hash` computes it, so value
/// hashing never has to read the fold's state (which is borrowed there).
pub(crate) fn note_slot_key(slot: u32, key: &super::fold::SlotKey) {
    if *DESCENT {
        use std::hash::{Hash, Hasher};
        let mut kh = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}", Some(key)).hash(&mut kh);
        let h = kh.finish();
        VAL_MEMO.with(|m| {
            m.borrow_mut().insert(slot, h);
        });
    }
}

/// A main-loop harvest starts: later writes belong to the next round.
pub(crate) fn next_round() {
    if *DESCENT {
        ROUND.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

std::thread_local! {
    static VAL_MEMO: std::cell::RefCell<std::collections::HashMap<u32, u64>> = std::cell::RefCell::new(std::collections::HashMap::new());
}
/// Certificate fields: cuts that stop a rule before it settles.
static CAPS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// A cap or limit fired (`RH_DESCENT=1` only): it stopped a rule before its
/// output held still, so the slot is not at its fixpoint.
pub(crate) fn note_cap(name: &'static str) {
    if *DESCENT {
        *CAPS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(name)
            .or_insert(0) += 1;
    }
}

pub(crate) fn descent_on() -> bool {
    *DESCENT
}

/// A write of `new` over `old` at `site` (`old` is `None` for a first write).
pub(crate) fn note(site: &'static str, old: Option<&Ty>, new: &Ty) {
    if matches!(
        site,
        "harvest.first" | "harvest.stabilize" | "harvest.untie_result" | "harvest.lastwrite"
    ) {
        super::errgate::note_drop(old, new);
    }
    if !*DESCENT {
        return;
    }
    let kind = match old {
        Some(old) if !leq(old, new) => Some(if leq(&strip_untyped(old, false), new) {
            2
        } else if leq(&strip_untyped(old, true), new) {
            3
        } else if !informative(new) {
            4
        } else {
            5
        }),
        _ => None,
    };
    // The multiset of values written at this site, as a sum of mixed
    // structural hashes (order-independent, `Var` ids ignored, references by
    // slot key): two runs whose sites wrote the same values agree here.
    let vh = mix(VAL_MEMO.with(|m| super::detfp::type_hash(new, &mut m.borrow_mut())));
    {
        let mut v = VALS.lock().unwrap_or_else(|e| e.into_inner());
        let x = v.entry(site).or_insert(0);
        *x = x.wrapping_add(vh);
        let r = ROUND.load(std::sync::atomic::Ordering::Relaxed);
        let mut by = BY_ROUND.lock().unwrap_or_else(|e| e.into_inner());
        let y = by.entry((r, site)).or_insert(0);
        *y = y.wrapping_add(vh);
    }
    let mut c = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    let e = c.entry(site).or_insert([0; 6]);
    e[0] += 1;
    if let Some(k) = kind {
        e[1] += 1;
        e[k] += 1;
    }
}

/// Public inputs only (`RH_DET_TRACE=<method name>`): `note`, and print the
/// write when the slot's method is the traced one.
static TRACE: LazyLock<std::collections::HashSet<String>> = LazyLock::new(|| {
    std::env::var("RH_DET_TRACE")
        .map(|v| v.split(',').map(str::to_string).collect())
        .unwrap_or_default()
});
pub(crate) fn note_named(site: &'static str, name: &str, old: Option<&Ty>, new: &Ty) {
    note(site, old, new);
    if std::env::var_os("RH_DET_TRACE").is_some() && !TRACE.is_empty() && TRACE.contains(name) {
        eprintln!(
            "rh-det-trace: {site} {name} leq={} old={} new={}",
            old.is_none_or(|o| leq(o, new)),
            old.map(|o| format!("{o:?}").chars().take(300).collect::<String>())
                .unwrap_or_default(),
            format!("{new:?}").chars().take(300).collect::<String>()
        );
    }
}

/// A row write: each position is a slot.
pub(crate) fn note_row(site: &'static str, old: Option<&[Ty]>, new: &[Ty]) {
    if !*DESCENT {
        return;
    }
    for (i, t) in new.iter().enumerate() {
        note(site, old.and_then(|o| o.get(i)), t);
    }
}

/// The join order (see the module comment). `a ⊑ b`.
pub(crate) fn leq(a: &Ty, b: &Ty) -> bool {
    if a == b {
        return true;
    }
    match (a, b) {
        (Ty::Var { .. } | Ty::Bottom, _) => true,
        (Ty::Union { variants }, _) => variants.iter().all(|v| leq(v, b)),
        (_, Ty::Union { variants }) => variants.iter().any(|v| leq(a, v)),
        (Ty::Array { elem: x }, Ty::Array { elem: y }) => leq(x, y),
        (Ty::Hash { key: k1, value: v1 }, Ty::Hash { key: k2, value: v2 }) => {
            leq(k1, k2) && leq(v1, v2)
        }
        (Ty::Tuple { elems: x }, Ty::Tuple { elems: y }) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| leq(p, q))
        }
        (Ty::Class { id: i1, args: a1 }, Ty::Class { id: i2, args: a2 }) => {
            i1 == i2 && a1.len() == a2.len() && a1.iter().zip(a2.iter()).all(|(p, q)| leq(p, q))
        }
        (Ty::Record { row: r1 }, Ty::Record { row: r2 }) => r1.fields.iter().all(|(k, v)| {
            r2.fields
                .iter()
                .find(|(k2, _)| *k2 == k)
                .is_some_and(|(_, w)| leq(v, w))
        }),
        (
            Ty::Fn {
                params: p1,
                ret: r1,
                ..
            },
            Ty::Fn {
                params: p2,
                ret: r2,
                ..
            },
        ) => {
            p1.len() == p2.len()
                && p1.iter().zip(p2.iter()).all(|(x, y)| leq(&x.ty, &y.ty))
                && leq(r1, r2)
        }
        _ => false,
    }
}

fn informative(t: &Ty) -> bool {
    match t {
        Ty::Untyped { .. } | Ty::Var { .. } | Ty::Bottom => false,
        Ty::Union { variants } => variants.iter().any(informative),
        _ => true,
    }
}

/// `t` with every `untyped` arm (and, with `refs`, every fold reference),
/// at any depth, read as ⊥. A reference's value is not known here.
fn strip_untyped(t: &Ty, refs: bool) -> Ty {
    let bot = || Ty::Var {
        var: crate::ident::TyVar(0),
    };
    match t {
        Ty::Untyped { .. } => bot(),
        Ty::Rec { .. } if refs => bot(),
        Ty::Union { variants } => {
            let kept: Vec<Ty> = variants
                .iter()
                .filter(|v| {
                    !matches!(v, Ty::Untyped { .. }) && !(refs && matches!(v, Ty::Rec { .. }))
                })
                .map(|v| strip_untyped(v, refs))
                .collect();
            match kept.len() {
                0 => bot(),
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union {
                    variants: kept.into(),
                },
            }
        }
        Ty::Array { elem } => Ty::Array {
            elem: std::sync::Arc::new(strip_untyped(elem, refs)),
        },
        Ty::Hash { key, value } => Ty::Hash {
            key: std::sync::Arc::new(strip_untyped(key, refs)),
            value: std::sync::Arc::new(strip_untyped(value, refs)),
        },
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems.iter().map(|v| strip_untyped(v, refs)).collect(),
        },
        Ty::Class { id, args } => Ty::Class {
            id: id.clone(),
            args: args.iter().map(|v| strip_untyped(v, refs)).collect(),
        },
        Ty::Record { row } => Ty::Record {
            row: crate::ty::Row {
                fields: row
                    .fields
                    .iter()
                    .map(|(k, v)| (k.clone(), strip_untyped(v, refs)))
                    .collect(),
                rest: row.rest.clone(),
            },
        },
        other => other.clone(),
    }
}

/// SplitMix64's finalizer.
pub(crate) fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The worklist's priority for unit `u` in drain `drain`: its WTO position,
/// or under `RH_SHUFFLE` a seeded per-drain permutation of the units.
pub(crate) fn shuffle_key(u: u32, drain: u64, pos: u32) -> u32 {
    match *SHUFFLE {
        None => pos,
        Some(seed) => {
            SHUFFLED_POPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            (mix(mix(seed ^ mix(drain)) ^ u as u64) >> 32) as u32
        }
    }
}

/// The `rh-descent:` and `rh-shuffle:` lines (numbers and site labels only).
pub(crate) fn report() {
    if *DESCENT {
        let c = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut total = [0u64; 6];
        let sites: Vec<String> = c
            .iter()
            .map(|(site, v)| {
                for i in 0..6 {
                    total[i] += v[i];
                }
                format!(
                    "\"{site}\":[{},{},{},{},{},{}]",
                    v[0], v[1], v[2], v[3], v[4], v[5]
                )
            })
            .collect();
        eprintln!(
            "rh-descent: {{\"columns\":[\"writes\",\"descents\",\"drop_untyped\",\"drop_ref\",\"to_pending\",\"other\"],\"total\":[{},{},{},{},{},{}],\"sites\":{{{}}},\"caps\":{{{}}},\"values\":{{{}}},\"by_round\":{{{}}}}}",
            total[0],
            total[1],
            total[2],
            total[3],
            total[4],
            total[5],
            sites.join(","),
            CAPS.lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|(k, v)| format!("\"{k}\":{v}"))
                .collect::<Vec<_>>()
                .join(","),
            VALS.lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|(k, v)| format!("\"{k}\":\"{v:016x}\""))
                .collect::<Vec<_>>()
                .join(","),
            BY_ROUND
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|((r, k), v)| format!("\"{r}@{k}\":\"{:08x}\"", v >> 32))
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    if let Some(seed) = *SHUFFLE {
        eprintln!(
            "rh-shuffle: {}",
            serde_json::json!({
                "seed": seed, "keyed_pushes": SHUFFLED_POPS.load(std::sync::atomic::Ordering::Relaxed),
                "hash_keys": "platform-default"
            })
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_order_basics() {
        let u = |a: Ty, b: Ty| crate::analyze::body::union_of(a, b);
        assert!(leq(
            &Ty::Var {
                var: crate::ident::TyVar(3)
            },
            &Ty::Int
        ));
        assert!(leq(&Ty::Int, &u(Ty::Int, Ty::unresolved())));
        assert!(!leq(&u(Ty::Int, Ty::unresolved()), &Ty::Int));
        assert!(!leq(&Ty::Int, &Ty::unresolved()));
        assert!(leq(
            &strip_untyped(&u(Ty::Int, Ty::unresolved()), false),
            &Ty::Int
        ));
        assert_eq!(shuffle_key(5, 1, 7), 7);
    }
}

/// Start an independent analysis; detector counters do not leak across apps.
pub(crate) fn reset() {
    use std::sync::atomic::Ordering::Relaxed;
    if *DESCENT {
        COUNTS.lock().unwrap().clear();
        VALS.lock().unwrap().clear();
        BY_ROUND.lock().unwrap().clear();
        CAPS.lock().unwrap().clear();
        ROUND.store(0, Relaxed);
        VAL_MEMO.with(|m| m.borrow_mut().clear());
    }
    if SHUFFLE.is_some() {
        SHUFFLED_POPS.store(0, Relaxed);
    }
}

/// Passive catch-all for registry changes made during a full harvest round.
type Registry = BTreeMap<(crate::ident::ClassId, bool, crate::ident::Symbol), Ty>;
pub(crate) fn registry_snapshot(
    classes: &std::collections::HashMap<crate::ident::ClassId, super::body::ClassInfo>,
) -> Option<Registry> {
    if !descent_on() {
        return None;
    }
    let mut out = BTreeMap::new();
    for (id, ci) in classes {
        for (side, table) in [(false, &ci.instance_methods), (true, &ci.class_methods)] {
            for (m, t) in table {
                out.insert((id.clone(), side, m.clone()), t.clone());
            }
        }
    }
    Some(out)
}
pub(crate) fn note_registry(
    site: &'static str,
    before: &Registry,
    classes: &std::collections::HashMap<crate::ident::ClassId, super::body::ClassInfo>,
) {
    for (id, ci) in classes {
        for (side, table) in [(false, &ci.instance_methods), (true, &ci.class_methods)] {
            for (m, t) in table {
                note(site, before.get(&(id.clone(), side, m.clone())), t);
            }
        }
    }
}

/// Optional normalized joins; zero leaves existing typing rules unchanged.
static DET: LazyLock<u8> = LazyLock::new(|| std::env::var("RH_DET").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0));

/// The replacements are on.
pub(crate) fn det_on() -> bool {
    *DET >= 1
}

/// `RH_DET_KEEP_UNRESOLVED=1`: retain inference gaps once pending slots settle.
static KEEP_UNRESOLVED: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_DET_KEEP_UNRESOLVED").as_deref() == Ok("1"));

std::thread_local! {
    static QUIESCENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn keep_unresolved_on() -> bool {
    det_on() && *KEEP_UNRESOLVED
}

pub(crate) struct Quiescent(bool);

impl Drop for Quiescent {
    fn drop(&mut self) {
        QUIESCENT.with(|q| q.set(self.0));
    }
}

/// Replay settled producers before handoff: a remaining Var is an inference
/// gap, not a pending value that a later round can replace.
pub(crate) fn quiescent_scope() -> Quiescent {
    Quiescent(QUIESCENT.with(|q| q.replace(true)))
}

/// How a writer's new value meets the entry it wrote before
/// (`RH_DET_WRITE=join|last`, default `join`): `join` is the inflationary
/// join of everything the entry was ever written; `last` is the
/// per-writer entry (the Kleene step), which needs monotone body transfers:
/// it is intended for monotone body transfers.
static WRITE_JOIN: LazyLock<bool> = LazyLock::new(|| !std::env::var("RH_DET_WRITE").is_ok_and(|v| v == "last"));
pub(crate) fn write_join() -> bool {
    *WRITE_JOIN
}

/// J₁: the gradual bit is kept by every join (the best transformer); J₀
/// (`RH_DET=1`) lets an informative non-`Nil` arm absorb it, as main does.
pub(crate) fn keep_gradual() -> bool {
    *DET >= 2
}

/// The join of D : `union_of` (canonical, spine-merging), then
/// `norm`. Commutative, associative and idempotent, so no write order can
/// matter.
pub(crate) fn lat_join(a: Ty, b: Ty) -> Ty {
    let j = super::body::union_of(a, b);
    norm_opt(&j, true).unwrap_or(j)
}

/// D's normal form, at every position: `Var` arms vanish (⊥); arms of one
/// shape merge pointwise, as `union_of` already does for Array and Hash
/// spines (Tuples of one arity, Classes of one id and arity, Records of one
/// key set), so a transient arm typed against a pending input is absorbed by
/// the arm it later becomes instead of staying beside it; under J₀ an
/// `untyped` arm beside an informative arm other than `Nil` is absorbed. The
/// result depends only on the arm set at each position, so the join stays
/// associative. Below the top, J₀ reads `untyped` as ⊥ like `Var`, so the
/// two are one marker there (printed `untyped`): `Array[Var]` from an empty
/// literal and `Array[untyped]` from the same literal typed against a
/// placeholder are the same value. `None` when `t` is already normal.
fn norm_opt(t: &Ty, top: bool) -> Option<Ty> {
    fn each(xs: &[Ty]) -> Option<Vec<Ty>> {
        let mut out: Option<Vec<Ty>> = None;
        for (i, x) in xs.iter().enumerate() {
            if let Some(n) = norm_opt(x, false) {
                out.get_or_insert_with(|| xs[..i].to_vec()).push(n);
            } else if let Some(o) = out.as_mut() {
                o.push(x.clone());
            }
        }
        out
    }
    fn same_shape(a: &Ty, b: &Ty) -> bool {
        match (a, b) {
            (Ty::Tuple { elems: x }, Ty::Tuple { elems: y }) => x.len() == y.len(),
            (Ty::Class { id: i, args: x }, Ty::Class { id: j, args: y }) => i == j && x.len() == y.len() && !x.is_empty(),
            (Ty::Record { row: x }, Ty::Record { row: y }) => {
                x.fields.len() == y.fields.len() && x.fields.iter().all(|(k, _)| y.fields.iter().any(|(k2, _)| k2.as_str() == k.as_str()))
            }
            _ => false,
        }
    }
    fn pointwise(a: &Ty, b: &Ty) -> Ty {
        let j = |x: &Ty, y: &Ty| lat_join(x.clone(), y.clone());
        match (a, b) {
            (Ty::Tuple { elems: x }, Ty::Tuple { elems: y }) => {
                Ty::Tuple { elems: x.iter().zip(y.iter()).map(|(p, q)| j(p, q)).collect() }
            }
            (Ty::Class { id, args: x }, Ty::Class { args: y, .. }) => {
                Ty::Class { id: id.clone(), args: x.iter().zip(y.iter()).map(|(p, q)| j(p, q)).collect() }
            }
            (Ty::Record { row: x }, Ty::Record { row: y }) => Ty::Record {
                row: crate::ty::Row {
                    fields: x
                        .fields
                        .iter()
                        .map(|(k, v)| {
                            let w = y.fields.iter().find(|(k2, _)| k2.as_str() == k.as_str()).map(|(_, w)| w.clone());
                            (k.clone(), match w { Some(w) => j(v, &w), None => v.clone() })
                        })
                        .collect(),
                    rest: x.rest.clone(),
                },
            },
            _ => a.clone(),
        }
    }
    let below = !top && !keep_gradual();
    let unresolved = top && QUIESCENT.with(|q| q.get());
    match t {
        Ty::Var { .. } if unresolved => Some(Ty::unresolved()),
        Ty::Var { .. } if below => Some(Ty::pending_untyped()),
        Ty::Union { variants } => {
            let mut changed = false;
            let mut arms: Vec<Ty> = Vec::with_capacity(variants.len());
            for v in variants.iter() {
                if matches!(v, Ty::Var { .. }) {
                    changed = true;
                    if unresolved {
                        arms.push(Ty::unresolved());
                    } else if below {
                        arms.push(Ty::pending_untyped());
                    }
                    continue;
                }
                match norm_opt(v, top) {
                    // A nested union (built outside `union_of`) is spliced, so
                    // equal arm sets have one form.
                    Some(Ty::Union { variants: inner }) => {
                        changed = true;
                        arms.extend(inner.iter().cloned());
                    }
                    Some(n) => {
                        changed = true;
                        arms.push(n);
                    }
                    None if matches!(v, Ty::Union { .. }) => {
                        changed = true;
                        if let Ty::Union { variants: inner } = v {
                            arms.extend(inner.iter().cloned());
                        }
                    }
                    None => arms.push(v.clone()),
                }
            }
            // A Tuple is a fixed-length Array: beside an Array arm it folds
            // into that arm's element (`Array[e] ⊔ Tuple[a, b] = Array[e | a |
            // b]`). An array literal is typed as a Tuple when its elements
            // differ and as an Array when they agree, so one literal flips
            // shape as its elements resolve (breaker's g2_const); without
            // this the join keeps the stale shape beside the final one.
            if let Some(ai) = arms.iter().position(|v| matches!(v, Ty::Array { .. }))
                && arms.iter().any(|v| matches!(v, Ty::Tuple { .. }))
            {
                let mut elem = match &arms[ai] {
                    Ty::Array { elem } => (**elem).clone(),
                    _ => unreachable!(),
                };
                for v in arms.iter() {
                    if let Ty::Tuple { elems } = v {
                        for e in elems.iter() {
                            elem = lat_join(elem, e.clone());
                        }
                    }
                }
                arms.retain(|v| !matches!(v, Ty::Tuple { .. }));
                if let Some(a) = arms.iter_mut().find(|v| matches!(v, Ty::Array { .. })) {
                    *a = Ty::Array { elem: std::sync::Arc::new(elem) };
                }
                changed = true;
            }
            let mut i = 0;
            while i < arms.len() {
                let mut k = i + 1;
                while k < arms.len() {
                    if same_shape(&arms[i], &arms[k]) {
                        let b = arms.remove(k);
                        arms[i] = pointwise(&arms[i], &b);
                        changed = true;
                    } else {
                        k += 1;
                    }
                }
                i += 1;
            }
            if !keep_gradual()
                && arms.iter().any(|v| matches!(v, Ty::Untyped { .. }))
                && arms.iter().any(|v| !matches!(v, Ty::Untyped { .. } | Ty::Var { .. } | Ty::Bottom | Ty::Nil))
            {
                arms.retain(|v| !matches!(v, Ty::Untyped { .. })
                    || (unresolved && v.provenance() == Some(crate::ty::Provenance::Unresolved)));
                changed = true;
            }
            if !changed {
                return None;
            }
            Ty::canonicalize_variants(&mut arms);
            arms.dedup();
            Some(match arms.len() {
                0 => Ty::Var { var: crate::ident::TyVar(0) },
                1 => arms.pop().unwrap(),
                _ => Ty::Union { variants: arms.into() },
            })
        }
        Ty::Array { elem } => norm_opt(elem, false).map(|e| Ty::Array { elem: std::sync::Arc::new(e) }),
        Ty::Hash { key, value } => {
            let (k, v) = (norm_opt(key, false), norm_opt(value, false));
            if k.is_none() && v.is_none() {
                return None;
            }
            Some(Ty::Hash {
                key: std::sync::Arc::new(k.unwrap_or_else(|| (**key).clone())),
                value: std::sync::Arc::new(v.unwrap_or_else(|| (**value).clone())),
            })
        }
        Ty::Tuple { elems } => each(elems).map(|e| Ty::Tuple { elems: e.into() }),
        Ty::Class { id, args } => each(args).map(|a| Ty::Class { id: id.clone(), args: a.into() }),
        Ty::Record { row } => {
            let vals: Vec<Ty> = row.fields.iter().map(|(_, v)| v.clone()).collect();
            each(&vals).map(|nv| Ty::Record {
                row: crate::ty::Row {
                    fields: row.fields.iter().map(|(k, _)| k.clone()).zip(nv).collect(),
                    rest: row.rest.clone(),
                },
            })
        }
        _ => None,
    }
}

/// One value on its own, normalized as a join with ⊥ would leave it.
pub(crate) fn lat_norm(t: Ty) -> Ty {
    if QUIESCENT.with(|q| q.get()) {
        // The synthetic join identity is not an unresolved producer.
        return norm_opt(&t, true).unwrap_or(t);
    }
    lat_join(Ty::Var { var: crate::ident::TyVar(0) }, t)
}

#[cfg(test)]
mod lat_tests {
    use super::*;
    use crate::analyze::body::union_of;

    #[test]
    fn lat_join_is_order_free_on_pending_pairs() {
        let v = Ty::Var { var: crate::ident::TyVar(4) };
        assert_eq!(lat_join(v.clone(), Ty::unresolved()), lat_join(Ty::unresolved(), v.clone()));
        let a = crate::analyze::body::union_of(Ty::Str, Ty::unresolved());
        let b = crate::analyze::body::union_of(Ty::Str, v.clone());
        assert_eq!(lat_join(a.clone(), b.clone()), lat_join(b, a));
        let nil = crate::analyze::body::union_of(Ty::Nil, Ty::unresolved());
        assert_eq!(lat_join(nil.clone(), Ty::Nil), lat_join(Ty::Nil, nil));
    }

    #[test]
    fn quiescent_variables_stay_unresolved_but_the_identity_does_not() {
        let v = Ty::Var { var: crate::ident::TyVar(0) };
        let nil = union_of(Ty::Nil, v.clone());
        assert_eq!(lat_norm(nil.clone()), Ty::Nil);
        {
            let _scope = quiescent_scope();
            assert_eq!(lat_norm(nil), union_of(Ty::Nil, Ty::unresolved()));
            assert_eq!(lat_norm(v.clone()), Ty::unresolved());
            assert_eq!(lat_norm(Ty::Int), Ty::Int);
            assert_eq!(lat_join(Ty::Str, v), union_of(Ty::Str, Ty::unresolved()));
            assert_eq!(lat_norm(union_of(Ty::Str, Ty::unresolved())),
                union_of(Ty::Str, Ty::unresolved()));
        }
        assert_eq!(lat_norm(union_of(Ty::Nil,
            Ty::Var { var: crate::ident::TyVar(0) })), Ty::Nil);
    }
}

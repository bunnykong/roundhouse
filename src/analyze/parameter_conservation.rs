//! Opt-in native parameter join. Unknown contributions are possible values,
//! not permission to discard them when another caller has a concrete type.
//! No per-writer completion is inferred from a bare legacy Var.
use crate::ty::Ty;
use std::sync::{Arc, OnceLock};

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_PARAMETER_REASON_CONSERVE").as_deref() == Ok("1"))
}

fn pident() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_PROVENANCE_PIDENT").as_deref() != Ok("0"))
}

/// provenance L2: an `untyped` whose only reason is Pending is the identity beside any other arm.
fn pending_only(t: &Ty) -> bool {
    matches!(t, Ty::Untyped { why } if *why == crate::ty::Provenance::Pending)
}

// provenance L3: initialization is a join with the fresh (pending) cell; no first-arrival rule.
pub(super) fn initialize(observed: Ty) -> Ty {
    if pident() {
        join(Ty::pending_untyped(), observed)
    } else if matches!(observed, Ty::Bottom) {
        Ty::pending_untyped()
    } else {
        observed
    }
}

/// Same container-spine union as the native join, without dropping unknowns.
/// Equal native shapes merge *all* reason bits even with RH_PROVENANCE_AUDIT disabled.
pub(super) fn join(a: Ty, b: Ty) -> Ty {
    fn push(t: Ty, out: &mut Vec<Ty>) {
        match t {
            Ty::Bottom => {}
            Ty::Union { variants } => {
                for t in variants {
                    push(t, out);
                }
            }
            Ty::Array { elem } => {
                if let Some(Ty::Array { elem: old }) =
                    out.iter_mut().find(|t| matches!(t, Ty::Array { .. }))
                {
                    *old = Arc::new(join((**old).clone(), Arc::unwrap_or_clone(elem)));
                } else {
                    out.push(Ty::Array { elem });
                }
            }
            Ty::Hash { key, value } => {
                if let Some(Ty::Hash {
                    key: old_key,
                    value: old_value,
                }) = out.iter_mut().find(|t| matches!(t, Ty::Hash { .. }))
                {
                    *old_key = Arc::new(join((**old_key).clone(), Arc::unwrap_or_clone(key)));
                    *old_value = Arc::new(join((**old_value).clone(), Arc::unwrap_or_clone(value)));
                } else {
                    out.push(Ty::Hash { key, value });
                }
            }
            t => {
                if let Some(old) = out.iter_mut().find(|old| **old == t) {
                    let chosen = Ty::canonical_min(old.clone(), t.clone());
                    let result = super::producer_evidence::merge_equal(chosen, &t);
                    // canonical_min may choose t's record order, so merge both inputs.
                    let result = super::producer_evidence::merge_equal(result, old);
                    super::producer_evidence::equal_merge(old, &t, &result);
                    *old = result;
                } else {
                    out.push(t);
                }
            }
        }
    }
    let mut variants = Vec::new();
    push(a, &mut variants);
    push(b, &mut variants);
    if pident() && variants.iter().any(|t| !pending_only(t)) {
        variants.retain(|t| !pending_only(t));
    }
    Ty::canonicalize_variants(&mut variants);
    match variants.len() {
        0 => Ty::Bottom,
        1 => variants.pop().unwrap(),
        _ => Ty::Union {
            variants: variants.into(),
        },
    }
}

#[cfg(test)]
mod tests;

//! protocol producer strictness and complete-read comparisons (spine v2 defaults).
use crate::ty::{Provenance, Ty};

pub(crate) fn stubs_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_PROVENANCE_STRICT_STUBS").as_deref() != Ok("0"))
}
pub(crate) fn xfer_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_PROVENANCE_STRICT_XFER").as_deref() != Ok("0"))
}
pub(crate) fn congruent_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_PROVENANCE_CONGRUENT").as_deref() != Ok("0"))
}
pub(crate) fn is_p(t: &Ty) -> bool {
    matches!(t, Ty::Untyped { why } if *why == Provenance::Pending)
}
/// No informative variant and no ambiguous Var: every leaf is explicitly pending.
pub(crate) fn only_pending(t: &Ty) -> bool {
    match t {
        Ty::Union { variants } => !variants.is_empty() && variants.iter().all(only_pending),
        _ => is_p(t),
    }
}
pub(crate) fn unresolved_from(t: &Ty) -> Ty {
    if xfer_on() && only_pending(t) { Ty::pending_untyped() } else { Ty::unresolved() }
}
pub(crate) fn stub() -> Ty {
    if stubs_on() { Ty::pending_untyped() } else { Ty::unresolved() }
}
/// The unknown variants removed by a strip operation contain P only; Nil/core variants do not contribute.
pub(crate) fn stripped_pending(inputs: &[&Ty]) -> bool {
    fn collect(t: &Ty, found: &mut bool) -> bool {
        match t {
            Ty::Union { variants } => variants.iter().all(|v| collect(v, found)),
            Ty::Untyped { .. } => { *found = true; is_p(t) }
            Ty::Var { .. } => false,
            _ => true,
        }
    }
    let mut found = false;
    xfer_on() && inputs.iter().all(|t| collect(t, &mut found)) && found
}
pub(crate) fn same(a: &Ty, b: &Ty) -> bool {
    if congruent_on() {
        crate::wire_ty::storage_types(std::slice::from_ref(a), std::slice::from_ref(b))
    } else { a == b }
}
pub(crate) fn same_types(a: &[Ty], b: &[Ty]) -> bool {
    if congruent_on() { crate::wire_ty::storage_types(a, b) } else { a == b }
}
pub(crate) fn same_row(a: Option<&Vec<Ty>>, b: Option<&Vec<Ty>>) -> bool {
    match (a, b) { (Some(a), Some(b)) => same_types(a, b), (None, None) => true, _ => false }
}
pub(crate) fn same_table<K: Eq + std::hash::Hash>(
    a: &std::collections::HashMap<K, Ty>, b: &std::collections::HashMap<K, Ty>,
) -> bool {
    a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(v, w)))
}

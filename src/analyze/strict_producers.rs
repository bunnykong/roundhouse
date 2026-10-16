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

// deferral-protocol: absence is decided only at the stratum boundary.
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
static STRATUM: AtomicU8 = AtomicU8::new(1);
static MINTED: AtomicU64 = AtomicU64::new(0);
static RESTORED: AtomicU64 = AtomicU64::new(0);
static STRATUM2_UNRESOLVED: AtomicU64 = AtomicU64::new(0);
static RS4_PENDING: AtomicU64 = AtomicU64::new(0);
static SETTLED: AtomicU64 = AtomicU64::new(0);
static STRATUM2_RAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn deferred_defer() -> &'static str {
    static MODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MODE.get_or_init(|| {
        let mode=std::env::var("RH_DEFERRED_DEFER").unwrap_or_else(|_| "0".into());
        assert!(matches!(mode.as_str(), "0" | "stub" | "all"), "invalid RH_DEFERRED_DEFER"); mode
    }).as_str()
}
pub(crate) fn deferred_stratify() -> bool {
    let on=std::env::var("RH_DEFERRED_STRATIFY").as_deref()==Ok("1");
    assert!(!on || deferred_defer()!="0", "stratification requires deferral"); on
}
pub(crate) fn deferred_ndedup() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_DEFERRED_NDEDUP").as_deref()==Ok("1"))
}
pub(crate) fn set_stratum(stratum: u8) { STRATUM.store(stratum,Ordering::Relaxed); }
pub(crate) fn deferred_deferred(site: &str) -> bool {
    STRATUM.load(Ordering::Relaxed)==1 && (deferred_defer()=="all" || (deferred_defer()=="stub" && site=="register"))
}
pub(crate) fn deferred_var_answer(site: &str) -> Ty {
    if let Some(t)=super::boundary_carry::answer(site) {return t}
    if deferred_deferred(site) {
        MINTED.fetch_add(1,Ordering::Relaxed); Ty::pending_untyped()
    } else {
        if STRATUM.load(Ordering::Relaxed)==2 { STRATUM2_UNRESOLVED.fetch_add(1,Ordering::Relaxed); }
        Ty::unresolved()
    }
}
pub(crate) fn deferred_reset() {
    set_stratum(1);
    for c in [&MINTED,&RESTORED,&STRATUM2_UNRESOLVED,&RS4_PENDING,&SETTLED] { c.store(0,Ordering::Relaxed); }
    STRATUM2_RAN.store(false,Ordering::Relaxed);
}
pub(crate) fn deferred_restored() { RESTORED.fetch_add(1,Ordering::Relaxed); }
pub(crate) fn deferred_rclosure_pending() { RS4_PENDING.fetch_add(1,Ordering::Relaxed); }
pub(crate) fn deferred_settled() { SETTLED.fetch_add(1,Ordering::Relaxed); }
pub(crate) fn deferred_start_second() { set_stratum(2); STRATUM2_RAN.store(true,Ordering::Relaxed); }
pub(crate) fn deferred_need_second() -> bool { MINTED.load(Ordering::Relaxed)>0 || RESTORED.load(Ordering::Relaxed)>0 }
pub(crate) fn deferred_report() -> serde_json::Value {
    let mut out=serde_json::json!({"defer":deferred_defer(),"stratify":deferred_stratify(),"ndedup":deferred_ndedup(),
        "stratum1_pending_minted":MINTED.load(Ordering::Relaxed),"boundary_restored":RESTORED.load(Ordering::Relaxed),
        "stratum2_ran":STRATUM2_RAN.load(Ordering::Relaxed),"stratum2_unresolved":STRATUM2_UNRESOLVED.load(Ordering::Relaxed),
        "rclosure_kept_pending":RS4_PENDING.load(Ordering::Relaxed),"settled_pending":SETTLED.load(Ordering::Relaxed)});
    out.as_object_mut().unwrap().extend(super::boundary_carry::report().as_object().unwrap().clone());out
}

/// An unbound or placeholder parameter is a deferred input, rather than an authored unknown.
pub(crate) fn deferred_seed_param(value: Option<Ty>) -> Option<Ty> {
    if deferred_deferred("param_seed") && value.as_ref().is_none_or(|t| matches!(t, Ty::Var { .. })) {
        Some(deferred_var_answer("param_seed"))
    } else { value }
}

pub(crate) fn deferred_seed_param_at(value:Option<Ty>,key:String)->Option<Ty> {
    if super::boundary_carry::second(){return super::boundary_carry::seed(key,value)}
    let _scope=super::boundary_carry::scope(key);
    deferred_seed_param(value)
}

//! Analysis-local reuse of canonical fingerprint components, never typed IR.
use super::*;
use std::rc::Rc;

pub(crate) struct Extra {
    pub(super) value: Rc<Value>,
    pub(super) bytes: Rc<Vec<u8>>,
    pub(super) original: Option<Value>,
}

#[derive(Default)]
struct Components {
    audit: bool,
    keys: Option<(Vec<ClassId>, Rc<Vec<u8>>)>,
    mode: Option<(u64, Rc<Vec<u8>>)>,
    extra: Option<(u64, Rc<Value>, Rc<Vec<u8>>)>,
    counters: BTreeMap<&'static str, u64>,
    peak_bytes: usize,
}

thread_local! {
    static COMPONENTS: RefCell<Components> = RefCell::new(Components::default());
}

pub(super) fn start() {
    COMPONENTS.with(|c| *c.borrow_mut() = Components {
        audit: std::env::var("RH_WARM_FP_AUDIT").as_deref() == Ok("1"),
        ..Components::default()
    });
}

#[cfg(test)]
pub(super) fn start_audit_for_test() {
    start();
    COMPONENTS.with(|c| c.borrow_mut().audit = true);
}

pub(super) fn audit() -> bool {
    COMPONENTS.with(|c| c.borrow().audit)
}

pub(super) fn count(key: &'static str, n: u64) {
    COMPONENTS.with(|c| *c.borrow_mut().counters.entry(key).or_default() += n);
}

pub(super) fn bytes(key: &'static str, value: &[u8]) {
    count(key, value.len() as u64);
}

fn retained(c: &Components) -> usize {
    c.keys.as_ref().map_or(0, |(_, b)| b.len())
        + c.mode.as_ref().map_or(0, |(_, b)| b.len())
        + c.extra.as_ref().map_or(0, |(_, _, b)| b.len())
}

fn peak(c: &mut Components) {
    c.peak_bytes = c.peak_bytes.max(retained(c));
}

pub(super) fn stop() -> Value {
    COMPONENTS.with(|c| {
        let c = c.replace(Components::default());
        serde_json::json!({
            "audit_enabled": c.audit, "counters": c.counters,
            "retained_canonical_bytes": retained(&c),
            "peak_retained_canonical_bytes": c.peak_bytes,
            "retained_class_key_count": c.keys.as_ref().map_or(0, |(k, _)| k.len()),
            "retained_extra_values": usize::from(c.extra.is_some()),
            "size_scope": "canonical buffers plus separately counted keys and one materialized extra Value",
        })
    })
}

/// Detect a generation by exact membership, including same-length replacements.
/// No registry pointer, hash collision, or assumed insertion-only lifetime is used.
pub(super) fn keys(classes: &HashMap<ClassId, ClassInfo>) -> Rc<Vec<u8>> {
    let _profile = phase(Phase::FpKeys);
    count("class_key_calls", 1);
    let hit = COMPONENTS.with(|c| {
        let c = c.borrow();
        c.keys.as_ref().filter(|(keys, _)| {
            keys.len() == classes.len() && keys.iter().all(|key| classes.contains_key(key))
        }).map(|(_, bytes)| bytes.clone())
    });
    if let Some(hit) = hit {
        count("class_key_generation_hits", 1);
        bytes("class_key_bytes_used", &hit);
        return hit;
    }
    let mut names: Vec<_> = classes.keys().cloned().collect();
    names.sort_unstable_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    let encoded = Rc::new(serde_json::to_vec(&names.iter().map(|n| n.0.as_str()).collect::<Vec<_>>()).unwrap());
    bytes("class_key_bytes_encoded", &encoded);
    bytes("class_key_bytes_used", &encoded);
    count("class_key_generations", 1);
    COMPONENTS.with(|c| {
        let mut c = c.borrow_mut();
        c.keys = Some((names, encoded.clone()));
        peak(&mut c);
    });
    encoded
}

pub(super) fn mode() -> Rc<Vec<u8>> {
    let _profile = phase(Phase::FpMetadata);
    count("metadata_calls", 1);
    let generation = super::super::fold::warm_structure_generation();
    let hit = COMPONENTS.with(|c| c.borrow().mode.as_ref()
        .filter(|(g, _)| *g == generation).map(|(_, b)| b.clone()));
    if let Some(hit) = hit {
        count("metadata_generation_hits", 1);
        bytes("metadata_bytes_used", &hit);
        return hit;
    }
    let encoded = Rc::new(serde_json::to_vec(&super::super::fold::warm_structure()).unwrap());
    bytes("metadata_bytes_encoded", &encoded);
    bytes("metadata_bytes_used", &encoded);
    count("metadata_generations", 1);
    COMPONENTS.with(|c| {
        let mut c = c.borrow_mut();
        c.mode = Some((generation, encoded.clone()));
        peak(&mut c);
    });
    encoded
}

/// Only Analyzer::body_typer supplies a generation: constants are replaced at
/// one audited helper (full-pass and test-only callers); factories/inquirers
/// are immutable after Analyzer::new.
/// Standalone BodyTypers and temporary constant-resolution maps never reuse extra.
pub(crate) fn extra(generation: Option<u64>, build: impl FnOnce() -> Value) -> Extra {
    let _profile = phase(Phase::FpExtra);
    count("extra_calls", 1);
    let hit = generation.and_then(|g| COMPONENTS.with(|c| c.borrow().extra.as_ref()
        .filter(|(old, _, _)| *old == g).map(|(_, v, b)| (v.clone(), b.clone()))));
    if let Some((value, encoded)) = hit {
        count("extra_generation_hits", 1);
        bytes("extra_bytes_used", &encoded);
        let original = if audit() {
            let _profile = phase(Phase::FpAudit);
            let original = build();
            assert_eq!(serde_json::to_vec(&original).unwrap(), *encoded, "RH_WARM_FP_AUDIT: extra bytes");
            count("extra_audit_comparisons", 1);
            Some(original)
        } else { None };
        return Extra { value, bytes: encoded, original };
    }
    let value = Rc::new(build());
    let encoded = Rc::new(serde_json::to_vec(value.as_ref()).unwrap());
    bytes("extra_bytes_encoded", &encoded);
    bytes("extra_bytes_used", &encoded);
    count("extra_generations", 1);
    if let Some(generation) = generation {
        COMPONENTS.with(|c| {
            let mut c = c.borrow_mut();
            c.extra = Some((generation, value.clone(), encoded.clone()));
            peak(&mut c);
        });
    }
    let original = audit().then(|| {
        count("extra_audit_comparisons", 1);
        value.as_ref().clone()
    });
    Extra { value, bytes: encoded, original }
}

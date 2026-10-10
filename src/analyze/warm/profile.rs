//! Exclusive wall-time accounting for the existing warm path.
//!
//! Scopes may nest: a fingerprint inside a guard is charged to
//! fingerprinting, rather than counted again as guard validation.
//! These counters never participate in cache validity or scheduling.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Other,
    Load,
    Fingerprint,
    Guard,
    Replay,
    Fresh,
    Record,
    ShadowClone,
    ValueFingerprint,
    ClassFingerprint,
    InputFingerprint,
    Cleanup,
}

const NAMES: [&str; 12] = [
    "other", "load_decode", "fingerprints", "guards", "replay", "fresh_typing", "recording",
    "shadow_clone",
    "value_fingerprints", "class_fingerprints", "input_fingerprints",
    "old_trace_cleanup",
];
static ENABLED: AtomicBool = AtomicBool::new(false);

struct Timeline {
    active: Phase,
    since: Instant,
    seconds: [Duration; 12],
    scopes: [u64; 12],
}

impl Timeline {
    fn switch(&mut self, phase: Phase) -> Phase {
        let now = Instant::now();
        self.seconds[self.active as usize] += now.duration_since(self.since);
        self.since = now;
        std::mem::replace(&mut self.active, phase)
    }
}

thread_local! {
    static TIMELINE: RefCell<Option<Timeline>> = const { RefCell::new(None) };
}

pub(super) fn start() {
    let enabled = std::env::var("RH_WARM_TIMINGS").as_deref() == Ok("1");
    TIMELINE.with(|t| {
        *t.borrow_mut() = enabled.then(|| Timeline {
            active: Phase::Other,
            since: Instant::now(),
            seconds: [Duration::ZERO; 12],
            scopes: [0; 12],
        });
    });
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub(crate) struct Scope(Phase);

pub(crate) fn phase(phase: Phase) -> Option<Scope> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    TIMELINE.with(|t| {
        let mut t = t.borrow_mut();
        let t = t.as_mut()?;
        t.scopes[phase as usize] += 1;
        Some(Scope(t.switch(phase)))
    })
}

impl Drop for Scope {
    fn drop(&mut self) {
        TIMELINE.with(|t| {
            if let Some(t) = t.borrow_mut().as_mut() {
                t.switch(self.0);
            }
        });
    }
}

pub(super) fn stop() -> Option<serde_json::Value> {
    ENABLED.store(false, Ordering::Relaxed);
    TIMELINE.with(|t| {
        let mut t = t.borrow_mut().take()?;
        t.switch(Phase::Other);
        let seconds: std::collections::BTreeMap<_, _> = NAMES.iter().copied()
            .zip(t.seconds.iter().map(Duration::as_secs_f64)).collect();
        let scopes: std::collections::BTreeMap<_, _> = NAMES.iter().copied()
            .zip(t.scopes).collect();
        Some(serde_json::json!({
            "schema": 1,
            "scope": "warm-start-through-stop-exclusive-wall",
            "seconds": seconds,
            "scope_counts": scopes,
            "total_seconds": t.seconds.iter().sum::<Duration>().as_secs_f64(),
        }))
    })
}

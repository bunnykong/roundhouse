//! Seeded worklist priorities, copied from spine de7589e4.
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
static SHUFFLE: LazyLock<Option<u64>> = LazyLock::new(|| {
    std::env::var("RH_SHUFFLE")
        .ok()
        .and_then(|v| v.trim().parse().ok())
});
static SHUFFLED_POPS: AtomicU64 = AtomicU64::new(0);
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

pub(crate) fn reset() {
    SHUFFLED_POPS.store(0, Relaxed);
}
pub(crate) fn report() {
    if let Some(seed) = *SHUFFLE {
        eprintln!(
            "rh-shuffle: {}",
            serde_json::json!({
                "seed": seed, "keyed_pushes": SHUFFLED_POPS.load(Relaxed),
                "hash_keys": "platform-default"
            })
        );
    }
}

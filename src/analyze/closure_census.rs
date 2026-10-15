//! Differential, per-harvest receipts. Shadow walks never touch analyzer state.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use crate::{App, ident::Symbol, ty::Ty, span::Span};
use super::{IvarWalk, ClosureHarvestMode, ClosureWriteTag};

#[derive(Default)]
struct Census {
    active: bool,
    walks: u64,
    legacy: u64,
    entrances: u64,
    new_write: u64,
    bound_only: u64,
    unexplained: u64,
    entrances_unexplained: u64,
    sites: Vec<(Span, ClosureWriteTag, Symbol)>,
}
thread_local! { static CENSUS: RefCell<Census> = RefCell::new(Census::default()); }

pub(super) fn start() {
    CENSUS.with(|c| *c.borrow_mut() = Census { active: true, ..Census::default() });
}

// Include provenance even if the value's ordinary equality intentionally ignores it.
fn equal(a: Option<&Ty>, b: Option<&Ty>) -> bool {
    a == b && format!("{a:?}") == format!("{b:?}")
}

pub(super) fn note(out: &HashMap<Symbol, Ty>, legacy: &HashMap<Symbol, Ty>,
                   entrances: &HashMap<Symbol, Ty>, walk: &IvarWalk) {
    CENSUS.with(|c| {
        let mut c = c.borrow_mut();
        if !c.active { return; }
        c.walks += 1;
        let keys: BTreeSet<_> = out.keys().chain(legacy.keys()).chain(entrances.keys()).collect();
        let mut changed_legacy = false;
        let mut changed_entrances = false;
        for name in keys {
            if !equal(out.get(name), legacy.get(name)) {
                changed_legacy = true;
                if walk.new_writes.contains(name) { c.new_write += 1; }
                else if walk.reflective.contains(name) && legacy.get(name).is_some_and(|ty|
                    equal(out.get(name), Some(&super::fixpoint_bound::bound(ty.clone())))) {
                    c.bound_only += 1;
                } else { c.unexplained += 1; }
            }
            if walk.mode == ClosureHarvestMode::Apply && !equal(out.get(name), entrances.get(name)) {
                changed_entrances = true;
                if !walk.applied_writes.contains(name) { c.entrances_unexplained += 1; }
            }
        }
        c.legacy += u64::from(changed_legacy);
        c.entrances += u64::from(changed_entrances);
        c.sites.extend(walk.sites.iter().cloned());
    });
}

pub(super) fn report(app: &App) -> Option<serde_json::Value> {
    let mode = ClosureHarvestMode::env();
    if mode == ClosureHarvestMode::Legacy || !super::fixpoint_check::stats_on() { return None; }
    CENSUS.with(|c| {
        let mut c = c.borrow_mut();
        c.active = false; // lower/emission harvests are outside this report and the value digests.
        let mut sites = BTreeMap::new();
        for (span, tag, slot) in &c.sites {
            let Some(source) = span.file.0.checked_sub(1).and_then(|i| app.sources.get(i as usize)) else { continue };
            let line = source.line_col(span.start).0;
            let (tag, consumer) = match tag {
                ClosureWriteTag::Legacy => continue,
                ClosureWriteTag::Entrance => ("E", None),
                ClosureWriteTag::Applied(kind) => ("I", Some(*kind)),
            };
            sites.entry((source.path.clone(), line, tag, consumer)).or_insert(slot.to_string());
        }
        let mut by = BTreeMap::from([("receiver", 0u64), ("block", 0), ("argument", 0), ("local", 0)]);
        let mut entrance = 0;
        let public_sites = std::env::var("RH_PUBLIC_INPUT").as_deref() == Ok("1")
            && std::env::var("RH_CLOSURE_SITES").as_deref() == Ok("1");
        for ((file, line, tag, consumer), slot) in &sites {
            if *tag == "E" { entrance += 1; }
            else if let Some(kind) = consumer { *by.get_mut(kind).unwrap() += 1; }
            if public_sites {
                eprintln!("rh-closure_harvest-site: {}", serde_json::json!({
                    "tag": tag, "consumer": consumer, "file": file, "line": line, "slot": slot
                }));
            }
        }
        Some(serde_json::json!({
            "mode": if mode == ClosureHarvestMode::Apply { "apply" } else { "entrances" },
            "walks": c.walks,
            "walks_changed_vs_legacy": c.legacy,
            "walks_changed_vs_entrances": c.entrances,
            "slots_changed_new_write": c.new_write,
            "slots_changed_bound_only": c.bound_only,
            "slots_changed_unexplained": c.unexplained,
            "slots_changed_vs_entrances_unexplained": c.entrances_unexplained,
            "sites_entrance": entrance,
            "sites_applied": by.values().sum::<u64>(),
            "sites_applied_by_consumer": by,
        }))
    })
}

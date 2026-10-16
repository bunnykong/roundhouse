//! RETURN_HELPERS: current raw writer cells, a conservative aggregate, then one reader view.
//! Destination allocation stays in fold; this ledger never allocates a slot.
//! Captured with RH_CONTRIBUTOR_VALUES independently of RH_PROVENANCE_AUDIT.
use super::{
    body::ClassInfo,
    fold::{self, SlotKey},
    parameter_conservation as pc,
};
use crate::{
    ident::ClassId,
    span::{SourceFile, Span},
    ty::Ty,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::{Arc, LazyLock},
};

static ON: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_PARAMETER_VIEWS").as_deref() == Ok("1"));
static CAPTURE: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_CONTRIBUTOR_VALUES").as_deref() == Ok("1"));
pub(super) fn on() -> bool {
    *ON
}
fn enabled() -> bool {
    *ON || *CAPTURE
}

#[derive(Clone)]
struct Destination {
    cells: BTreeMap<String, Ty>,
    aggregate: Ty,
    absent: BTreeSet<String>,
}
#[derive(Default)]
struct Ledger {
    sources: Vec<String>,
    declared: BTreeSet<String>,
    observed: BTreeSet<String>,
    destinations: BTreeMap<String, Destination>,
    superseded: u64,
    unscoped: u64,
    history_joins: u64,
    // Preserve all observed storage descents for resolved classification at the end.
    transitions: Vec<(Ty, Ty)>,
}
thread_local! { static LEDGER: RefCell<Ledger> = RefCell::new(Ledger::default()); }

pub(super) fn reset(sources: &[SourceFile]) {
    if enabled() {
        LEDGER.with(|x| {
            *x.borrow_mut() = Ledger {
                sources: sources.iter().map(|s| s.path.clone()).collect(),
                ..Ledger::default()
            }
        });
    }
}
fn destination(key: &SlotKey) -> String {
    format!("Some({key:?})")
}
fn writer(span: Span, sources: &[String]) -> String {
    let path = span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| sources.get(i as usize));
    format!(
        "{}:{}-{}",
        path.map(String::as_str).unwrap_or("<synthetic>"),
        span.start,
        span.end
    )
}
fn identity(key: &SlotKey, span: Span, sources: &[String]) -> String {
    format!("{}|{}", destination(key), writer(span, sources))
}
/// Independent declaration at the formal enumeration site, before looking for a row.
pub(super) fn declare(key: &SlotKey, span: Span) {
    if enabled() {
        LEDGER.with(|x| {
            let mut l = x.borrow_mut();
            let id = identity(key, span, &l.sources);
            l.declared.insert(id);
        });
    }
}
fn current_join(a: Ty, b: Ty) -> Ty {
    // Pending is the ledger identity even beside Bottom; the native row join
    // otherwise keeps a Pending-only initialization when Bottom arrives.
    if pending(&a) {
        return pc::normalize(b);
    }
    if pending(&b) {
        return pc::normalize(a);
    }
    pc::join(a, b)
}
fn aggregate<'a>(cells: impl Iterator<Item = &'a Ty>) -> Ty {
    pc::normalize(cells.cloned().fold(Ty::pending_untyped(), current_join))
}
pub(super) fn view(t: Ty) -> Ty {
    super::provenance_reader_row(std::slice::from_ref(&t))[0].clone()
}
/// Replace one writer, never join against its previous contribution or a view output.
pub(super) fn replace(key: &SlotKey, span: Span, raw: Option<Ty>) -> Ty {
    if !enabled() {
        return raw.unwrap_or_else(Ty::pending_untyped);
    }
    LEDGER.with(|x| {
        let mut l = x.borrow_mut();
        let id = identity(key, span, &l.sources);
        let w = writer(span, &l.sources);
        if span.is_synthetic() {
            l.unscoped += 1;
        }
        l.observed.insert(id);
        let d = l
            .destinations
            .entry(destination(key))
            .or_insert_with(|| Destination {
                cells: BTreeMap::new(),
                aggregate: Ty::pending_untyped(),
                absent: BTreeSet::new(),
            });
        if raw.is_none() {
            d.absent.insert(w.clone());
        } else {
            d.absent.remove(&w);
        }
        let t = raw.unwrap_or_else(Ty::pending_untyped);
        let superseded = d.cells.get(&w).is_some_and(|old| !same(old, &t));
        d.cells.insert(w, t);
        d.aggregate = aggregate(d.cells.values());
        let out = d.aggregate.clone();
        if superseded {
            l.superseded += 1;
        }
        out
    })
}
fn origins(t: &Ty) -> BTreeSet<String> {
    super::producer_evidence::origins(t)
        .into_iter()
        .map(|s| s.replace("/arm", ""))
        .filter(|s| !s.ends_with(":ReferenceRequiresDependencyAudit"))
        .collect()
}
fn same(a: &Ty, b: &Ty) -> bool {
    a == b && origins(a) == origins(b)
}
fn pending(t: &Ty) -> bool {
    matches!(t,Ty::Untyped { why } if *why==crate::ty::Provenance::Pending)
}
pub(super) fn note_storage(before: Option<Ty>, after: Option<Ty>, history_join: bool) {
    if !enabled() {
        return;
    }
    LEDGER.with(|x| {
        let mut l = x.borrow_mut();
        if history_join {
            l.history_joins += 1;
        }
        if let (Some(a), Some(b)) = (before, after) {
            if !same(&a, &b) {
                l.transitions.push((a, b));
            }
        }
    });
}

/// Conservative symbolic resolution for this audit. Unlike the ordinary final
/// expander, resolving a union must not itself project away raw unknown evidence.
/// Cycles retain their symbolic back edge; missing dependencies or an exhausted
/// audit budget make coverage incomplete rather than inventing a successful audit.
struct Resolver<'a> {
    classes: &'a HashMap<ClassId, ClassInfo>,
    stack: HashSet<u32>,
    budget: usize,
    complete: bool,
}
impl Resolver<'_> {
    fn go(&mut self, t: &Ty) -> Ty {
        if self.budget == 0 {
            self.complete = false;
            return t.clone();
        }
        self.budget -= 1;
        match t {
            Ty::Rec { slot } => {
                if !self.stack.insert(*slot) {
                    return t.clone();
                }
                let r = if let Some(v) = fold::value_of(*slot, self.classes) {
                    self.go(&v)
                } else {
                    self.complete = false;
                    t.clone()
                };
                self.stack.remove(slot);
                r
            }
            Ty::Union { variants } => pc::normalize(
                variants
                    .iter()
                    .map(|v| self.go(v))
                    .fold(Ty::pending_untyped(), current_join),
            ),
            Ty::Array { elem } => Ty::Array {
                elem: Arc::new(self.go(elem)),
            },
            Ty::Hash { key, value } => Ty::Hash {
                key: Arc::new(self.go(key)),
                value: Arc::new(self.go(value)),
            },
            Ty::Tuple { elems } => Ty::Tuple {
                elems: elems.iter().map(|t| self.go(t)).collect(),
            },
            Ty::Class { id, args } => Ty::Class {
                id: id.clone(),
                args: args.iter().map(|t| self.go(t)).collect(),
            },
            Ty::Record { row } => Ty::Record {
                row: crate::ty::Row {
                    fields: row
                        .fields
                        .iter()
                        .map(|(k, t)| (k.clone(), self.go(t)))
                        .collect(),
                    rest: row.rest,
                },
            },
            Ty::Fn {
                params,
                block,
                ret,
                effects,
            } => Ty::Fn {
                params: params
                    .iter()
                    .map(|p| crate::ty::Param {
                        name: p.name.clone(),
                        kind: p.kind.clone(),
                        ty: Arc::new(self.go(&p.ty)),
                    })
                    .collect(),
                block: block.as_ref().map(|b| Arc::new(self.go(b))),
                ret: Arc::new(self.go(ret)),
                effects: effects.clone(),
            },
            _ => t.clone(),
        }
    }
}
fn resolved(t: &Ty, classes: &HashMap<ClassId, ClassInfo>) -> (Ty, bool) {
    let mut r = Resolver {
        classes,
        stack: HashSet::new(),
        budget: 100_000,
        complete: true,
    };
    let t = r.go(t);
    (t, r.complete)
}
fn record(t: &Ty, classes: &HashMap<ClassId, ClassInfo>, complete: &mut bool) -> Value {
    let (r, ok) = resolved(t, classes);
    *complete &= ok;
    json!({"ty":super::fixpoint_check::contributor_ty(t),"origins":origins(t),
        "resolved_ty":super::fixpoint_check::contributor_ty(&r),"resolved_origins":origins(&r)})
}

pub(super) fn report(classes: &HashMap<ClassId, ClassInfo>) {
    if !*CAPTURE {
        return;
    }
    let (declared, observed, destinations, superseded, unscoped, history, transitions) = LEDGER
        .with(|x| {
            let l = x.borrow();
            (
                l.declared.clone(),
                l.observed.clone(),
                l.destinations.clone(),
                l.superseded,
                l.unscoped,
                l.history_joins,
                l.transitions.clone(),
            )
        });
    let actual: HashMap<_, _> = fold::contributor_values().into_iter().collect();
    let mut slots = Vec::new();
    let mut complete = true;
    let (mut loss, mut pending_eliminations, mut projections) = (0u64, 0u64, 0u64);
    let (mut many, mut pending_slots, mut pident) = (0u64, 0u64, 0u64);
    let (mut aggregate_mismatches, mut storage_mismatches) = (0u64, 0u64);
    let mut premature_unresolved = 0u64;
    for (key, d) in &destinations {
        let replay = aggregate(d.cells.values().rev());
        premature_unresolved += d
            .cells
            .iter()
            .filter(|(w, t)| {
                d.absent.contains(*w) && origins(t).iter().any(|o| o.ends_with(":Unresolved"))
            })
            .count() as u64;
        let stored = actual.get(key);
        let expected = stored.map(|_| view(d.aggregate.clone()));
        if !same(&d.aggregate, &replay) {
            aggregate_mismatches += 1;
        }
        if let (Some(s), Some(e)) = (stored, &expected) {
            if !same(s, e) {
                storage_mismatches += 1;
            }
        }
        many += u64::from(d.cells.len() > 1);
        pending_slots += u64::from(stored.is_some() && pending(&d.aggregate));
        pident += d
            .cells
            .values()
            .filter(|t| pending(t) && !pending(&d.aggregate))
            .count() as u64;
        let mut current_origins = BTreeSet::new();
        let cells: Vec<_> = d
            .cells
            .iter()
            .map(|(writer, t)| {
                let v = record(t, classes, &mut complete);
                current_origins.extend(
                    v["resolved_origins"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|s| s.as_str().unwrap().to_string()),
                );
                json!({"writer":writer,"raw":v})
            })
            .collect();
        let raw_record = record(&d.aggregate, classes, &mut complete);
        let raw_origins: BTreeSet<String> = raw_record["resolved_origins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().into())
            .collect();
        for o in current_origins.difference(&raw_origins) {
            if o.ends_with(":Pending") {
                pending_eliminations += 1;
            } else {
                loss += 1;
            }
        }
        let expected_record = expected.as_ref().map(|t| record(t, classes, &mut complete));
        if let Some(v) = &expected_record {
            let viewed: BTreeSet<String> = v["resolved_origins"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().into())
                .collect();
            projections += raw_origins.difference(&viewed).count() as u64;
        }
        slots.push(json!({"destination":key,"materialized":stored.is_some(),"cells":cells,
            "raw_aggregate":raw_record,"replayed_aggregate":record(&replay,classes,&mut complete),
            "stored":stored.map(|t|record(t,classes,&mut complete)),"expected_stored":expected_record}));
    }
    let descents:Vec<_>=transitions.into_iter().filter_map(|(a,b)| {
        let (ra,ok_a)=resolved(&a,classes);let (rb,ok_b)=resolved(&b,classes);complete &= ok_a && ok_b;
        if same(&current_join(ra.clone(),rb.clone()),&rb) {return None;}
        Some(json!({"before":record(&ra,classes,&mut complete),"after":record(&rb,classes,&mut complete),"count":1}))
    }).collect();
    let mut ownership = declared.iter().cloned().collect::<Vec<_>>();
    ownership.sort();
    let ledger = json!({"available":true,"resolved":complete,"slot_identity":"symbolic",
        "declared_writers":declared.len(),"observed_writers":observed.len(),
        "undeclared_observed":observed.difference(&declared).count(),"unscoped_seeds":unscoped,
        "history_joins":history,"aggregate_mismatches":aggregate_mismatches,"storage_mismatches":storage_mismatches,
        "premature_unresolved_contributions":premature_unresolved,"superseded_contributions":superseded,
        "pending_identity_eliminations":pident,"pending_only_slots":pending_slots,"slots_with_many_writers":many});
    let audit = json!({"origin_loss_current":loss,"origin_coverage_complete":complete && declared==observed,
        "origin_pending_eliminations":pending_eliminations,"origin_view_projections":projections});
    let snapshot = json!({"ledger":ledger,"audit":audit,"declared":declared,"observed":observed,
        "slots":slots,"ownership":ownership,"descents":descents});
    let path = std::path::PathBuf::from(std::env::var_os("RH_STRUCT_DUMP").expect("observer path"))
        .with_file_name("schedule-row-views.json");
    let out = std::io::BufWriter::new(std::fs::File::create(path).expect("RETURN_HELPERS snapshot"));
    serde_json::to_writer(out, &snapshot).expect("write RETURN_HELPERS snapshot");
    eprintln!("rh-schedule-row-views: {}", json!({"ledger":ledger,"audit":audit}));
}

#[cfg(test)]
mod tests;

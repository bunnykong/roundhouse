//! Bounded observational producer ledger for move 3. No scheduler or writer policy changes.
//! Identities use definition owner, receiver side, method and source position.
use crate::{
    ident::{ClassId, Symbol},
    span::Span,
    ty::Ty,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_D3_PROV").as_deref() == Ok("1"))
}

#[derive(Default)]
struct Ledger {
    sources: Vec<String>,
    current: Option<String>,
    writers: BTreeMap<String, BTreeSet<String>>,
    shapes: BTreeMap<String, BTreeSet<String>>,
    parameter_routes: BTreeMap<String, BTreeSet<String>>,
    visits: BTreeMap<String, u64>,
    unscoped_inputs: u64,
    equal_merges: u64,
    tag_only_changes: u64,
    parameter_merges: u64,
    parameter_type_origin_losses: u64,
    parameter_losses_by_kind: BTreeMap<String, u64>,
    parameter_current: Option<(String, String)>,
    parameter_destinations: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    parameter_events: BTreeMap<String, u64>,
    parameter_output_origin_losses: u64,
    parameter_repair_merges: u64,
    parameter_seed_initializations: u64,
    parameter_unmapped_merges: u64,
}
thread_local! { static AUDIT: RefCell<Ledger> = RefCell::new(Ledger::default()); }

pub(super) fn reset(sources: &[crate::span::SourceFile]) {
    if on() {
        AUDIT.with(|a| {
            *a.borrow_mut() = Ledger {
                sources: sources.iter().map(|s| s.path.clone()).collect(),
                ..Ledger::default()
            }
        });
    }
}

pub(super) struct WriterGuard {
    previous: Option<Option<String>>,
    _current: super::current_harvest::Guard,
}
impl Drop for WriterGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            AUDIT.with(|a| a.borrow_mut().current = previous);
        }
    }
}
pub(super) fn writer(
    owner: &ClassId,
    method: &Symbol,
    class_side: bool,
    span: Span,
) -> WriterGuard {
    let current = super::current_harvest::writer(owner, method, class_side, span);
    if !on() {
        return WriterGuard { previous: None, _current: current };
    }
    AUDIT.with(|a| {
        let mut a = a.borrow_mut();
        let path = span
            .file
            .0
            .checked_sub(1)
            .and_then(|i| a.sources.get(i as usize))
            .cloned()
            .unwrap_or_else(|| "<synthetic-unmapped>".into());
        let id = format!(
            "owner={0}|includer={0}|{1}|{2}|{3}:{4}-{5}",
            owner.0.as_str(),
            if class_side { "class" } else { "instance" },
            method.as_str(),
            path,
            span.start,
            span.end
        );
        let previous = a.current.replace(id.clone());
        a.writers.entry(id).or_default();
        WriterGuard { previous: Some(previous), _current: current }
    })
}

/// Each distinct origin is retained independently of ordinary equality/serde.
/// Var is deliberately called ambiguous: this pilot does not infer pending from Var(0).
pub fn origins(t: &Ty) -> BTreeSet<String> {
    fn visit(t: &Ty, path: String, out: &mut BTreeSet<String>) {
        match t {
            Ty::Untyped { why } => {
                for reason in why.reasons() {
                    out.insert(format!("{path}:{reason}"));
                }
            }
            Ty::Var { .. } => {
                out.insert(format!("{path}:LegacyAmbiguousVar"));
            }
            Ty::Array { elem } => visit(elem, format!("{path}/element"), out),
            Ty::Hash { key, value } => {
                visit(key, format!("{path}/key"), out);
                visit(value, format!("{path}/value"), out);
            }
            Ty::Tuple { elems } | Ty::Class { args: elems, .. } => {
                for (i, t) in elems.iter().enumerate() {
                    visit(t, format!("{path}/{i}"), out);
                }
            }
            Ty::Union { variants } => {
                for t in variants {
                    visit(t, format!("{path}/arm"), out);
                }
            }
            Ty::Record { row } => {
                for (k, t) in row.fields.iter() {
                    visit(t, format!("{path}/{}", k.as_str()), out);
                }
            }
            Ty::Fn {
                params, block, ret, ..
            } => {
                for (i, p) in params.iter().enumerate() {
                    visit(&p.ty, format!("{path}/param/{i}"), out);
                }
                if let Some(b) = block {
                    visit(b, format!("{path}/block"), out);
                }
                visit(ret, format!("{path}/return"), out);
            }
            Ty::Rec { .. } => {
                out.insert(format!("{path}:ReferenceRequiresDependencyAudit"));
            }
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    visit(t, "$".into(), &mut out);
    out
}

// Raw argument origins precede helper-channel conversion and keyword placement.
// These are source writer identities; complete destination routing waits for move 2.
pub(super) fn parameter_call(
    caller: Option<&ClassId>,
    receivers: &[ClassId],
    method: &Symbol,
    side: &str,
    span: Span,
    args: &[crate::expr::Expr],
) -> Vec<String> {
    if !on() {
        return Vec::new();
    }
    let mut sources = Vec::new();
    for (position, arg) in args.iter().enumerate() {
        let previous = AUDIT.with(|a| {
            let mut a = a.borrow_mut();
            let path = span
                .file
                .0
                .checked_sub(1)
                .and_then(|i| a.sources.get(i as usize))
                .cloned()
                .unwrap_or_else(|| "<synthetic-unmapped>".into());
            let mut targets: Vec<_> = receivers.iter().map(|c| c.0.as_str()).collect();
            targets.sort();
            targets.dedup();
            let id = format!(
                "parameter|owner={0}|includer={0}|{1}|{2}|arg={3}|{4}:{5}-{6}",
                caller.map(|c| c.0.as_str()).unwrap_or("<template>"),
                side,
                method.as_str(),
                position,
                path,
                span.start,
                span.end
            );
            a.parameter_routes
                .entry(id.clone())
                .or_default()
                .extend(targets.iter().map(|s| (*s).to_owned()));
            a.writers.entry(id.clone()).or_default();
            sources.push(id.clone());
            a.current.replace(id)
        });
        input(arg.ty.as_ref());
        AUDIT.with(|a| a.borrow_mut().current = previous);
    }
    sources
}

fn destination(key: &super::ParamKey, position: usize) -> String {
    format!("{}|{}|{:?}|param={position}", key.0.0.as_str(), key.1.as_str(), key.2)
}

fn native(t: &Ty) -> String {
    // Debug includes the complete native reason set, unlike serde and Display.
    format!("{t:?}")
}

pub(super) fn parameter_destination(
    key: &super::ParamKey, sources: &[String], positional: bool, args: &[Ty],
) {
    if !on() { return; }
    AUDIT.with(|a| {
        let mut a = a.borrow_mut();
        for (position, ty) in args.iter().enumerate() {
            let candidates = if positional {
                sources.get(position).cloned().into_iter().collect::<Vec<_>>()
            } else {
                // Keyword placement and splats can combine producers. Preserve
                // the entire candidate set rather than invent one exact source.
                sources.to_vec()
            };
            a.parameter_destinations.entry(destination(key, position)).or_default()
                .entry(native(ty)).or_default().extend(candidates);
        }
    });
}

pub(super) struct ParameterGuard(Option<Option<(String, String)>>);
impl Drop for ParameterGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            AUDIT.with(|a| a.borrow_mut().parameter_current = previous);
        }
    }
}
pub(super) fn parameter_context(key: &super::ParamKey, position: usize, phase: &str) -> ParameterGuard {
    if !on() { return ParameterGuard(None); }
    AUDIT.with(|a| ParameterGuard(Some(a.borrow_mut().parameter_current
        .replace((destination(key, position), phase.to_owned())))))
}

pub(super) fn parameter_seed(observed: &Ty, result: &Ty) {
    if !on() { return; }
    AUDIT.with(|a| a.borrow_mut().parameter_seed_initializations += 1);
    // The old allocation Var is an explicitly empty table seed, not a raw
    // caller contribution. Its discharge is counted separately and justified
    // by allocation metadata. Check the actual observed contribution normally.
    parameter_merge(&Ty::Bottom, observed, result);
}

pub(super) fn parameter_merge(stored: &Ty, observed: &Ty, result: &Ty) {
    if !on() {
        return;
    }
    fn kinds(t: &Ty) -> BTreeSet<String> {
        origins(t)
            .into_iter()
            .map(|s| s.rsplit(':').next().unwrap().to_owned())
            .collect()
    }
    let before = kinds(stored)
        .union(&kinds(observed))
        .cloned()
        .collect::<BTreeSet<_>>();
    let after = kinds(result);
    let paths = |t: &Ty| origins(t).into_iter()
        .map(|path| path.replace("/arm", ""))
        .collect::<BTreeSet<_>>();
    let input_paths = paths(stored).union(&paths(observed)).cloned().collect::<BTreeSet<_>>();
    let output_paths = paths(result);
    let lost_paths = input_paths.difference(&output_paths).cloned().collect::<Vec<_>>();
    AUDIT.with(|a| {
        let mut a = a.borrow_mut();
        let (target, phase) = a.parameter_current.clone()
            .unwrap_or(("<unscoped-native-join>".into(), "join".into()));
        if phase == "join" {
            a.parameter_merges += 1;
            if super::parameter_conservation::on() { a.parameter_repair_merges += 1; }
        }
        let lost = before.difference(&after).cloned().collect::<Vec<_>>();
        a.parameter_type_origin_losses += lost.len() as u64;
        for kind in &lost {
            *a.parameter_losses_by_kind.entry(kind.clone()).or_default() += 1;
        }
        a.parameter_output_origin_losses += lost_paths.len() as u64;
        let sources = a.parameter_destinations.get(&target)
            .and_then(|shapes| shapes.get(&native(observed))).cloned().unwrap_or_default();
        if phase == "join" && sources.is_empty() { a.parameter_unmapped_merges += 1; }
        let event = json!({"destination":target,"phase":phase,
            "observed_source_candidates":sources,"source_mapping":"post-placement native shape; keyword candidates may be overinclusive",
            "stored":native(stored),"observed":native(observed),"output":native(result),
            "input_origins":input_paths,"output_origins":output_paths,
            "lost_origins":lost_paths,"lost_kinds":lost,"removal_justification":null});
        *a.parameter_events.entry(event.to_string()).or_default() += 1;
    });
}

pub(super) fn input(t: Option<&Ty>) {
    if !on() {
        return;
    }
    let reasons = t
        .map(origins)
        .unwrap_or_else(|| BTreeSet::from(["$:MissingBodyEvidence".into()]));
    let shape = t
        .map(|t| serde_json::to_string(t).unwrap())
        .unwrap_or_else(|| "<missing>".into());
    AUDIT.with(|a| {
        let mut a = a.borrow_mut();
        if let Some(id) = a.current.clone() {
            a.shapes.entry(id.clone()).or_default().insert(shape);
            *a.visits.entry(id.clone()).or_default() += 1;
            a.writers.entry(id).or_default().extend(reasons);
        } else {
            a.unscoped_inputs += 1;
        }
    });
}

pub(crate) fn equal_merge(a: &Ty, b: &Ty, result: &Ty) {
    if !on() {
        return;
    }
    AUDIT.with(|s| {
        let mut s = s.borrow_mut();
        s.equal_merges += 1;
        if !crate::wire_ty::storage_types(std::slice::from_ref(a), std::slice::from_ref(result))
            || !crate::wire_ty::storage_types(std::slice::from_ref(b), std::slice::from_ref(result))
        {
            s.tag_only_changes += 1;
        }
    });
}

pub fn snapshot() -> Value {
    AUDIT.with(|a| {
        let a = a.borrow();
        json!({"schema":1,"scope":"direct return bodies and raw parameter producers; cumulative observational evidence",
            "writers":a.writers,"parameter_routes":a.parameter_routes,"input_shapes":a.shapes,"visits":a.visits,"unscoped_inputs":a.unscoped_inputs,
            "equal_merges":a.equal_merges,"tag_only_changes":a.tag_only_changes,"parameter_merges":a.parameter_merges,
            "parameter_type_origin_losses":a.parameter_type_origin_losses,"parameter_losses_by_kind":a.parameter_losses_by_kind,
            "parameter_repair_merges":a.parameter_repair_merges,
            "parameter_seed_initializations":a.parameter_seed_initializations,
            "parameter_seed_justification":"only a cell newly allocated by row resize is empty; observed Vars remain ambiguous; observed Bottom cannot complete the writer universe",
            "parameter_output_origin_losses":a.parameter_output_origin_losses,
            "parameter_unmapped_merges":a.parameter_unmapped_merges,
            "parameter_destinations":a.parameter_destinations,
            "parameter_native_events":a.parameter_events.iter().map(|(event,count)| {
                let mut event: Value = serde_json::from_str(event).unwrap();
                event["count"] = json!(count);
                event
            }).collect::<Vec<_>>(),
            "complete_writer_inventory":false,"scheduler_dependencies_migrated":false})
    })
}
pub(super) fn report() {
    if on() {
        eprintln!("rh-d3-prov: {}", snapshot());
    }
}

/// Merge tags recursively for types that already compare equal. The ordinary
/// type shape, record ordering, display, hash and serde contract stay unchanged.
pub(crate) fn merge_equal(a: Ty, b: &Ty) -> Ty {
    fn go(a: &Ty, b: &Ty, memo: &mut std::collections::HashMap<(usize, usize), Ty>) -> Ty {
        if std::ptr::eq(a, b) {
            return a.clone();
        }
        let key = (a as *const Ty as usize, b as *const Ty as usize);
        if let Some(t) = memo.get(&key) {
            return t.clone();
        }
        let result = match (a, b) {
            (Ty::Untyped { why: a }, Ty::Untyped { why: b }) => Ty::untyped(a.union(*b)),
            (Ty::Array { elem: a }, Ty::Array { elem: b }) => Ty::Array {
                elem: std::sync::Arc::new(go(a, b, memo)),
            },
            (Ty::Hash { key: ak, value: av }, Ty::Hash { key: bk, value: bv }) => Ty::Hash {
                key: std::sync::Arc::new(go(ak, bk, memo)),
                value: std::sync::Arc::new(go(av, bv, memo)),
            },
            (Ty::Tuple { elems: a }, Ty::Tuple { elems: b }) => Ty::Tuple {
                elems: a
                    .iter()
                    .zip(b.iter())
                    .map(|(a, b)| go(a, b, memo))
                    .collect(),
            },
            (Ty::Union { variants: a }, Ty::Union { variants: b }) => Ty::Union {
                variants: a
                    .iter()
                    .zip(b.iter())
                    .map(|(a, b)| go(a, b, memo))
                    .collect(),
            },
            (Ty::Class { id, args: a }, Ty::Class { args: b, .. }) => Ty::Class {
                id: id.clone(),
                args: a
                    .iter()
                    .zip(b.iter())
                    .map(|(a, b)| go(a, b, memo))
                    .collect(),
            },
            (Ty::Record { row: a }, Ty::Record { row: b }) => Ty::Record {
                row: crate::ty::Row {
                    fields: a
                        .fields
                        .iter()
                        .map(|(k, t)| (k.clone(), go(t, &b.fields[k], memo)))
                        .collect(),
                    rest: a.rest.clone(),
                },
            },
            (
                Ty::Fn {
                    params: a,
                    block: ab,
                    ret: ar,
                    effects,
                },
                Ty::Fn {
                    params: b,
                    block: bb,
                    ret: br,
                    ..
                },
            ) => Ty::Fn {
                params: a
                    .iter()
                    .zip(b.iter())
                    .map(|(a, b)| {
                        let mut p = a.clone();
                        p.ty = std::sync::Arc::new(go(&a.ty, &b.ty, memo));
                        p
                    })
                    .collect(),
                block: match (ab, bb) {
                    (Some(a), Some(b)) => Some(std::sync::Arc::new(go(a, b, memo))),
                    (a, _) => a.clone(),
                },
                ret: std::sync::Arc::new(go(ar, br, memo)),
                effects: effects.clone(),
            },
            (a, _) => a.clone(),
        };
        memo.insert(key, result.clone());
        result
    }
    go(&a, b, &mut std::collections::HashMap::new())
}

#[cfg(test)]
mod return_accumulator;

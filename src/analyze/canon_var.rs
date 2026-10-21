//! Canonical storage for a real unknown retained beside other union arms.
//! Bare Vars remain inference/empty-container holes. Explicit Pending is never
//! promoted, and this pass runs after typing so it cannot affect refinement.
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use super::Analyzer;
use crate::{App, expr::Expr, ty::Ty};

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_CANON_VAR").as_deref() == Ok("1"))
}

fn canonical(t: &Ty, memo: &mut HashMap<usize, Ty>) -> Ty {
    rewrite(t, memo, false)
}

pub(super) fn source_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_UNK_AT_SOURCE").as_deref() == Ok("1"))
}

/// A completed transfer's unknown is evidence, not a future computation.
/// Lookup internals may still use Var as a no-answer sentinel; it cannot
/// escape the producer into an annotation or a fold writer cell.
pub(super) fn produced(t: Ty) -> Ty {
    if !source_on() { return t; }
    let mut has_var = false;
    super::fold::visit(&t, &mut |v| has_var |= matches!(v, Ty::Var { .. }));
    if has_var { rewrite(&t, &mut HashMap::new(), true) } else { t }
}

fn rewrite(t: &Ty, memo: &mut HashMap<usize, Ty>, at_source: bool) -> Ty {
    let key = t as *const Ty as usize;
    if let Some(done) = memo.get(&key) {
        return done.clone();
    }
    let mut go = |t: &Ty| rewrite(t, memo, at_source);
    let out = match t {
        Ty::Var { .. } if at_source => Ty::unresolved(),
        Ty::Union { variants } => {
            let kept_unknown = variants.iter().any(|v| matches!(v, Ty::Var { .. }))
                && (at_source || variants
                    .iter()
                    .any(|v| !v.is_unknown() && !matches!(v, Ty::Bottom)));
            let mut changed = kept_unknown;
            let mut next_variants = Vec::new();
            for v in variants {
                let next = if kept_unknown && matches!(v, Ty::Var { .. }) {
                    Ty::unresolved()
                } else {
                    go(v)
                };
                changed |= !crate::wire_ty::storage_types(
                    std::slice::from_ref(v),
                    std::slice::from_ref(&next),
                );
                next_variants.push(next);
            }
            if !changed {
                memo.insert(key, t.clone());
                return t.clone();
            }
            let mut out: Vec<Ty> = Vec::new();
            for next in next_variants {
                // Deduplicate the converted Var and an existing Unresolved,
                // without merging away Pending or any other reason bits.
                if !out.iter().any(|v| {
                    crate::wire_ty::storage_types(
                        std::slice::from_ref(v),
                        std::slice::from_ref(&next),
                    )
                }) {
                    out.push(next);
                }
            }
            Ty::canonicalize_variants(&mut out);
            match out.len() {
                1 => out.pop().unwrap(),
                _ => Ty::Union {
                    variants: out.into(),
                },
            }
        }
        Ty::Array { elem } => Ty::Array {
            elem: Arc::new(go(elem)),
        },
        Ty::Hash { key, value } => Ty::Hash {
            key: Arc::new(go(key)),
            value: Arc::new(go(value)),
        },
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems.iter().map(&mut go).collect(),
        },
        Ty::Class { id, args } => Ty::Class {
            id: id.clone(),
            args: args.iter().map(&mut go).collect(),
        },
        Ty::Record { row } => Ty::Record {
            row: crate::ty::Row {
                fields: row.fields.iter().map(|(k, v)| (k.clone(), go(v))).collect(),
                rest: row.rest.clone(),
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
                    ty: Arc::new(go(&p.ty)),
                })
                .collect(),
            block: block.as_ref().map(|b| Arc::new(go(b))),
            ret: Arc::new(go(ret)),
            effects: effects.clone(),
        },
        other => other.clone(),
    };
    let out = if crate::wire_ty::storage_types(std::slice::from_ref(t), std::slice::from_ref(&out))
    {
        t.clone()
    } else {
        out
    };
    memo.insert(key, out.clone());
    out
}

fn stored(t: &mut Ty) {
    let next = canonical(t, &mut HashMap::new());
    if !crate::wire_ty::storage_types(std::slice::from_ref(t), std::slice::from_ref(&next)) {
        CHANGED.with(|n| n.set(n.get() + 1));
        if source_on() { CHANGED.with(|n| {
            if n.get() <= 16 { eprintln!("rh-canon-var-change: {}",serde_json::json!({
                "before":format!("{t:?}"),"after":format!("{next:?}")})); }
        }); }
    }
    *t = next;
}

thread_local! { static CHANGED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

fn expr(e: &mut Expr) {
    if let Some(t) = &mut e.ty {
        stored(t);
    }
    e.node.for_each_child_mut(&mut expr);
}

pub(super) fn finish(analyzer: &mut Analyzer, app: &mut App) {
    if on() {
        CHANGED.with(|n| n.set(0));
        rewrite_storage(analyzer, app);
        if source_on() { CHANGED.with(|n| eprintln!("rh-canon-var: {{\"changed_values\":{}}}", n.get())); }
    }
}

fn rewrite_storage(analyzer: &mut Analyzer, app: &mut App) {
    for cls in analyzer.classes.values_mut() {
        cls.instance_methods
            .values_mut()
            .chain(cls.class_methods.values_mut())
            .for_each(stored);
    }
    analyzer
        .inferred_params
        .values_mut()
        .chain(app.inferred_method_params.values_mut())
        .flatten()
        .for_each(stored);
    for names in app
        .view_ivar_types
        .values_mut()
        .chain(app.partial_local_types.values_mut())
    {
        names.values_mut().for_each(stored);
    }
    for resolution in app.controller_resolutions.values_mut() {
        for filter in &mut resolution.filter_chain {
            filter.assigns.values_mut().for_each(stored);
        }
    }
    for bindings in analyzer.refined_action_bindings.values_mut() {
        bindings.values_mut().for_each(stored);
    }
    for (bindings, bodies) in analyzer.controller_action_meta_cache.values_mut() {
        for names in bindings.values_mut() {
            names.values_mut().for_each(stored);
        }
        bodies.values_mut().for_each(expr);
    }
    super::fold::canonicalize_kept_vars();
    crate::lower::for_each_emit_body_mut(app, &mut expr);
}

pub(super) fn side_value(t: &mut Ty) {
    stored(t);
}

#[cfg(test)]
mod tests;

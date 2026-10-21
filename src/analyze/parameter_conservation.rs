//! Native parameter join. Unknown contributions are possible values,
//! not permission to discard them when another caller has a concrete type.
//! No per-writer completion is inferred from a bare legacy Var.
use crate::ty::Ty;
use std::sync::{Arc, OnceLock};

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_P2_PARAM_CONSERVE").as_deref() == Ok("1"))
}

/// Rows are joins of current call-site cells, with no previous aggregate.
/// SCCQ retains a skipped body's sites; the broad walk reads its last typed tree.
pub(super) fn row_current_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_ROW_CURRENT").as_deref() == Ok("1"))
}

/// A legacy Var on an actual argument expression denotes an unknown Ruby
/// value. Give it permanent provenance before the reader's Var cleanup. An
/// empty literal's element/key/value Vars are structural holes, not values.
/// This is applied to the latest site result, never to its previous result.
pub(super) fn argument(expr: &crate::expr::Expr) -> Ty {
    use crate::expr::ExprNode;
    let t = expr.ty.clone().unwrap_or(Ty::Var { var: crate::ident::TyVar(0) });
    if !row_current_on() { return t; }
    match (&*expr.node, t) {
        (ExprNode::Hash { entries, .. }, Ty::Hash { .. }) if !entries.is_empty() => {
            let (key, value) = entries.iter().fold((Ty::Bottom, Ty::Bottom), |(key, value), (k, v)| {
                (join(key, argument(k)), join(value, argument(v)))
            });
            Ty::Hash { key: Arc::new(key), value: Arc::new(value) }
        }
        (_, Ty::Var { .. }) => Ty::unresolved(),
        (_, t) => t,
    }
}

fn pident() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_C5_PIDENT").as_deref() == Ok("1"))
}

/// C5 L2: an `untyped` whose only reason is Pending is the identity beside any other arm.
fn pending_only(t: &Ty) -> bool {
    matches!(t, Ty::Untyped { why } if *why == crate::ty::Provenance::Pending)
}

// C5 L3: initialization is a join with the fresh (pending) cell; no first-arrival rule.
pub(super) fn initialize(observed: Ty) -> Ty {
    // Bottom is an already-computed non-returning argument, not an unpaid
    // input. Joining the fresh Pending seed would recreate it on every replay.
    if super::route_static::strict_on() && matches!(observed, Ty::Bottom) {
        return Ty::Bottom;
    }
    if pident() {
        join(Ty::pending_untyped(), observed)
    } else if matches!(observed, Ty::Bottom) {
        Ty::pending_untyped()
    } else {
        observed
    }
}

fn norm_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RH_C5_NORM").as_deref() == Ok("1"))
}

/// C5 L0: normalize every contribution before it enters the parameter join.
pub(super) fn join(a: Ty, b: Ty) -> Ty {
    if norm_on() {
        join_normal(normalize(a), normalize(b))
    } else {
        join_normal(a, b)
    }
}

/// Recursive storage normal form; record insertion order and evidence survive.
pub(super) fn normalize(t: Ty) -> Ty {
    match t {
        Ty::Union { variants } => variants
            .iter()
            .cloned()
            .map(normalize)
            .fold(Ty::Bottom, join_normal),
        Ty::Array { elem } => Ty::Array {
            elem: Arc::new(normalize(Arc::unwrap_or_clone(elem))),
        },
        Ty::Hash { key, value } => Ty::Hash {
            key: Arc::new(normalize(Arc::unwrap_or_clone(key))),
            value: Arc::new(normalize(Arc::unwrap_or_clone(value))),
        },
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems.iter().cloned().map(normalize).collect(),
        },
        Ty::Class { id, args } => Ty::Class {
            id,
            args: args.iter().cloned().map(normalize).collect(),
        },
        Ty::Record { row } => Ty::Record {
            row: crate::ty::Row {
                fields: row
                    .fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), normalize(ty.clone())))
                    .collect(),
                rest: row.rest,
            },
        },
        Ty::Fn { params, block, ret, effects } => Ty::Fn {
            params: params
                .iter()
                .map(|p| crate::ty::Param {
                    name: p.name.clone(),
                    ty: Arc::new(normalize(Arc::unwrap_or_clone(p.ty.clone()))),
                    kind: p.kind.clone(),
                })
                .collect(),
            block: block.map(|t| Arc::new(normalize(Arc::unwrap_or_clone(t)))),
            ret: Arc::new(normalize(Arc::unwrap_or_clone(ret))),
            effects,
        },
        Ty::Untyped { why } => Ty::Untyped { why: why.union(why) },
        other => other,
    }
}

/// Same container-spine union as the native join, without dropping unknowns.
/// Equal native shapes merge *all* reason bits even with RH_D3_PROV disabled.
fn join_normal(a: Ty, b: Ty) -> Ty {
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

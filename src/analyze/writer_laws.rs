//! Generated checks of carried-state writer policies, not analyzer repairs.
//!
//! Binary storage policies get the semilattice laws. Transfers into another
//! position get permutation, join-preservation, repetition, pending and
//! monotonicity checks instead. Reordering source statements is not a claim
//! about Ruby semantics: a failed overwrite law identifies a non-join policy.
//! Every law runs even after another fails. Witnesses minimize total Ty nodes
//! over this finite universe, then their printed inputs (not all Ruby programs).

use std::collections::BTreeMap;
use std::sync::Arc;

use super::body;
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, TyVar};
use crate::span::Span;
use crate::ty::{Param, ParamKind, Row, Ty};

pub(super) fn pending() -> Ty {
    Ty::Var { var: TyVar(0) }
}

pub(super) fn arr(elem: Ty) -> Ty {
    Ty::Array {
        elem: Arc::new(elem),
    }
}

pub(super) fn hash(key: Ty, value: Ty) -> Ty {
    Ty::Hash {
        key: Arc::new(key),
        value: Arc::new(value),
    }
}

pub(super) fn class(name: &str) -> Ty {
    Ty::Class {
        id: ClassId(Symbol::from(name)),
        args: vec![].into(),
    }
}

pub(super) fn function(ret: Ty) -> Ty {
    Ty::Fn {
        params: vec![Param {
            name: Symbol::from("x"),
            ty: Ty::Str.into(),
            kind: ParamKind::Required,
        }].into(),
        block: Some(Arc::new(arr(Ty::Int))),
        ret: Arc::new(ret),
        effects: EffectSet::default(),
    }
}

pub(super) fn expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

pub(super) fn stamped(ty: Ty) -> Expr {
    let mut e = expr(ExprNode::Lit {
        value: Literal::Nil,
    });
    e.ty = Some(ty);
    e
}

pub(super) fn nodes(t: &Ty) -> usize {
    let children: Vec<&Ty> = match t {
        Ty::Array { elem } => vec![elem],
        Ty::Hash { key, value } => vec![key, value],
        Ty::Union { variants } => variants.iter().collect(),
        Ty::Tuple { elems } => elems.iter().collect(),
        Ty::Record { row } => row.fields.values().collect(),
        Ty::Class { args, .. } => args.iter().collect(),
        Ty::Fn {
            params, block, ret, ..
        } => params
            .iter()
            .map(|p| &*p.ty)
            .chain(block.as_deref())
            .chain(std::iter::once(&**ret))
            .collect(),
        _ => vec![],
    };
    1 + children.into_iter().map(nodes).sum::<usize>()
}

pub(super) fn show(t: &Ty) -> String {
    let list = |ts: &[Ty]| {
        if ts.len() > 8 {
            format!(
                "{} arms; first={}; last={}",
                ts.len(),
                show(&ts[0]),
                show(&ts[ts.len() - 1])
            )
        } else {
            ts.iter().map(show).collect::<Vec<_>>().join(", ")
        }
    };
    match t {
        Ty::Var { var } => format!("Var({})", var.0),
        Ty::Array { elem } => format!("Array[{}]", show(elem)),
        Ty::Hash { key, value } => format!("Hash[{}, {}]", show(key), show(value)),
        Ty::Tuple { elems } => format!("Tuple[{}]", list(elems)),
        Ty::Union { variants } => format!("Union[{}]", list(variants)),
        Ty::Class { id, args } if args.is_empty() => id.0.as_str().to_string(),
        Ty::Class { id, args } => format!("{}[{}]", id.0.as_str(), list(args)),
        Ty::Record { row } => format!(
            "Record[{}]",
            row.fields
                .iter()
                .map(|(k, v)| format!("{}:{}", k.as_str(), show(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Ty::Fn { ret, .. } => format!("Fn[Str; block=Array[Int]; ret={}]", show(ret)),
        other => format!("{other:?}"),
    }
}

#[derive(Default)]
pub(super) struct Report {
    laws: BTreeMap<&'static str, (usize, Option<(usize, String)>)>,
}

impl Report {
    pub(super) fn merge(&mut self, other: Self) {
        for (law, (checks, witness)) in other.laws {
            let entry = self.laws.entry(law).or_default();
            entry.0 += checks;
            if let Some(witness) = witness {
                if entry.1.as_ref().is_none_or(|old| witness < *old) {
                    entry.1 = Some(witness);
                }
            }
        }
    }
    pub(super) fn equal(&mut self, law: &'static str, inputs: &[&Ty], left: Ty, right: Ty) {
        let entry = self.laws.entry(law).or_default();
        entry.0 += 1;
        // Raw variant permutations are intentional inputs. Compare their meaning,
        // not a Union Vec's incidental order (record Eq already ignores order).
        let (left, right) = (canonical(left), canonical(right));
        if left == right {
            return;
        }
        let cost = inputs.iter().map(|t| nodes(t)).sum();
        let witness = format!(
            "({}) => {} != {}",
            inputs
                .iter()
                .map(|t| show(t))
                .collect::<Vec<_>>()
                .join("; "),
            show(&left),
            show(&right)
        );
        let candidate = (cost, witness);
        if entry.1.as_ref().is_none_or(|old| candidate < *old) {
            entry.1 = Some(candidate);
        }
    }

    pub(super) fn finish(self, name: &str) {
        assert!(!self.laws.is_empty(), "empty writer-law selection: {name}");
        let mut failed = 0;
        for (law, (checks, witness)) in self.laws {
            assert!(checks > 0);
            match witness {
                None => println!("WRITER-LAW {name} | {law} | PASS | {checks} checks"),
                Some((cost, witness)) => {
                    failed += 1;
                    println!(
                        "WRITER-LAW {name} | {law} | FAIL | {checks} checks | {cost} nodes | {witness}"
                    );
                }
            }
        }
        assert_eq!(
            failed, 0,
            "{name}: {failed} writer laws failed (all laws reported above)"
        );
    }
}

fn canonical(t: Ty) -> Ty {
    match t {
        Ty::Union { variants } => body::union_many(variants.into_iter().map(canonical).collect()),
        Ty::Array { elem } => arr(canonical(Arc::unwrap_or_clone(elem))),
        Ty::Hash { key, value } => hash(canonical(Arc::unwrap_or_clone(key)), canonical(Arc::unwrap_or_clone(value))),
        Ty::Tuple { elems } => Ty::Tuple {
            elems: elems.into_iter().map(canonical).collect(),
        },
        Ty::Record { row } => Ty::Record {
            row: Row {
                fields: row
                    .fields
                    .into_iter()
                    .map(|(k, v)| (k, canonical(v)))
                    .collect(),
                rest: row.rest,
            },
        },
        Ty::Class { id, args } => Ty::Class {
            id,
            args: args.into_iter().map(canonical).collect(),
        },
        Ty::Fn {
            params,
            block,
            ret,
            effects,
        } => Ty::Fn {
            params: params
                .into_iter()
                .map(|p| Param {
                    ty: Arc::new(canonical(Arc::unwrap_or_clone(p.ty))),
                    ..p
                })
                .collect(),
            block: block.map(|b| Arc::new(canonical(Arc::unwrap_or_clone(b)))),
            ret: Arc::new(canonical(Arc::unwrap_or_clone(ret))),
            effects,
        },
        other => other,
    }
}

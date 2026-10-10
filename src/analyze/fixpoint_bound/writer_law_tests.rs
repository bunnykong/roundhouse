use super::*;
use crate::analyze::writer_laws as laws;
use crate::ident::{ClassId, Symbol};

fn nested(depth: usize) -> Ty {
    (0..depth).fold(Ty::Int, |t, _| laws::arr(t))
}

fn wide(prefix: &str, count: usize) -> Ty {
    // Distinct leaves need one canonical arm vector, not a quadratic fold
    // over all its prefixes. The production joins remain below.
    let mut variants: Vec<_> = (0..count)
        .map(|i| laws::class(&format!("{prefix}{i:03}")))
        .collect();
    Ty::canonicalize_variants(&mut variants);
    match variants.len() {
        0 => Ty::Bottom,
        1 => variants.pop().unwrap(),
        _ => Ty::Union { variants },
    }
}

fn boundary_universe() -> Vec<Ty> {
    let mut samples = laws::universe();
    for depth in [MAX_DEPTH - 1, MAX_DEPTH, MAX_DEPTH + 1] {
        samples.push(nested(depth));
    }
    for count in [MAX_NODES - 2, MAX_NODES - 1, MAX_NODES] {
        samples.push(wide("C", count)); // 511, 512, 513 nodes.
        samples.push(Ty::Tuple {
            elems: vec![Ty::Int; count],
        });
    }
    for count in [MAX_NODES - 4, MAX_NODES - 3, MAX_NODES - 2] {
        samples.push(laws::hash(Ty::Str, wide("H", count))); // 511, 512, 513 nodes.
    }
    samples
}

#[test]
fn writer_law_bound_idempotence_and_pending() {
    let mut report = laws::Report::default();
    report.equal("pending", &[], bound(laws::pending()), laws::pending());
    for t in boundary_universe() {
        let once = bound(t.clone());
        report.equal("idempotence", &[&t], bound(once.clone()), once);
        let (depth, count) = measure(&t);
        if depth <= MAX_DEPTH && count <= MAX_NODES {
            report.equal("within-limit-identity", &[&t], bound(t.clone()), t.clone());
        }
        let bounded = bound(t.clone());
        let (depth, count) = measure(&bounded);
        assert!(
            depth <= MAX_DEPTH && count <= MAX_NODES,
            "{}",
            laws::show(&t)
        );
    }
    report.finish("fixpoint_bound/boundary-universe");
}

/// Generate small-spine, large-node writers at the size/depth boundaries.
/// Tuples avoid making this check spend its time repeatedly sorting hundreds
/// of distinct leaves inside `cut`. The original #705/#724 class-union
/// regressions remain in the default suite. Minimize across every triple.
fn writer_triples() -> Vec<[Ty; 3]> {
    let mut triples = vec![];
    for depth in [0, 1, 2, 10, MAX_DEPTH - 1, MAX_DEPTH, MAX_DEPTH + 1] {
        for n in [1, 2, 254, 255, 256, 257, 300] {
            for m in [1, 2, 254, 255, 256, 257, 300] {
                for as_hash in [false, true] {
                    let (a, b) = (
                        Ty::Tuple {
                            elems: vec![Ty::Int; n],
                        },
                        Ty::Tuple {
                            elems: vec![Ty::Str; m],
                        },
                    );
                    let (a, b) = if as_hash {
                        (laws::hash(Ty::Str, a), laws::hash(Ty::Str, b))
                    } else {
                        (a, b)
                    };
                    triples.push([nested(depth), a, b]);
                }
            }
        }
    }
    triples
}

fn check_schedule(name: &str, join: fn(Ty, Ty) -> Ty) {
    let mut report = laws::Report::default();
    for writers in writer_triples() {
        let unbounded = join(
            join(writers[0].clone(), writers[1].clone()),
            writers[2].clone(),
        );
        let once = bound(unbounded);
        for [i, j, k] in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let (a, b, c) = (writers[i].clone(), writers[j].clone(), writers[k].clone());
            // Record the actual arrival order, so the minimized witness
            // reproduces the pairwise cut without searching its permutations.
            let inputs = [&a, &b, &c];
            let once_permuted = bound(join(join(a.clone(), b.clone()), c.clone()));
            report.equal("permutation/once", &inputs, once_permuted, once.clone());
            let per_pair = bound(join(bound(join(a.clone(), b.clone())), c.clone()));
            report.equal("once-vs-per-pair", &inputs, once.clone(), per_pair.clone());
            let regrouped = bound(join(a.clone(), bound(join(b.clone(), c.clone()))));
            report.equal("regrouping/per-pair", &inputs, per_pair, regrouped);
        }
    }
    report.finish(name);
}

#[test]
#[ignore = "known: bounding once and bounding each ivar merge pair disagree at the node limit"]
fn writer_law_bound_ivar_schedule() {
    check_schedule("bound/join_ivar_slot", body::join_ivar_slot);
}

#[test]
#[ignore = "known: bounding once and bounding each parameter merge pair disagree at the node limit"]
fn writer_law_bound_param_schedule() {
    check_schedule("bound/unify_param_ty", crate::analyze::unify_param_ty);
}

#[test]
fn writer_law_ivar_batch_bound() {
    let name = Symbol::from("x");
    let mut report = laws::Report::default();
    for writers in writer_triples() {
        let expected = bound(
            writers
                .iter()
                .cloned()
                .fold(laws::pending(), body::join_ivar_slot),
        );
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let make = |i: usize| {
                laws::expr(crate::expr::ExprNode::Assign {
                    target: crate::expr::LValue::Ivar { name: name.clone() },
                    value: laws::stamped(writers[i].clone()),
                })
            };
            let mut writes: Vec<_> = order.into_iter().map(make).collect();
            writes.push(make(order[0])); // Duplicate an original, before the batch cut.
            let body = laws::expr(crate::expr::ExprNode::Seq { exprs: writes });
            let mut out = std::collections::HashMap::new();
            super::super::extract_ivar_assignments(&body, &mut out);
            report.equal(
                "permutation-and-duplicate",
                &order.iter().map(|&i| &writers[i]).collect::<Vec<_>>(),
                out.remove(&name).unwrap(),
                expected.clone(),
            );
        }
    }
    report.finish("extract_ivar_assignments/bound-once");
}

#[test]
fn writer_law_param_batch_bound() {
    let key = (
        ClassId(Symbol::from("C")),
        Symbol::from("m"),
        crate::dialect::MethodReceiver::Instance,
    );
    let mut report = laws::Report::default();
    for writers in writer_triples() {
        let expected = bound(
            writers
                .iter()
                .cloned()
                .fold(laws::pending(), crate::analyze::unify_param_ty),
        );
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut observations: Vec<_> = order
                .iter()
                .map(|&i| (key.clone(), vec![writers[i].clone()]))
                .collect();
            observations.push((key.clone(), vec![writers[order[0]].clone()]));
            let mut rows = std::collections::HashMap::new();
            super::super::fold_param_observations(&mut rows, observations);
            report.equal(
                "permutation-and-duplicate",
                &order.iter().map(|&i| &writers[i]).collect::<Vec<_>>(),
                rows.remove(&key).unwrap().remove(0),
                expected.clone(),
            );
        }
    }
    report.finish("fold_param_observations/bound-once");
}

#[test]
#[ignore = "known: reflective ivar writes do not mark the slot touched for the batch bound"]
fn writer_law_reflective_ivar_bound() {
    let name = Symbol::from("x");
    let mut report = laws::Report::default();
    for t in boundary_universe() {
        if t.is_open() {
            continue;
        }
        let set = laws::expr(crate::expr::ExprNode::Send {
            recv: None,
            method: Symbol::from("instance_variable_set"),
            args: vec![
                laws::expr(crate::expr::ExprNode::Lit {
                    value: crate::expr::Literal::Sym {
                        value: Symbol::from("@x"),
                    },
                }),
                laws::stamped(t.clone()),
            ],
            block: None,
            parenthesized: false,
        });
        let mut out = std::collections::HashMap::new();
        super::super::extract_ivar_assignments(&set, &mut out);
        report.equal(
            "bounded-storage",
            &[&t],
            out.remove(&name).unwrap(),
            bound(t.clone()),
        );
    }
    report.finish("harvest_ivar_set/bound-coverage");
}

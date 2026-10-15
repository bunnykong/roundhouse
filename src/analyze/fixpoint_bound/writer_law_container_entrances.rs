//! Evaluated container entrances, compared to the existing complete-body bound.
use std::collections::{BTreeSet, HashMap};

use super::{MAX_DEPTH, bound, writer_law_tagged::reasons, writer_law_tests};
use crate::analyze::{IvarWalk, ivar_set::IvarNameEnv, writer_laws as laws};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

fn literal() -> Expr {
    laws::expr(ExprNode::Lit {
        value: Literal::Sym {
            value: Symbol::from("key"),
        },
    })
}

fn array(elements: Vec<Expr>) -> Expr {
    laws::expr(ExprNode::Array {
        elements,
        style: Default::default(),
    })
}

fn hash(key: Expr, value: Expr, kwargs: bool) -> Expr {
    laws::expr(ExprNode::Hash {
        entries: vec![(key, value)],
        kwargs,
    })
}

fn call(args: Vec<Expr>) -> Expr {
    laws::expr(ExprNode::Send {
        recv: None,
        method: Symbol::from("sink"),
        args,
        block: None,
        parenthesized: true,
    })
}

fn set(t: Ty, explicit_self: bool) -> Expr {
    laws::expr(ExprNode::Send {
        recv: explicit_self.then(|| laws::expr(ExprNode::SelfRef)),
        method: Symbol::from("instance_variable_set"),
        args: vec![
            laws::expr(ExprNode::Lit {
                value: Literal::Sym {
                    value: Symbol::from("@x"),
                },
            }),
            laws::stamped(t),
        ],
        block: None,
        parenthesized: true,
    })
}

const CONTEXTS: &[&str] = &[
    "array-first",
    "array-last",
    "hash-key",
    "hash-value",
    "kwargs-value",
    "array-hash-array",
    "hash-array-hash",
    "array-splat",
    "call-splat",
    "call-keyword-splat",
    "hash-send-value",
    "nested-splats",
    "forward-keyword-pairs",
];

fn wrap(context: &str, write: Expr) -> Expr {
    match context {
        "array-first" => array(vec![write, literal()]),
        "array-last" => array(vec![literal(), write]),
        "hash-key" => hash(write, literal(), false),
        "hash-value" => hash(literal(), write, false),
        "kwargs-value" => call(vec![hash(literal(), write, true)]),
        "array-hash-array" => array(vec![hash(literal(), array(vec![write]), false)]),
        "hash-array-hash" => hash(literal(), array(vec![hash(literal(), write, false)]), false),
        "array-splat" => array(vec![laws::expr(ExprNode::Splat { value: write })]),
        "call-splat" => call(vec![laws::expr(ExprNode::Splat { value: write })]),
        "call-keyword-splat" => call(vec![laws::expr(ExprNode::KeywordSplat { value: write })]),
        "hash-send-value" => hash(literal(), call(vec![write]), false),
        "nested-splats" => array(vec![laws::expr(ExprNode::Splat {
            value: array(vec![hash(
                literal(),
                call(vec![laws::expr(ExprNode::KeywordSplat {
                    value: hash(
                        literal(),
                        array(vec![laws::expr(ExprNode::Splat { value: write })]),
                        false,
                    ),
                })]),
                false,
            )]),
        })]),
        "forward-keyword-pairs" => call(vec![laws::expr(ExprNode::ForwardKeywordsWithPairs {
            entries: vec![(literal(), write)],
        })]),
        _ => unreachable!("unknown context"),
    }
}

fn plain(t: Ty) -> Expr {
    laws::expr(ExprNode::Assign {
        target: LValue::Ivar {
            name: Symbol::from("x"),
        },
        value: laws::stamped(t),
    })
}

fn seed() -> HashMap<Symbol, Ty> {
    HashMap::from([(
        Symbol::from("untouched"),
        (0..MAX_DEPTH + 3).fold(Ty::Str, |t, _| laws::arr(t)),
    )])
}

fn native_equal(left: &Ty, right: &Ty) {
    assert_eq!(format!("{left:?}"), format!("{right:?}"), "native shape");
    assert_eq!(reasons(left), reasons(right), "complete native reasons");
}

#[test]
#[ignore = "flag-on container boundary matrix; run explicitly with off/on receipts"]
fn writer_law_bound_reflective_container_boundary() {
    let x = Symbol::from("x");
    let untouched = Symbol::from("untouched");
    let mut checks = 0;
    let mut membership = 0;
    for (i, t) in writer_law_tests::boundary_universe()
        .into_iter()
        .enumerate()
    {
        if t.is_open() {
            continue;
        }
        let mut reference = seed();
        crate::analyze::extract_ivar_assignments(&plain(t.clone()), &mut reference);
        for context in CONTEXTS {
            for explicit_self in [false, true] {
                let body = wrap(context, set(t.clone(), explicit_self));
                let mut out = seed();
                let mut walk = IvarWalk::default();
                crate::analyze::walk_ivar_assignments(
                    &body,
                    &mut out,
                    &IvarNameEnv::NONE,
                    &mut walk,
                );
                assert_eq!(walk.touched, BTreeSet::from([x.clone()]), "{context}");
                native_equal(&out[&x], &t); // No eager container cut.
                native_equal(&out[&untouched], &seed()[&untouched]);
                let mut stored = seed();
                crate::analyze::extract_ivar_assignments(&body, &mut stored);
                native_equal(&stored[&x], &reference[&x]);
                native_equal(&stored[&untouched], &seed()[&untouched]);
                membership +=
                    super::writer_law_partitions::check_sampled_membership(&t, &stored[&x]);
                if !explicit_self {
                    println!(
                        "P2R2-CONTAINER {}",
                        serde_json::json!({
                            "case": format!("{context}/{i}"), "context": context, "input": t,
                            "plain": reference[&x], "reflective": stored[&x],
                            "native_plain": format!("{:?}", reference[&x]),
                            "native_reflective": format!("{:?}", stored[&x]),
                            "plain_reasons": reasons(&reference[&x]), "reflective_reasons": reasons(&stored[&x]),
                            "touched": ["x"], "unrelated_unchanged": true,
                        })
                    );
                }
                checks += 1;
            }
        }
    }
    assert!(checks > 0 && membership > 0);
    println!(
        "P2R2-METRIC {}",
        serde_json::json!({"container_boundary_checks": checks,
        "contexts": CONTEXTS.len(), "membership_checks": membership})
    );
}

#[test]
fn writer_law_bound_reflective_container_ineligible() {
    let t = (0..MAX_DEPTH + 1).fold(Ty::Int, |t, _| laws::arr(t));
    let base = set(t, false);
    let ExprNode::Send {
        recv, method, args, ..
    } = &*base.node
    else {
        unreachable!()
    };
    let controls = [
        (Some(laws::stamped(Ty::Int)), method.clone(), args.clone()),
        (
            recv.clone(),
            Symbol::from("instance_variable_get"),
            args.clone(),
        ),
        (recv.clone(), method.clone(), args[..1].to_vec()),
        (
            recv.clone(),
            method.clone(),
            vec![args[0].clone(), args[1].clone(), literal()],
        ),
        (
            recv.clone(),
            method.clone(),
            vec![laws::stamped(Ty::Str), args[1].clone()],
        ),
        (
            recv.clone(),
            method.clone(),
            vec![args[0].clone(), laws::stamped(laws::pending())],
        ),
        (
            recv.clone(),
            method.clone(),
            vec![args[0].clone(), laws::stamped(Ty::Bottom)],
        ),
        (
            recv.clone(),
            method.clone(),
            vec![args[0].clone(), literal()],
        ),
    ];
    let mut checks = 0;
    for context in CONTEXTS {
        for (recv, method, args) in &controls {
            let body = wrap(
                context,
                laws::expr(ExprNode::Send {
                    recv: recv.clone(),
                    method: method.clone(),
                    args: args.clone(),
                    block: None,
                    parenthesized: true,
                }),
            );
            let original = seed();
            let mut out = original.clone();
            let mut walk = IvarWalk::default();
            crate::analyze::walk_ivar_assignments(&body, &mut out, &IvarNameEnv::NONE, &mut walk);
            assert!(walk.touched.is_empty());
            assert_eq!(format!("{out:?}"), format!("{original:?}"));
            crate::analyze::extract_ivar_assignments(&body, &mut out);
            assert_eq!(format!("{out:?}"), format!("{original:?}"));
            checks += 1;
        }
    }
    println!(
        "P2R2-METRIC {}",
        serde_json::json!({"container_ineligible_checks": checks})
    );
}

#[test]
#[ignore = "flag-on whole-container batch; run explicitly"]
fn writer_law_bound_reflective_container_batch() {
    let x = Symbol::from("x");
    let mut checks = 0;
    for ts in writer_law_tests::writer_triples() {
        let expected = bound(
            ts.iter()
                .cloned()
                .fold(laws::pending(), crate::analyze::body::join_ivar_slot),
        );
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let writes = vec![
                wrap("array-first", set(ts[order[0]].clone(), false)),
                wrap("hash-value", set(ts[order[1]].clone(), true)),
                wrap("array-splat", set(ts[order[2]].clone(), false)),
                wrap("call-keyword-splat", set(ts[order[0]].clone(), false)),
            ];
            let mut out = seed();
            crate::analyze::extract_ivar_assignments(&array(writes), &mut out);
            native_equal(&out[&x], &expected);
            checks += 1;
        }
    }
    println!(
        "P2R2-METRIC {}",
        serde_json::json!({"container_batch_checks": checks})
    );
}

#[test]
#[ignore = "flag-on native Ruby container fixtures; run explicitly"]
fn writer_law_bound_reflective_container_native() {
    fn find<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
        if matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "instance_variable_set")
        {
            out.push(e);
        }
        e.node.for_each_child(&mut |child| find(child, out));
    }
    crate::ingest::survey::activate();
    let tree = HashMap::from([(
        std::path::PathBuf::from("app/models/container_entrance_witness.rb"),
        include_bytes!("fixtures/reflective_containers.rb").to_vec(),
    )]);
    let mut app = crate::ingest::app::ingest_app_from_tree(tree).unwrap();
    crate::analyze::Analyzer::new(&app).analyze(&mut app);
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "ContainerEntranceWitness")
        .unwrap();
    let mut checks = 0;
    for method in class
        .methods
        .iter()
        .filter(|m| m.name.as_str().starts_with("write_"))
    {
        let mut sets = Vec::new();
        find(&method.body, &mut sets);
        assert_eq!(sets.len(), 1, "{}", method.name);
        let ExprNode::Send {
            recv,
            method: send_name,
            args,
            ..
        } = &*sets[0].node
        else {
            unreachable!()
        };
        let mut direct = HashMap::new();
        let dest = crate::analyze::ivar_set::harvest_ivar_set(
            recv,
            send_name,
            args,
            &IvarNameEnv::NONE,
            &mut direct,
        )
        .expect("independently ingested writer must be eligible");
        let mut reference = seed();
        crate::analyze::extract_ivar_assignments(&plain(direct[&dest].clone()), &mut reference);
        let mut out = seed();
        let mut walk = IvarWalk::default();
        crate::analyze::walk_ivar_assignments(
            &method.body,
            &mut out,
            &IvarNameEnv::NONE,
            &mut walk,
        );
        assert_eq!(
            walk.touched,
            BTreeSet::from([dest.clone()]),
            "{}",
            method.name
        );
        native_equal(&out[&dest], &direct[&dest]);
        crate::analyze::extract_ivar_assignments(&method.body, &mut out);
        native_equal(&out[&dest], &reference[&dest]);
        println!(
            "P2R2-NATIVE {}",
            serde_json::json!({"method": method.name.to_string(),
            "eligible": true, "touched": [dest.to_string()], "storage_matches_plain": true,
            "native_plain": format!("{:?}", reference[&dest]), "reasons": reasons(&out[&dest])})
        );
        checks += 1;
    }
    assert_eq!(checks, 10);
    println!(
        "P2R2-METRIC {}",
        serde_json::json!({"native_container_entrances": checks})
    );
}

#[test]
#[ignore = "flag-on evaluated children must not walk syntax-only operands"]
fn writer_law_bound_reflective_container_deferred_syntax() {
    let body = array(vec![laws::expr(ExprNode::Defined {
        operand: set(Ty::Int, false),
    })]);
    let mut out = seed();
    let original = out.clone();
    crate::analyze::extract_ivar_assignments(&body, &mut out);
    assert_eq!(format!("{out:?}"), format!("{original:?}"));
}

fn lambda(body: Expr) -> Expr {
    laws::expr(ExprNode::Lambda {
        params: vec![],
        rest_param: None,
        extra_params: vec![],
        block_param: None,
        body,
        block_style: Default::default(),
    })
}

#[test]
#[ignore = "flag-on literal closure versus attached block evaluation controls"]
fn writer_law_bound_reflective_container_deferred_bodies() {
    let mut checks = 0;
    for context in CONTEXTS {
        // The Seq also checks that evaluation context propagates past a wrapper.
        let deferred = laws::expr(ExprNode::Seq {
            exprs: vec![lambda(set(Ty::Int, false))],
        });
        let body = wrap(context, deferred);
        let original = seed();
        let mut out = original.clone();
        crate::analyze::extract_ivar_assignments(&body, &mut out);
        assert_eq!(format!("{out:?}"), format!("{original:?}"), "{context}");
        let block_call = laws::expr(ExprNode::Send {
            recv: None,
            method: Symbol::from("yielding_call"),
            args: vec![],
            block: Some(lambda(set(Ty::Int, false))),
            parenthesized: false,
        });
        crate::analyze::extract_ivar_assignments(&wrap(context, block_call), &mut out);
        native_equal(&out[&Symbol::from("x")], &Ty::Int);
        checks += 2;
    }
    println!(
        "P2R2-METRIC {}",
        serde_json::json!({"deferred_and_block_controls": checks})
    );
}

#[test]
#[ignore = "flag-on Hash key and value destinations must both be reached"]
fn writer_law_bound_reflective_container_distinct_destinations() {
    let x = set(Ty::Int, false);
    let mut y = set(Ty::Str, true);
    let ExprNode::Send { args, .. } = y.node.as_mut() else {
        unreachable!()
    };
    args[0] = laws::expr(ExprNode::Lit {
        value: Literal::Sym {
            value: Symbol::from("@y"),
        },
    });
    let body = hash(x, y, false);
    let mut out = seed();
    let mut walk = IvarWalk::default();
    crate::analyze::walk_ivar_assignments(&body, &mut out, &IvarNameEnv::NONE, &mut walk);
    assert_eq!(
        walk.touched,
        BTreeSet::from([Symbol::from("x"), Symbol::from("y")])
    );
    native_equal(&out[&Symbol::from("x")], &Ty::Int);
    native_equal(&out[&Symbol::from("y")], &Ty::Str);
    native_equal(
        &out[&Symbol::from("untouched")],
        &seed()[&Symbol::from("untouched")],
    );
}

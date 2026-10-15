//! Independently ingested neighbours found in the evaluated-child audit.
use super::writer_law_tagged::reasons;
use crate::analyze::{IvarWalk, ivar_set::IvarNameEnv, writer_laws as laws};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use crate::ty::Ty;
use std::collections::{BTreeSet, HashMap};

fn check_native(method_name: &str, expected_reflection: bool) {
    fn find<'a>(e: &'a Expr, calls: &mut Vec<&'a Expr>, ordinary: &mut Vec<(Symbol, Ty)>) {
        match &*e.node {
            ExprNode::Send { method, .. } if method.as_str() == "instance_variable_set" => {
                calls.push(e)
            }
            ExprNode::Assign {
                target: LValue::Ivar { name },
                value,
            }
            | ExprNode::OpAssign {
                target: LValue::Ivar { name },
                value,
                ..
            } => {
                ordinary.push((
                    name.clone(),
                    value.ty.clone().expect("native ivar RHS is typed"),
                ));
            }
            _ => {}
        }
        e.node
            .for_each_child(&mut |child| find(child, calls, ordinary));
    }
    crate::ingest::survey::activate();
    let tree = HashMap::from([(
        std::path::PathBuf::from("app/models/neighbour_entrance_witness.rb"),
        include_bytes!("fixtures/reflective_neighbours.rb").to_vec(),
    )]);
    let mut app = crate::ingest::app::ingest_app_from_tree(tree).unwrap();
    crate::analyze::Analyzer::new(&app).analyze(&mut app);
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "NeighbourEntranceWitness")
        .unwrap();
    let body = &class
        .methods
        .iter()
        .find(|m| m.name.as_str() == method_name)
        .unwrap()
        .body;
    let mut calls = vec![];
    let mut ordinary = vec![];
    find(body, &mut calls, &mut ordinary);
    let mut expected = HashMap::new();
    let mut expected_touched = BTreeSet::new();
    if expected_reflection {
        assert_eq!(calls.len(), 1);
        let ExprNode::Send {
            recv, method, args, ..
        } = &*calls[0].node
        else {
            unreachable!()
        };
        let dest = crate::analyze::ivar_set::harvest_ivar_set(
            recv,
            method,
            args,
            &IvarNameEnv::NONE,
            &mut expected,
        )
        .expect("native neighbour writer is eligible");
        expected_touched.insert(dest);
    }
    for (name, ty) in ordinary {
        expected.insert(name.clone(), ty);
        expected_touched.insert(name);
    }
    let mut out = HashMap::new();
    let mut walk = IvarWalk::default();
    crate::analyze::walk_ivar_assignments(body, &mut out, &IvarNameEnv::NONE, &mut walk);
    let mut actual_node_kinds = BTreeSet::new();
    fn kinds(e: &Expr, out: &mut BTreeSet<String>) {
        out.insert(e.node.kind_str().to_string());
        e.node.for_each_child(&mut |child| kinds(child, out));
    }
    kinds(body, &mut actual_node_kinds);
    println!(
        "P2R2-NEIGHBOUR {}",
        serde_json::json!({"method": method_name,
        "expected_touched": expected_touched.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "actual_touched": walk.touched.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "ir_kinds": actual_node_kinds, "expected_reflection": expected_reflection})
    );
    assert_eq!(walk.touched, expected_touched, "{method_name}");
    assert_eq!(out.len(), expected.len());
    for (name, ty) in &expected {
        assert_eq!(
            format!("{:?}", out[name]),
            format!("{ty:?}"),
            "unbounded storage {method_name}"
        );
    }
    // Build an independent flat plain-assignment batch with exactly those writes.
    let flat = laws::expr(ExprNode::Seq {
        exprs: expected
            .iter()
            .map(|(name, ty)| {
                laws::expr(ExprNode::Assign {
                    target: LValue::Ivar { name: name.clone() },
                    value: laws::stamped(ty.clone()),
                })
            })
            .collect(),
    });
    let mut reference = HashMap::new();
    crate::analyze::extract_ivar_assignments(&flat, &mut reference);
    let mut stored = HashMap::new();
    crate::analyze::extract_ivar_assignments(body, &mut stored);
    for (name, ty) in &reference {
        assert_eq!(format!("{:?}", stored[name]), format!("{ty:?}"));
        assert_eq!(reasons(&stored[name]), reasons(ty));
    }
}

#[test]
#[ignore = "flag-on native explicit pairs beside anonymous keyword forwarding"]
fn writer_law_bound_reflective_container_neighbour_forwarding() {
    check_native("write_forwarded_keywords", true);
}

#[test]
#[ignore = "flag-on native Array in an ivar assignment RHS"]
fn writer_law_bound_reflective_container_neighbour_ivar_rhs() {
    check_native("write_ivar_rhs", true);
}

#[test]
#[ignore = "flag-on native Array in an ivar compound assignment RHS"]
fn writer_law_bound_reflective_container_neighbour_ivar_op_rhs() {
    check_native("write_ivar_op_rhs", true);
}

#[test]
#[ignore = "flag-on native attached block and deferred literal/syntax controls"]
fn writer_law_bound_reflective_container_neighbour_evaluation() {
    check_native("write_block", true);
    check_native("defer_in_array", false);
    check_native("query_in_array", false);
}

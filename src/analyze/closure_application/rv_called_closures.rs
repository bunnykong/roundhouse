//! Independent native-Ruby counterexample: a lambda value that is immediately invoked.
use super::writer_law_tagged::reasons;
use crate::analyze::{IvarWalk, ivar_set::IvarNameEnv, writer_laws as laws};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use std::collections::{BTreeSet, HashMap};

fn check(methods: &[&str], should_write: bool) {
    fn find<'a>(e: &'a Expr, calls: &mut Vec<&'a Expr>, kinds: &mut BTreeSet<String>) {
        kinds.insert(e.node.kind_str().to_owned());
        if let ExprNode::Send { method, .. } = &*e.node {
            if method.as_str() == "instance_variable_set" { calls.push(e); }
        }
        e.node.for_each_child(&mut |child| find(child, calls, kinds));
    }
    crate::ingest::survey::activate();
    let tree = HashMap::from([(
        std::path::PathBuf::from("app/models/called_closure_witness.rb"),
        include_bytes!("fixtures/rv_called_closures.rb").to_vec(),
    )]);
    let mut app = crate::ingest::app::ingest_app_from_tree(tree).unwrap();
    crate::analyze::Analyzer::new(&app).analyze(&mut app);
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "CalledClosureWitness").unwrap();
    let x = Symbol::from("x");
    let mut missed = Vec::new();
    for name in methods {
        let body = &class.methods.iter().find(|m| m.name.as_str() == *name).unwrap().body;
        let mut calls = Vec::new();
        let mut kinds = BTreeSet::new();
        find(body, &mut calls, &mut kinds);
        assert_eq!(calls.len(), 1, "one independently located writer per case");
        let ExprNode::Send { recv, method, args, .. } = &*calls[0].node else { unreachable!() };
        let mut flat_write = HashMap::new();
        let dest = crate::analyze::ivar_set::harvest_ivar_set(
            recv, method, args, &IvarNameEnv::NONE, &mut flat_write
        ).expect("native lambda-body write must meet unchanged eligibility");
        assert_eq!(dest, x);
        let plain = laws::expr(ExprNode::Assign {
            target: LValue::Ivar { name: x.clone() },
            value: laws::stamped(flat_write[&x].clone()),
        });
        let mut reference = HashMap::new();
        crate::analyze::extract_ivar_assignments(&plain, &mut reference);
        let mut out = HashMap::new();
        let mut walk = IvarWalk { mode: crate::analyze::ClosureHarvestMode::Apply, ..IvarWalk::default() };
        crate::analyze::walk_ivar_assignments(body, &mut out, &IvarNameEnv::NONE, &mut walk);
        let mut stored = HashMap::new();
        crate::analyze::extract_ivar_assignments_mode(body, &mut stored, &IvarNameEnv::NONE, crate::analyze::ClosureHarvestMode::Apply);
        let present = stored.contains_key(&x);
        let matches_plain = stored.get(&x).map(|t|
            format!("{t:?}") == format!("{:?}", reference[&x]) && reasons(t) == reasons(&reference[&x])
        ).unwrap_or(false);
        println!("RV-CLOSURE {}", serde_json::json!({
            "method": name, "eligible": true, "should_write": should_write,
            "touched": walk.touched.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "stored_x": present, "matches_plain": matches_plain, "ir_kinds": kinds,
            "plain": reference[&x], "actual": stored.get(&x),
            "plain_reasons": reasons(&reference[&x]),
            "actual_reasons": stored.get(&x).map(reasons),
        }));
        if (should_write && (!present || !matches_plain)) || (!should_write && present) {
            missed.push(*name);
        }
    }
    assert!(missed.is_empty(), "native evaluated/deferred behavior mismatched: {missed:?}");
}

#[test]
fn rv_called_closures_plain_controls() {
    check(&["bare_call", "bare_reflection"], true);
}

#[test]
fn rv_called_closures_direct_array_control() {
    check(&["array_reflection"], true);
}

#[test]
fn rv_called_closures_deferred_controls() {
    check(&["deferred_array", "deferred_hash"], false);
}

#[test]
fn rv_called_closures_evaluated_entrances() {
    check(&["array_call", "hash_key_call", "hash_value_call", "nested_call",
            "positional_splat_call", "keyword_splat_call", "array_brackets", "ivar_rhs_call"], true);
}

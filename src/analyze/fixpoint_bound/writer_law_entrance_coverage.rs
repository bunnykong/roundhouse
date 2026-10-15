//! A previously uncovered reflective call inside a Ruby array literal.
use std::collections::{BTreeSet, HashMap};
use super::MAX_DEPTH;
use crate::analyze::{IvarWalk, ivar_set::IvarNameEnv, writer_laws as laws};
use crate::expr::{ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

#[test]
#[ignore = "uncovered native Ruby entrance: Array elements are absent from the harvest walk; run explicitly"]
fn writer_law_bound_reflective_native_array_entrance() {
    fn find<'a>(e: &'a crate::expr::Expr, out: &mut Vec<&'a crate::expr::Expr>) {
        if matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "instance_variable_set") {
            out.push(e);
        }
        e.node.for_each_child(&mut |c| find(c, out));
    }
    crate::ingest::survey::activate();
    let tree = HashMap::from([(
        std::path::PathBuf::from("app/models/bound_entrance_witness.rb"),
        include_bytes!("fixtures/reflective_array.rb").to_vec(),
    )]);
    let mut app = crate::ingest::app::ingest_app_from_tree(tree).unwrap();
    crate::analyze::Analyzer::new(&app).analyze(&mut app);
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "BoundEntranceWitness")
        .expect("native fixture class must be ingested");
    let body = &class.methods.iter().find(|m| m.name.as_str() == "write").unwrap().body;
    let mut sets = Vec::new();
    find(body, &mut sets);
    assert_eq!(sets.len(), 1, "native fixture must contain one reflective writer");
    let ExprNode::Send { recv, method, args, .. } = &*sets[0].node else { unreachable!() };
    let mut direct = HashMap::new();
    let successful = crate::analyze::ivar_set::harvest_ivar_set(recv, method, args, &IvarNameEnv::NONE, &mut direct);
    assert_eq!(successful, Some(Symbol::from("x")), "native writer must be eligible");
    let mut walk = IvarWalk::default();
    let mut out = HashMap::new();
    crate::analyze::walk_ivar_assignments(body, &mut out, &IvarNameEnv::NONE, &mut walk);
    crate::analyze::extract_ivar_assignments(body, &mut out);
    println!("REFLECTIVE-ENTRANCE-UNCOVERED-NATIVE {}", serde_json::json!({
        "entrance": "Array.elements[0]", "successful_destination": "x",
        "reflective_calls": sets.len(),
        "typed_rhs": format!("{:?}", args[1].ty),
        "touched": walk.touched.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "stored_destinations": out.keys().map(|s| s.to_string()).collect::<Vec<_>>(),
    }));
    assert_eq!(walk.touched, BTreeSet::from([Symbol::from("x")]), "uncovered native reflective entrance");
    assert!(out.contains_key(&Symbol::from("x")), "native reflective destination missing from harvest");
}

#[test]
#[ignore = "uncovered entrance: the body harvest walker does not visit Array elements; run explicitly"]
fn writer_law_bound_reflective_nested_array_entrance() {
    let name = Symbol::from("x");
    let t = (0..MAX_DEPTH + 1).fold(Ty::Int, |t, _| laws::arr(t));
    let value = laws::stamped(t.clone());
    let name_expr = laws::expr(ExprNode::Lit { value: Literal::Sym { value: Symbol::from("@x") } });
    let set = laws::expr(ExprNode::Send {
        recv: None, method: Symbol::from("instance_variable_set"),
        args: vec![name_expr, value], block: None, parenthesized: false,
    });
    let ExprNode::Send { recv, method, args, .. } = &*set.node else { unreachable!() };
    let mut direct = HashMap::new();
    let successful = crate::analyze::ivar_set::harvest_ivar_set(recv, method, args, &IvarNameEnv::NONE, &mut direct);
    assert_eq!(successful, Some(name.clone()));
    let expr = laws::expr(ExprNode::Array { elements: vec![set], style: Default::default() });
    let mut out = HashMap::new();
    let mut walk = IvarWalk::default();
    crate::analyze::walk_ivar_assignments(&expr, &mut out, &IvarNameEnv::NONE, &mut walk);
    crate::analyze::extract_ivar_assignments(&expr, &mut out);
    println!("REFLECTIVE-ENTRANCE-UNCOVERED {}", serde_json::json!({
        "entrance": "Array.elements[0]", "successful_destination": successful.map(|s| s.to_string()),
        "touched": walk.touched.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "stored_destinations": out.keys().map(|s| s.to_string()).collect::<Vec<_>>(),
        "input_depth": MAX_DEPTH + 1, "required_destination": "x",
        "eligible_direct_storage": format!("{:?}", direct[&name]),
    }));
    assert_eq!(walk.touched, BTreeSet::from([name.clone()]), "uncovered reflective entrance");
    assert!(out.contains_key(&name), "successful reflective destination missing from harvest");
}

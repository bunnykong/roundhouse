use crate::analyze::ivar_set::IvarNameEnv;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;
use std::collections::HashMap;

#[test]
fn rv_minimal_called_lambda_in_array() {
    fn writer(e: &Expr, out: &mut Vec<Expr>) {
        if let ExprNode::Send { method, .. } = &*e.node {
            if method.as_str() == "instance_variable_set" { out.push(e.clone()); }
        }
        e.node.for_each_child(&mut |e| writer(e, out));
    }
    crate::ingest::survey::activate();
    let source = include_bytes!("fixtures/rv_minimal.rb").to_vec();
    let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([(
        std::path::PathBuf::from("app/models/minimal_called_closure.rb"), source
    )])).unwrap();
    crate::analyze::Analyzer::new(&app).analyze(&mut app);
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "MinimalCalledClosure").unwrap();
    let body = &class.methods.iter().find(|m| m.name.as_str() == "write").unwrap().body;
    let mut writes = Vec::new();
    writer(body, &mut writes);
    assert_eq!(writes.len(), 1);
    let ExprNode::Send { recv, method, args, .. } = &*writes[0].node else { unreachable!() };
    let mut flat = HashMap::new();
    let x = crate::analyze::ivar_set::harvest_ivar_set(recv, method, args, &IvarNameEnv::NONE, &mut flat)
        .expect("minimal literal writer is eligible");
    assert_eq!(x, Symbol::from("x"));
    assert_eq!(flat[&x], Ty::Int);
    let mut actual = HashMap::new();
    crate::analyze::extract_ivar_assignments_mode(body, &mut actual, &IvarNameEnv::NONE, crate::analyze::S4Mode::Apply);
    println!("RV-MINIMAL {}", serde_json::json!({
        "eligible": true, "plain": flat[&x], "actual": actual.get(&x), "body": body
    }));
    assert_eq!(actual.get(&x), Some(&Ty::Int), "executed eligible Array-element lambda write was missed");
}

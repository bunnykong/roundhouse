//! S4 card 1. Exclusions are decided before assertions, with eligibility/IR receipts retained.
mod rv_minimal;
mod rv_called_closures;
mod writer_law_tagged;
use super::*;
use std::path::{Path, PathBuf};
use writer_law_tagged::reasons;

fn fixture_app(set: &str) -> (App, serde_json::Value) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/analyze/s4_closures/fixtures").join(set);
    let spec: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("expectations.json")).unwrap()).unwrap();
    let tree = spec["fixtures"].as_array().unwrap().iter().map(|f| {
        let file = f["file"].as_str().unwrap();
        (PathBuf::from(file), std::fs::read(root.join(file)).unwrap())
    }).collect();
    crate::ingest::survey::activate();
    let mut app = crate::ingest::app::ingest_app_from_tree(tree).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    (app, spec)
}

fn x_writers<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match &*e.node {
        ExprNode::Send { method, .. } if method.as_str() == "instance_variable_set" => out.push(e),
        ExprNode::Assign { target: LValue::Ivar { name }, .. }
        | ExprNode::OpAssign { target: LValue::Ivar { name }, .. } if name.as_str() == "x" => out.push(e),
        _ => {},
    }
    e.node.for_each_child(&mut |child| x_writers(child, out));
}

fn proc_receiver(e: &Expr) -> Option<&Expr> {
    if let ExprNode::Send { recv: Some(recv), method, .. } = &*e.node
        && method.as_str() == "new" { return Some(recv); }
    let mut found = None;
    e.node.for_each_child(&mut |child| { if found.is_none() { found = proc_receiver(child); } });
    found
}

fn harvest(body: &Expr, mode: S4Mode) -> HashMap<Symbol, Ty> {
    let mut out = HashMap::new();
    extract_ivar_assignments_mode(body, &mut out, &ivar_set::IvarNameEnv::NONE, mode);
    out
}

fn exclusion(eligible: bool, constructor_invalid: bool) -> Option<&'static str> {
    if !eligible { Some("ineligible writer") }
    else if constructor_invalid { Some("typed Proc receiver is not constant Proc") }
    else { None }
}

#[test]
fn s4_fixture_matrix() {
    // Synthetic exclusion controls: excluded IDs retain evidence but do not reach harvest assertions.
    assert_eq!(exclusion(false, false), Some("ineligible writer"));
    assert_eq!(exclusion(true, true), Some("typed Proc receiver is not constant Proc"));
    assert_eq!(exclusion(true, false), None);
    let x = Symbol::from("x");
    for set in ["fixtures", "twins"] {
        let (app, spec) = fixture_app(set);
        for f in spec["fixtures"].as_array().unwrap() {
            let id = f["id"].as_str().unwrap();
            let class = app.library_classes.iter().find(|c| c.name.0.as_str() == f["cls"].as_str().unwrap()).unwrap();
            let body = &class.methods.iter().find(|m| m.name.as_str() == "write").unwrap().body;
            let mut writers = Vec::new();
            for m in &class.methods { x_writers(&m.body, &mut writers); }
            assert_eq!(writers.len(), 1, "{set}/{id}: locate writer independently, including helper methods");
            let writer = writers[0];
            let eligible = match &*writer.node {
                ExprNode::Send { recv, method, args, .. } => {
                    let mut isolated = HashMap::new();
                    ivar_set::harvest_ivar_set(recv, method, args, &ivar_set::IvarNameEnv::NONE, &mut isolated) == Some(x.clone())
                }
                ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => value.ty.is_some(),
                _ => unreachable!(),
            };
            let receiver = if matches!(id, "I6" | "D4") { proc_receiver(body) } else { None };
            let constructor_invalid = matches!(id, "I6" | "D4") && !receiver.is_some_and(|r|
                matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Proc"));
            let excluded = exclusion(eligible, constructor_invalid); // before every mode's assertion
            for (mode, column) in [(S4Mode::Legacy, "S"), (S4Mode::Entrances, "E"), (S4Mode::Apply, "I")] {
                let out = harvest(body, mode);
                let present = out.contains_key(&x);
                println!("S4-FIXTURE {}", serde_json::json!({
                    "set": set, "id": id, "mode": column, "eligible": eligible, "writer": writer,
                    "constructor_receiver": receiver, "excluded": excluded,
                    "got": if present { "h" } else { "m" }, "expected": f[column],
                }));
                if excluded.is_none() {
                    assert_eq!(present, f[column] == "h", "{set}/{id}/{column}");
                }
            }
        }
    }
}

#[test]
fn s4_unit_additivity() {
    for set in ["fixtures", "twins"] {
        let (app, _) = fixture_app(set);
        for class in &app.library_classes {
            for m in &class.methods {
                let legacy = harvest(&m.body, S4Mode::Legacy);
                for mode in [S4Mode::Entrances, S4Mode::Apply] {
                    let mut actual = HashMap::new();
                    let walk = extract_ivar_walk(&m.body, &mut actual, &ivar_set::IvarNameEnv::NONE, mode);
                    let keys: BTreeSet<_> = legacy.keys().chain(actual.keys()).collect();
                    for name in keys {
                        if walk.new_writes.contains(name) { continue; }
                        let expected = legacy.get(name).cloned().map(|ty|
                            if walk.reflective.contains(name) { fixpoint_bound::bound(ty) } else { ty });
                        assert_eq!(actual.get(name), expected.as_ref(), "{} {} {name} {mode:?}", class.name.0, m.name);
                        assert_eq!(actual.get(name).map(reasons), expected.as_ref().map(reasons),
                                   "provenance {} {} {name} {mode:?}", class.name.0, m.name);
                    }
                }
            }
        }
    }
}

#[test]
fn s4_mode_parse() {
    for (e, i, mode) in [(None, None, S4Mode::Legacy), (Some("1"), None, S4Mode::Entrances),
                         (None, Some("0"), S4Mode::Legacy),
                         (Some("1"), Some("0"), S4Mode::Entrances),
                         (None, Some("1"), S4Mode::Apply), (Some("1"), Some("1"), S4Mode::Apply)] {
        assert_eq!(S4Mode::parse(e, i), mode);
    }
    for v in ["", "0", "true", "2"] { assert_eq!(S4Mode::parse(Some(v), Some(v)), S4Mode::Legacy); }
}

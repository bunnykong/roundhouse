//! Native reason, name-environment and storage controls for the tagged port.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use super::{MAX_DEPTH, body, bound};
use crate::analyze::{IvarWalk, ivar_set::IvarNameEnv, writer_laws as laws};
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

// Walk every native child directly. Serde, semantic equality and current warm
// digests omit reasons. Keep occurrences and paths as well as the reason set.
pub(super) fn reasons(t: &Ty) -> BTreeMap<String, String> {
    fn walk(t: &Ty, path: String, out: &mut BTreeMap<String, String>) {
        match t {
            Ty::Untyped { why } => { out.insert(path, format!("{why:?}")); }
            Ty::Var { var } => { out.insert(path, format!("Var({})", var.0)); }
            Ty::Rec { slot } => { out.insert(path, format!("Rec({slot})")); }
            Ty::Array { elem } => walk(elem, format!("{path}.elem"), out),
            Ty::Hash { key, value } => {
                walk(key, format!("{path}.key"), out);
                walk(value, format!("{path}.value"), out);
            }
            Ty::Tuple { elems } | Ty::Union { variants: elems } => {
                for (i, t) in elems.iter().enumerate() { walk(t, format!("{path}[{i}]"), out); }
            }
            Ty::Class { args, .. } => {
                for (i, t) in args.iter().enumerate() { walk(t, format!("{path}.arg[{i}]"), out); }
            }
            Ty::Record { row } => {
                for (k, t) in &row.fields { walk(t, format!("{path}.field[{k}]"), out); }
                if let Some(var) = row.rest { out.insert(format!("{path}.rest"), format!("Var({})", var.0)); }
            }
            Ty::Fn { params, block, ret, .. } => {
                for (i, p) in params.iter().enumerate() { walk(&p.ty, format!("{path}.param[{i}]"), out); }
                if let Some(block) = block { walk(block, format!("{path}.block"), out); }
                walk(ret, format!("{path}.ret"), out);
            }
            Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Sym | Ty::Date | Ty::Time
            | Ty::Nil | Ty::Relation { .. } | Ty::SelfInstance | Ty::Bottom => {}
        }
    }
    let mut out = BTreeMap::new();
    walk(t, "$".into(), &mut out);
    out
}

fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    laws::expr(ExprNode::Send {
        recv, method: Symbol::from(method), args, block: None, parenthesized: false,
    })
}

fn string(s: &str) -> Expr {
    laws::expr(ExprNode::Lit { value: Literal::Str { value: s.into() } })
}

fn interp(e: Expr) -> Expr {
    laws::expr(ExprNode::StringInterp { parts: vec![
        InterpPart::Text { value: "@".into() }, InterpPart::Expr { expr: e },
    ] })
}

#[test]
fn writer_law_bound_reflective_reason_canaries() {
    assert_eq!(Ty::pending_untyped(), Ty::gradual()); // Semantic equality is insufficient.
    assert_ne!(reasons(&Ty::pending_untyped()), reasons(&Ty::gradual()));
    assert_ne!(reasons(&laws::arr(Ty::gradual())), reasons(&laws::arr(Ty::unresolved())));
    let all = Ty::Union { variants: vec![Ty::pending_untyped(), Ty::gradual(), Ty::unresolved()].into() };
    assert_eq!(reasons(&all).values().cloned().collect::<BTreeSet<_>>(),
               BTreeSet::from(["Pending".into(), "Gradual".into(), "Unresolved".into()]));
    println!("REFLECTIVE-ENTRANCE-METRIC {{\"native_reason_canaries\":3}}");
}

#[test]
#[ignore = "requires RH_REFLECT_BOUND=1; exercised explicitly in both flag arms"]
fn writer_law_bound_reflective_environment_names() {
    let class = ClassId(Symbol::from("Admin::WidgetsController"));
    let owned = HashMap::from([(Symbol::from("instance_name"),
        laws::expr(ExprNode::Return { value: laws::expr(ExprNode::Seq { exprs: vec![string("widget")] }) }))]);
    let lookup = |name: &Symbol| owned.get(name);
    let envs = [
        IvarNameEnv { self_class: Some(&class), owned: Some(&owned), lookup: None, models_by_ivar: None },
        IvarNameEnv { self_class: Some(&class), owned: None, lookup: Some(&lookup), models_by_ivar: None },
    ];
    let nested = (0..MAX_DEPTH + 1).fold(Ty::Int, |t, _| laws::arr(t));
    let names = [
        interp(send(Some(send(None, "controller_name", vec![])), "singularize", vec![])),
        interp(send(Some(send(Some(laws::expr(ExprNode::SelfRef)), "controller_name", vec![])),
                    "singularize", vec![])),
        interp(send(None, "instance_name", vec![])),
        interp(send(Some(send(Some(send(Some(laws::expr(ExprNode::SelfRef)), "class", vec![])),
            "to_s", vec![])), "demodulize", vec![])),
        send(Some(string("@widget_suffix")), "delete_suffix", vec![string("_suffix")]),
        send(Some(string("prefix@widget")), "delete_prefix", vec![string("prefix")]),
    ];
    let mut count = 0;
    for env in &envs {
        for name in &names {
            let dest = crate::analyze::ivar_set::fold_ivar_name(name, env).unwrap();
            for explicit_self in [false, true] {
                let write = send(explicit_self.then(|| laws::expr(ExprNode::SelfRef)),
                    "instance_variable_set", vec![name.clone(), laws::stamped(nested.clone())]);
                let mut out = HashMap::new();
                let mut walk = IvarWalk::default();
                crate::analyze::walk_ivar_assignments(&write, &mut out, env, &mut walk);
                assert_eq!(walk.touched, BTreeSet::from([dest.clone()]));
                assert_eq!(format!("{:?}", out[&dest]), format!("{nested:?}"));
                crate::analyze::extract_ivar_assignments_in(&write, &mut out, env);
                let expected = bound(nested.clone());
                assert_eq!(format!("{:?}", out[&dest]), format!("{expected:?}"));
                assert_eq!(reasons(&out[&dest]), reasons(&expected));
                count += 1;
            }
        }
    }
    // Model-name projection remains an independent transfer before the cut.
    let model = ClassId(Symbol::from("Widget"));
    let models = HashMap::from([(Symbol::from("widget"), model.clone())]);
    let env = IvarNameEnv { self_class: Some(&class), owned: None, lookup: None, models_by_ivar: Some(&models) };
    let expected = Ty::Class { id: model, args: vec![].into() };
    let rhs = body::union_of(expected.clone(), Ty::Nil);
    let write = send(None, "instance_variable_set", vec![string("@widget"), laws::stamped(rhs)]);
    let mut out = HashMap::new();
    let mut walk = IvarWalk::default();
    crate::analyze::walk_ivar_assignments(&write, &mut out, &env, &mut walk);
    assert_eq!(walk.touched, BTreeSet::from([Symbol::from("widget")]));
    assert_eq!(out[&Symbol::from("widget")], expected);
    println!("REFLECTIVE-ENTRANCE-METRIC {{\"environment_entrances\":{count},\"projection_controls\":1}}");
}

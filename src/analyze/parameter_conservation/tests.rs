use super::*;
use crate::{
    analyze::{fold_param_observations, producer_evidence as audit, unify_param_ty},
    dialect::MethodReceiver,
    ident::{ClassId, Symbol, TyVar},
    ty::{Param, ParamKind, Provenance, Row},
};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};

fn origins(t: &Ty) -> BTreeSet<String> {
    audit::origins(t)
        .into_iter()
        .map(|p| p.replace("/arm", ""))
        .collect()
}
fn array(t: Ty) -> Ty {
    Ty::Array { elem: Arc::new(t) }
}
fn hash(t: Ty) -> Ty {
    Ty::Hash {
        key: Arc::new(Ty::Str),
        value: Arc::new(t),
    }
}
fn tuple(t: Ty) -> Ty {
    Ty::Tuple {
        elems: vec![Ty::Int, t].into(),
    }
}
fn class(t: Ty) -> Ty {
    Ty::Class {
        id: ClassId(Symbol::from("Box")),
        args: vec![t].into(),
    }
}
fn record(t: Ty) -> Ty {
    Ty::Record {
        row: Row {
            fields: vec![(Symbol::from("x"), t)].into_iter().collect(),
            rest: None,
        },
    }
}
fn function(t: Ty) -> Ty {
    Ty::Fn {
        params: vec![Param {
            name: Symbol::from("x"),
            ty: Arc::new(t.clone()),
            kind: ParamKind::Required,
        }]
        .into(),
        block: Some(Arc::new(t.clone())),
        ret: Arc::new(t),
        effects: Default::default(),
    }
}
fn samples() -> Vec<Ty> {
    let mut out = vec![
        Ty::Bottom,
        Ty::Nil,
        Ty::Int,
        Ty::Str,
        Ty::Var { var: TyVar(0) },
        Ty::pending_untyped(),
        Ty::unresolved(),
        Ty::gradual(),
    ];
    for wrap in [array, hash, tuple, class, record, function] {
        for t in [Ty::Int, Ty::unresolved(), Ty::gradual()] {
            out.push(wrap(t));
        }
    }
    out.push(array(hash(Ty::unresolved())));
    out.push(Ty::Union {
        variants: vec![Ty::Int, Ty::unresolved()].into(),
    });
    out
}

#[derive(Debug)]
enum Value {
    Nil,
    Int,
    Str,
    Array(Vec<Value>),
    Hash(Vec<(Value, Value)>),
    Tuple(Vec<Value>),
}

// Independent finite interpretation. Pending has no runtime value evidence;
// ambiguous Var and permanent unknowns conservatively admit every witness.
fn contains(t: &Ty, v: &Value) -> bool {
    match (t, v) {
        (Ty::Untyped { why }, _) => why.reasons().iter().any(|r| *r != "Pending"),
        (Ty::Var { .. }, _) => true,
        (Ty::Nil, Value::Nil) | (Ty::Int, Value::Int) | (Ty::Str, Value::Str) => true,
        (Ty::Array { elem }, Value::Array(xs)) => xs.iter().all(|x| contains(elem, x)),
        (Ty::Hash { key, value }, Value::Hash(xs)) => xs
            .iter()
            .all(|(k, v)| contains(key, k) && contains(value, v)),
        (Ty::Tuple { elems }, Value::Tuple(xs)) => {
            elems.len() == xs.len() && elems.iter().zip(xs).all(|(t, v)| contains(t, v))
        }
        (Ty::Union { variants }, _) => variants.iter().any(|t| contains(t, v)),
        _ => false,
    }
}

#[test]
#[ignore = "repair-on native parameter conservation gate"]
fn writer_law_parameter_reason_conservation_grouping_and_values() {
    assert!(on(), "RH_PARAMETER_REASON_CONSERVE must be active");
    let ts = samples();
    let values = vec![
        Value::Nil,
        Value::Int,
        Value::Str,
        Value::Array(vec![]),
        Value::Array(vec![Value::Int]),
        Value::Array(vec![Value::Str]),
        Value::Array(vec![Value::Nil]),
        Value::Array(vec![Value::Int, Value::Str]),
        Value::Hash(vec![]),
        Value::Hash(vec![(Value::Str, Value::Int)]),
        Value::Hash(vec![(Value::Str, Value::Str)]),
        Value::Hash(vec![(Value::Str, Value::Nil)]),
        Value::Tuple(vec![Value::Int, Value::Str]),
        Value::Tuple(vec![Value::Int, Value::Nil]),
    ];
    let mut pairs = 0;
    let mut triples = 0;
    let mut permutations = 0;
    let mut membership_checks = 0;
    for a in &ts {
        for b in &ts {
            let ab = unify_param_ty(a.clone(), b.clone());
            let ba = unify_param_ty(b.clone(), a.clone());
            assert!(crate::wire_ty::storage_types(&[ab.clone()], &[ba]));
            let expected = origins(a)
                .union(&origins(b))
                .cloned()
                .collect::<BTreeSet<_>>();
            assert_eq!(origins(&ab), expected, "lost native component reason");
            let replay = unify_param_ty(ab.clone(), a.clone());
            assert!(crate::wire_ty::storage_types(&[ab.clone()], &[replay]));
            for v in &values {
                if contains(a, v) || contains(b, v) {
                    assert!(contains(&ab, v), "lost {v:?} from {ab:?}");
                }
                membership_checks += 1;
            }
            pairs += 1;
            for c in &ts {
                let expected = unify_param_ty(ab.clone(), c.clone());
                let right = unify_param_ty(a.clone(), unify_param_ty(b.clone(), c.clone()));
                assert!(
                    crate::wire_ty::storage_types(&[expected.clone()], &[right]),
                    "grouping changed native storage"
                );
                let reasons = origins(&ab)
                    .union(&origins(c))
                    .cloned()
                    .collect::<BTreeSet<_>>();
                assert_eq!(origins(&expected), reasons);
                for (x, y, z) in [
                    (a, b, c),
                    (a, c, b),
                    (b, a, c),
                    (b, c, a),
                    (c, a, b),
                    (c, b, a),
                ] {
                    let got = unify_param_ty(unify_param_ty(x.clone(), y.clone()), z.clone());
                    assert!(crate::wire_ty::storage_types(&[expected.clone()], &[got]));
                    permutations += 1;
                }
                triples += 1;
            }
        }
    }
    let known = unify_param_ty(Ty::Int, Ty::Str);
    assert!(
        matches!(&known, Ty::Union { variants } if variants.len()==2 && variants.contains(&Ty::Int) && variants.contains(&Ty::Str))
    );
    assert!(
        !known.mentions_unknown() && !contains(&known, &Value::Nil),
        "all unknown must not pass"
    );
    println!(
        "PARAMETER-EVIDENCE {}",
        json!({"samples":ts.len(), "pairs":pairs,"triples":triples,
        "permutations":permutations,"finite_membership_checks":membership_checks,"lost_values":0,"lost_reasons":0})
    );
}

#[test]
#[ignore = "repair-on native parameter seed, replay and destination gate"]
fn writer_law_parameter_reason_conservation_native_rows() {
    assert!(on() && audit::on());
    audit::reset(&[]);
    let key = (
        ClassId(Symbol::from("Owner")),
        Symbol::from("call"),
        MethodReceiver::Instance,
    );
    let mut rows = HashMap::new();
    let source = vec!["parameter|owner=Caller|instance|call|arg=0|fixture.rb:1-8".to_owned()];
    for t in [Ty::Int, Ty::unresolved(), Ty::gradual()] {
        audit::parameter_destination(&key, &source, true, std::slice::from_ref(&t));
        fold_param_observations(&mut rows, vec![(key.clone(), vec![t])]);
    }
    let native = rows[&key][0].clone();
    assert!(contains(&native, &Value::Int) && contains(&native, &Value::Str));
    assert_eq!(
        origins(&native),
        BTreeSet::from(["$:Gradual".into(), "$:Unresolved".into()])
    );
    fold_param_observations(&mut rows, vec![(key.clone(), vec![Ty::gradual()])]);
    assert!(crate::wire_ty::storage_types(&[native], &rows[&key]));
    let snapshot = audit::snapshot();
    assert!(snapshot["parameter_repair_merges"].as_u64().unwrap() > 0);
    assert_eq!(snapshot["parameter_output_origin_losses"], 0);
    assert_eq!(snapshot["parameter_type_origin_losses"], 0);
    assert_eq!(snapshot["parameter_unmapped_merges"], 0);
    assert!(snapshot["parameter_native_events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["destination"] == "Owner|call|Instance|param=0"));

    let mut known = HashMap::new();
    fold_param_observations(
        &mut known,
        vec![(key.clone(), vec![Ty::Int]), (key.clone(), vec![Ty::Str])],
    );
    assert_eq!(known[&key][0], unify_param_ty(Ty::Int, Ty::Str));
    assert!(
        !known[&key][0].mentions_unknown(),
        "an empty allocation seed is not a caller"
    );
    let mut unfinished = HashMap::new();
    fold_param_observations(&mut unfinished, vec![(key.clone(), vec![Ty::Bottom])]);
    assert_eq!(
        unfinished[&key][0],
        Ty::pending_untyped(),
        "one nonreturn does not complete the writer universe"
    );
    fold_param_observations(
        &mut unfinished,
        vec![(key.clone(), vec![Ty::pending_untyped()])],
    );
    assert_ne!(unfinished[&key][0], Ty::Bottom, "early never");
    fold_param_observations(
        &mut unfinished,
        vec![(key.clone(), vec![Ty::Var { var: TyVar(7) }])],
    );
    assert!(origins(&unfinished[&key][0])
        .iter()
        .any(|p| p.ends_with("LegacyAmbiguousVar")));
    println!("PARAMETER-NATIVE {}", snapshot);
}

#[test]
#[ignore = "repair-on native tag-only storage gate"]
fn writer_law_parameter_reason_conservation_tag_only_storage() {
    assert!(on());
    let a = array(Ty::gradual());
    let b = array(Ty::unresolved());
    let result = unify_param_ty(a.clone(), b);
    assert_eq!(a, result, "tag-only update changed ordinary equality");
    assert!(!crate::wire_ty::storage_types(&[a], &[result.clone()]));
    let mut stored: crate::shared::Shared<Vec<Ty>> = vec![result.clone()].into();
    let retained = stored.clone();
    stored[0] = Ty::Int;
    assert!(crate::wire_ty::storage_types(&[result.clone()], &retained));
    assert_eq!(
        origins(&retained[0]),
        BTreeSet::from(["$/element:Gradual".into(), "$/element:Unresolved".into()])
    );
    let wire: Ty = serde_json::from_value(serde_json::to_value(result).unwrap()).unwrap();
    assert_ne!(
        origins(&wire),
        origins(&retained[0]),
        "the current codec is still lossy"
    );
    println!("PARAMETER-STORAGE tag_only=pass shared=pass current_codec=known-lossy");
}

#[test]
#[ignore = "repair-on legacy ambiguity and mixed completion gate"]
fn writer_law_parameter_reason_conservation_pending_and_legacy() {
    assert!(on());
    for pending in [Ty::pending_untyped(), Ty::Var { var: TyVar(0) }] {
        let result = unify_param_ty(Ty::Bottom, pending.clone());
        assert_ne!(
            result,
            Ty::Bottom,
            "unfinished/ambiguous writer exported never"
        );
        let known = unify_param_ty(result, Ty::Int);
        assert!(origins(&pending).is_subset(&origins(&known)));
    }
    let all = Ty::untyped(
        Provenance::Pending
            .union(Provenance::Gradual)
            .union(Provenance::Unresolved),
    );
    let known = unify_param_ty(Ty::Int, all.clone());
    assert!(origins(&all).is_subset(&origins(&known)));
    assert!(contains(&known, &Value::Int) && contains(&known, &Value::Str));
    println!(
        "PARAMETER-COMPLETION unresolved_inventory=legacy-Var observed_pending=retained early_never=0"
    );
}

#[test]
#[ignore = "native parameter seed arrival breaker; intentionally red for this candidate"]
fn writer_law_parameter_reason_conservation_seed_arrival() {
    assert!(on() && audit::on());
    audit::reset(&[]);
    let key = (
        ClassId(Symbol::from("SeedOwner")),
        Symbol::from("call"),
        MethodReceiver::Instance,
    );
    let bottom_source =
        vec!["parameter|owner=NoValueWriter|Instance|call|arg=0|seed.rb:1-2".into()];
    let int_source = vec!["parameter|owner=IntegerWriter|Instance|call|arg=0|seed.rb:3-4".into()];
    audit::parameter_destination(&key, &bottom_source, true, &[Ty::Bottom]);
    audit::parameter_destination(&key, &int_source, true, &[Ty::Int]);
    let mut bottom_first = HashMap::new();
    let mut int_first = HashMap::new();
    fold_param_observations(
        &mut bottom_first,
        vec![
            (key.clone(), vec![Ty::Bottom]),
            (key.clone(), vec![Ty::Int]),
        ],
    );
    fold_param_observations(
        &mut int_first,
        vec![
            (key.clone(), vec![Ty::Int]),
            (key.clone(), vec![Ty::Bottom]),
        ],
    );
    let left = &bottom_first[&key];
    let right = &int_first[&key];
    let equal = crate::wire_ty::storage_types(left, right);
    println!(
        "PARAMETER-SEED-ARRIVAL {}",
        json!({
            "same_inputs":["Bottom","Int"], "destination":"SeedOwner|call|Instance|param=0",
            "bottom_first":format!("{:?}",left[0]), "int_first":format!("{:?}",right[0]),
            "bottom_first_origins":origins(&left[0]), "int_first_origins":origins(&right[0]),
            "integer_membership_both":contains(&left[0],&Value::Int) && contains(&right[0],&Value::Int),
            "native_storage_equal":equal
        })
    );
    assert!(
        equal,
        "native arrival order chooses known versus Pending priority"
    );
}

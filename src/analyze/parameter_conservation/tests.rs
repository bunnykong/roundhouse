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
    assert!(on(), "RH_P2_PARAM_CONSERVE must be active");
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
#[test]
#[ignore = "c5 card 1: next's native row algebra, repair off"]
fn c5_seed_arrival_native_control() {
    assert!(!on(), "run with RH_P2_PARAM_CONSERVE=0");
    let key = (ClassId(Symbol::from("SeedOwner")), Symbol::from("call"), MethodReceiver::Instance);
    let fold = |obs: Vec<Ty>| {
        let mut rows = HashMap::new();
        fold_param_observations(&mut rows, obs.into_iter().map(|t| (key.clone(), vec![t])).collect());
        rows[&key][0].clone()
    };
    let bottom_first = fold(vec![Ty::Bottom, Ty::Int]);
    let int_first = fold(vec![Ty::Int, Ty::Bottom]);
    assert!(crate::wire_ty::storage_types(&[bottom_first.clone()], &[int_first]), "native arrival order");
    assert_eq!(bottom_first, Ty::Int);
    assert!(matches!(fold(vec![Ty::Bottom]), Ty::Var { .. }), "bottom alone stays pending on next");
}

fn evidence(o: &BTreeSet<String>) -> BTreeSet<String> {
    o.iter().filter(|p| !p.ends_with(":Pending")).cloned().collect()
}

#[test]
#[ignore = "c5 card 1b: lineage factorial; counts every counterexample"]
fn c5_lineage_factorial() {
    assert!(on(), "run with RH_P2_PARAM_CONSERVE=1");
    let arm = (std::env::var("RH_C5_PIDENT").unwrap_or_else(|_| "1".into()),
               std::env::var("RH_C5_LIFT").unwrap_or_else(|_| "1".into()));
    let st = |a: &Ty, b: &Ty| crate::wire_ty::storage_types(&[a.clone()], &[b.clone()]);
    let key = (ClassId(Symbol::from("LawOwner")), Symbol::from("call"), MethodReceiver::Instance);
    let fold = |obs: &[&Ty]| {
        let mut rows = HashMap::new();
        fold_param_observations(&mut rows, obs.iter().map(|t| (key.clone(), vec![(*t).clone()])).collect());
        rows[&key][0].clone()
    };
    let (ts, p) = (samples(), Ty::pending_untyped());
    let mut n: std::collections::BTreeMap<&str, u64> = Default::default();
    let mut ex: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
    let mut bad = |law: &'static str, case: String| {
        *n.entry(law).or_default() += 1;
        let e = ex.entry(law).or_default();
        if e.len() < 5 { e.push(case) }
    };
    for a in &ts {
        for b in &ts {
            let ab = unify_param_ty(a.clone(), b.clone());
            if !st(&ab, &unify_param_ty(b.clone(), a.clone())) { bad("comm", format!("{a:?} | {b:?}")) }
            if !st(&unify_param_ty(ab.clone(), a.clone()), &ab) { bad("idem", format!("{a:?} | {b:?}")) }
            if !st(&fold(&[a, b]), &fold(&[b, a])) { bad("fold2", format!("{a:?} | {b:?}")) }
            let want: BTreeSet<String> = evidence(&origins(a)).union(&evidence(&origins(b))).cloned().collect();
            if evidence(&origins(&ab)) != want { bad("evidence", format!("{a:?} | {b:?}")) }
            for c in &ts {
                let left = unify_param_ty(ab.clone(), c.clone());
                let right = unify_param_ty(a.clone(), unify_param_ty(b.clone(), c.clone()));
                if !st(&left, &right) { bad("assoc", format!("{a:?} | {b:?} | {c:?}")) }
                let f = fold(&[a, b, c]);
                if [[a, c, b], [b, a, c], [b, c, a], [c, a, b], [c, b, a]].iter().any(|o| !st(&f, &fold(o))) {
                    bad("fold3", format!("{a:?} | {b:?} | {c:?}"))
                }
            }
        }
    }
    for x in &ts {
        let want = if matches!(x, Ty::Bottom) { p.clone() } else { unify_param_ty(x.clone(), x.clone()) };
        if !st(&unify_param_ty(p.clone(), x.clone()), &want) { bad("pid", format!("{x:?}")) }
    }
    let seed_ok = st(&fold(&[&Ty::Bottom, &Ty::Int]), &fold(&[&Ty::Int, &Ty::Bottom]));
    let no_early_never = !matches!(fold(&[&Ty::Bottom]), Ty::Bottom);
    println!("C5-FACTORIAL {}", json!({"arm": arm, "counts": n, "examples": ex,
        "seed_ok": seed_ok, "no_early_never": no_early_never}));
}

#[test]
#[ignore = "C4/C5: extended native census, including producer-normalization shapes"]
fn c45_extended_laws() {
    assert!(on(), "run with RH_P2_PARAM_CONSERVE=1");
    let arm = (std::env::var("RH_C5_PIDENT").unwrap_or_else(|_| "1".into()),
               std::env::var("RH_C5_LIFT").unwrap_or_else(|_| "1".into()));
    let st = |a: &Ty, b: &Ty| crate::wire_ty::storage_types(&[a.clone()], &[b.clone()]);
    let key = (ClassId(Symbol::from("LawOwner")), Symbol::from("call"), MethodReceiver::Instance);
    let fold = |obs: &[&Ty]| {
        let mut rows = HashMap::new();
        fold_param_observations(&mut rows, obs.iter().map(|t| (key.clone(), vec![(*t).clone()])).collect());
        rows[&key][0].clone()
    };
    let (ts, p) = (c45_samples(), Ty::pending_untyped());
    let mut n: std::collections::BTreeMap<&str, u64> = Default::default();
    let mut ex: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
    let mut bad = |law: &'static str, case: String| {
        *n.entry(law).or_default() += 1;
        let e = ex.entry(law).or_default();
        if e.len() < 5 { e.push(case) }
    };
    for a in &ts {
        for b in &ts {
            let ab = unify_param_ty(a.clone(), b.clone());
            if !st(&ab, &unify_param_ty(b.clone(), a.clone())) { bad("comm", format!("{a:?} | {b:?}")) }
            if !st(&unify_param_ty(ab.clone(), a.clone()), &ab) { bad("idem", format!("{a:?} | {b:?}")) }
            if !st(&fold(&[a, b]), &fold(&[b, a])) { bad("fold2", format!("{a:?} | {b:?}")) }
            let want: BTreeSet<String> = evidence(&origins(a)).union(&evidence(&origins(b))).cloned().collect();
            if evidence(&origins(&ab)) != want { bad("evidence", format!("{a:?} | {b:?}")) }
            for c in &ts {
                let left = unify_param_ty(ab.clone(), c.clone());
                let right = unify_param_ty(a.clone(), unify_param_ty(b.clone(), c.clone()));
                if !st(&left, &right) { bad("assoc", format!("{a:?} | {b:?} | {c:?}")) }
                let f = fold(&[a, b, c]);
                if [[a, c, b], [b, a, c], [b, c, a], [c, a, b], [c, b, a]].iter().any(|o| !st(&f, &fold(o))) {
                    bad("fold3", format!("{a:?} | {b:?} | {c:?}"))
                }
            }
        }
    }
    for x in &ts {
        let normalized = unify_param_ty(x.clone(), x.clone());
        let want = if matches!(normalized, Ty::Bottom) { p.clone() } else { normalized };
        if !st(&unify_param_ty(p.clone(), x.clone()), &want) { bad("pid", format!("{x:?}")) }
    }
    let seed_ok = st(&fold(&[&Ty::Bottom, &Ty::Int]), &fold(&[&Ty::Int, &Ty::Bottom]));
    let no_early_never = !matches!(fold(&[&Ty::Bottom]), Ty::Bottom);
    println!("C45-EXTENDED {}", json!({"arm": arm, "samples": ts.len(), "counts": n, "examples": ex,
        "seed_ok": seed_ok, "no_early_never": no_early_never}));
    assert!(n.is_empty() && seed_ok && no_early_never, "native extended laws have counterexamples: {n:?}");
}


fn c45_rbs_observation(declaration: &str) -> Ty {
    let source = format!("class C45Source\n def produce: () -> ({declaration})\nend\n");
    let parsed = crate::rbs::parse_app_signatures(&source).expect("valid production RBS input");
    let ty = &parsed[&ClassId(Symbol::from("C45Source"))][&Symbol::from("produce")];
    let Ty::Fn { ret, .. } = ty else { panic!("expected parsed method signature") };
    (**ret).clone()
}

fn c45_samples() -> Vec<Ty> {
    let mut ts = samples();
    for declaration in ["bot | bot", "Array[String | Integer]", "Hash[String, String | Integer]",
                        "Array[Integer | Integer | bot]", "Hash[String, Integer | Integer | bot]"] {
        ts.push(c45_rbs_observation(declaration));
    }
    let nested = Ty::Union { variants: vec![
        Ty::Union { variants: vec![Ty::Int, Ty::pending_untyped()].into() },
        Ty::unresolved(), Ty::gradual(), Ty::Bottom,
    ].into() };
    ts.extend([nested.clone(), array(nested.clone()), hash(nested)]);
    ts
}

fn c45_fold(observations: &[Ty]) -> Ty {
    let key = (ClassId(Symbol::from("C45Owner")), Symbol::from("observe"), MethodReceiver::Instance);
    let mut rows = HashMap::new();
    for observed in observations {
        fold_param_observations(&mut rows, vec![(key.clone(), vec![observed.clone()])]);
    }
    rows[&key][0].clone()
}

#[test]
#[ignore = "C4/C5: required native C1 Bottom regression, production parser and fold"]
fn c45_rbs_bottom_pending() {
    assert!(on());
    let mut bad = Vec::new();
    for declaration in ["bot", "bot | bot"] {
        let observed = c45_rbs_observation(declaration);
        let got = c45_fold(std::slice::from_ref(&observed));
        println!("C45-BOTTOM {}", json!({"declaration": declaration, "observed": observed,
            "native_row": got, "native_debug": format!("{got:?}"), "origins": origins(&got),
            "pending": matches!(&got, Ty::Untyped { why } if *why == Provenance::Pending),
            "bottom": matches!(&got, Ty::Bottom)}));
        if !matches!(&got, Ty::Untyped { why } if *why == Provenance::Pending) { bad.push(declaration); }
    }
    assert!(bad.is_empty(), "unfinished writer exported native Bottom: {bad:?}");
}

#[test]
#[ignore = "C4/C5: preregistered supplemental RBS array replay"]
fn c45_rbs_array_replay() {
    assert!(on());
    for declaration in ["Array[Integer]", "Array[String | Integer]",
                        "Array[Integer | Integer | bot]", "Hash[String, Integer | Integer | bot]"] {
        let observed = c45_rbs_observation(declaration);
        let once = c45_fold(std::slice::from_ref(&observed));
        let replayed = c45_fold(&[observed.clone(), observed.clone()]);
        let same = crate::wire_ty::storage_types(std::slice::from_ref(&once), std::slice::from_ref(&replayed));
        println!("C45-REPLAY {}", json!({"declaration": declaration, "observed": observed,
            "once": once, "replayed": replayed, "storage_equal": same}));
    }
}

#[test]
#[ignore = "C4/C5: M0 changes annotations while preserving values"]
fn c45_m0_equal_merge() {
    assert!(super::super::handoff::c45_m0_on());
    let a=array(Ty::gradual());
    let b=array(Ty::unresolved());
    let joined=super::super::handoff::join(a.clone(),b.clone());
    let slot=super::super::handoff::join_slot(a.clone(),b.clone());
    let harvested=super::super::harvest_return::evidence_control_decision(a.clone(),b.clone());
    for got in [joined,slot,harvested] {
        assert_eq!(a,got);
        assert_eq!(origins(&got),origins(&a).union(&origins(&b)).cloned().collect());
    }
}

#[test]
#[ignore = "c5 card 4: normal form L0 and attribution of the extended census"]
fn c4_normal_form() {
    assert!(on(), "run with RH_P2_PARAM_CONSERVE=1");
    let st = |a: &Ty, b: &Ty| crate::wire_ty::storage_types(&[a.clone()], &[b.clone()]);
    let n = |t: &Ty| super::normalize(t.clone());
    let nested_raw = |x: &Ty| !st(&n(x), &super::join_normal(Ty::Bottom, x.clone()));
    let (ts, p, originals) = (c45_samples(), Ty::pending_untyped(), samples().len());
    let mut c: std::collections::BTreeMap<&str, u64> = Default::default();
    let mut inc = |k: &'static str| *c.entry(k).or_default() += 1;
    for (i, x) in ts.iter().enumerate() {
        if !st(&n(&n(x)), &n(x)) { inc("n_not_idempotent") }
        if !st(&n(x), x) { inc(if i < originals { "n_changes_original" } else { "n_changes_added" }) }
        if evidence(&origins(&n(x))) != evidence(&origins(x)) { inc("n_loses_evidence") }
        if nested_raw(x) { inc("nested_raw_samples") }
    }
    for a in &ts {
        for b in &ts {
            let ab = unify_param_ty(a.clone(), b.clone());
            if !st(&unify_param_ty(ab.clone(), a.clone()), &ab) {
                inc("idem");
                if !(nested_raw(a) || nested_raw(b)) { inc("idem_unattributed") }
            }
        }
    }
    for x in &ts {
        let nn = unify_param_ty(x.clone(), x.clone());
        let want = if matches!(nn, Ty::Bottom) { p.clone() } else { nn };
        if !st(&unify_param_ty(p.clone(), x.clone()), &want) {
            inc("pid");
            if !nested_raw(x) { inc("pid_unattributed") }
        }
    }
    println!("C4-NORMAL-FORM {}", json!({"norm": std::env::var("RH_C5_NORM").unwrap_or_else(|_| "1".into()), "counts": c}));
}

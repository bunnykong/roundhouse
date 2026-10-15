//! Read-only characterization of the pinned analyzer; not a semantic repair.
use super::*;
use crate::ty::Provenance;
use serde_json::{Value, json};

fn describe(t: &Ty) -> Value {
    json!({"debug": format!("{t:?}"), "wire": serde_json::to_value(t).unwrap()})
}

#[test]
fn producer_reason_provenance_observe() {
    let p = Ty::pending_untyped();
    let g = Ty::gradual();
    let u = Ty::unresolved();
    let classes = HashMap::new();
    let typer = BodyTyper::new(&classes);
    let ghost = Ty::Class {
        id: ClassId(Symbol::from("EvidenceExternal")),
        args: vec![].into(),
    };
    let miss = typer.dispatch(Some(&ghost), &Symbol::from("producer_reason_unmodeled"), None, &[]);
    let pg = union_of(p.clone(), g.clone());
    let gp = union_of(g.clone(), p.clone());
    let roundtrip: Ty = serde_json::from_value(serde_json::to_value(&u).unwrap()).unwrap();
    let (guard_true, guard_false) = narrowing::head_split(&u, &Ty::Str);
    println!(
        "producer_reason: {}",
        json!({
            "condition": "producer_reason-provenance-38403140-characterization-v1",
            "fold": std::env::var("RH_FOLD").ok(),
            "union_pending_gradual": describe(&pg),
            "union_gradual_pending": describe(&gp),
            "ordinary_equality_of_reordered_joins": pg == gp,
            "storage_equality_of_reordered_joins": crate::wire_ty::storage_types(
                std::slice::from_ref(&pg), std::slice::from_ref(&gp)),
            "parameter_int_gradual": describe(&super::super::unify_param_ty(Ty::Int, g)),
            "ivar_nil_tagged_pending": describe(&join_ivar_slot(Ty::Nil, p)),
            "missing_dispatch": describe(&miss),
            "ivar_nil_missing_dispatch": describe(&join_ivar_slot(Ty::Nil, miss)),
            "unresolved_json_roundtrip": describe(&roundtrip),
            "string_guard_true": describe(&guard_true),
            "string_guard_false": describe(&guard_false)
        })
    );
}

/// Proposed conservation gate: run explicitly, and expect failure on the pin.
/// It is a design requirement, not an existing Roundhouse guarantee.
#[test]
#[ignore = "proposed provenance contract; deliberately fails on 38403140"]
fn producer_reason_provenance_contract() {
    for (a, b) in [
        (Ty::pending_untyped(), Ty::gradual()),
        (Ty::unresolved(), Ty::gradual()),
    ] {
        for (left, right) in [(a.clone(), b.clone()), (b, a)] {
            assert_eq!(
                union_of(left, right).provenance(),
                Some(Provenance::Gradual),
                "merging unknown tags must retain the gradual possibility"
            );
        }
    }
    // This is only the scalar policy minimum. A reason-set prototype must add
    // assertions for all input origins; this test alone cannot certify that.
    // Ordinary serialized type output must stay unchanged in this first trial.
    for (a, b, c) in [
        (Ty::pending_untyped(), Ty::unresolved(), Ty::gradual()),
        (Ty::gradual(), Ty::pending_untyped(), Ty::unresolved()),
        (Ty::unresolved(), Ty::gradual(), Ty::pending_untyped()),
    ] {
        let left = union_of(union_of(a.clone(), b.clone()), c.clone());
        let right = union_of(a, union_of(b, c));
        assert_eq!(left.provenance(), Some(Provenance::Gradual));
        assert_eq!(right.provenance(), Some(Provenance::Gradual));
        assert_eq!(
            serde_json::to_value(left).unwrap(),
            serde_json::to_value(Ty::gradual()).unwrap()
        );
    }
}

#[test]
#[ignore = "proposed producer distinction; deliberately fails on 38403140"]
fn producer_reason_pending_contract() {
    let classes = HashMap::new();
    let typer = BodyTyper::new(&classes);
    let ghost = Ty::Class {
        id: ClassId(Symbol::from("EvidenceExternal")),
        args: vec![].into(),
    };
    let miss = typer.dispatch(Some(&ghost), &Symbol::from("producer_reason_unmodeled"), None, &[]);
    assert_eq!(
        miss.provenance(),
        Some(Provenance::Unresolved),
        "a missing external method is not an unevaluated slot"
    );
    assert_ne!(
        join_ivar_slot(Ty::Nil, miss),
        Ty::Nil,
        "a modeling gap must not certify nil-only"
    );
    assert_eq!(
        join_ivar_slot(Ty::Nil, Ty::pending_untyped()),
        Ty::Nil,
        "a verified pending seed contributes no value evidence"
    );
}

#[test]
#[ignore = "proposed positive guard bound; deliberately fails on 38403140"]
fn producer_reason_guard_contract() {
    // This asserts the type projection of the true branch. A provenance-aware
    // implementation must retain its origin separately, not erase the cause.
    for unknown in [Ty::gradual(), Ty::unresolved()] {
        let receiver = union_of(Ty::Int, unknown);
        let (yes, no) = narrowing::head_split(&receiver, &Ty::Str);
        assert_eq!(
            yes,
            Ty::Str,
            "a successful String test bounds this occurrence"
        );
        assert!(
            no.mentions_unknown(),
            "the negative branch still admits other unknown values"
        );
        assert!(
            receiver.mentions_unknown(),
            "branch refinement must not mutate the carried slot"
        );
    }
}

#[test]
#[ignore = "new origin-aware candidate contract, explicitly enabled"]
fn producer_reason_provenance_contract_reason_sets() {
    use std::{collections::BTreeSet, sync::Arc};
    fn wrap(kind: usize, t: Ty) -> Ty {
        match kind {
            0 => t,
            1 => Ty::Array { elem: Arc::new(t) },
            2 => Ty::Hash {
                key: Arc::new(Ty::Str),
                value: Arc::new(t),
            },
            3 => Ty::Tuple {
                elems: vec![Ty::Int, t].into(),
            },
            4 => Ty::Class {
                id: ClassId(Symbol::from("Box")),
                args: vec![t].into(),
            },
            5 => Ty::Record {
                row: crate::ty::Row {
                    fields: vec![(Symbol::from("x"), t)].into_iter().collect(),
                    rest: None,
                },
            },
            _ => Ty::Fn {
                params: vec![].into(),
                block: None,
                ret: Arc::new(t),
                effects: crate::effect::EffectSet::default(),
            },
        }
    }
    let input = [Ty::pending_untyped(), Ty::unresolved(), Ty::gradual()];
    let mut checks = 0;
    for kind in 0..7 {
        for a in &input {
            for b in &input {
                for c in &input {
                    let (a, b, c) = (
                        wrap(kind, a.clone()),
                        wrap(kind, b.clone()),
                        wrap(kind, c.clone()),
                    );
                    let reasons = |t: &Ty| {
                        crate::analyze::producer_evidence::origins(t)
                            .into_iter()
                            .map(|s| s.rsplit(':').next().unwrap().to_owned())
                            .collect::<BTreeSet<_>>()
                    };
                    let expected = reasons(&a)
                        .union(&reasons(&b))
                        .cloned()
                        .collect::<BTreeSet<_>>()
                        .union(&reasons(&c))
                        .cloned()
                        .collect::<BTreeSet<_>>();
                    let wire = serde_json::to_value(&a).unwrap();
                    for (x, y, z) in [
                        (&a, &b, &c),
                        (&a, &c, &b),
                        (&b, &a, &c),
                        (&b, &c, &a),
                        (&c, &a, &b),
                        (&c, &b, &a),
                    ] {
                        let left = union_of(union_of(x.clone(), y.clone()), z.clone());
                        let right = union_of(x.clone(), union_of(y.clone(), z.clone()));
                        assert_eq!(reasons(&left), expected);
                        assert_eq!(reasons(&right), expected);
                        assert_eq!(serde_json::to_value(&left).unwrap(), wire);
                        assert_eq!(reasons(&union_of(left.clone(), x.clone())), expected);
                        // Exercise the memo explicitly, even though S3 bypasses it.
                        let memo =
                            crate::join_memo::join(x.clone(), y.clone(), |x, y| union_of_raw(x, y));
                        assert_eq!(
                            reasons(&memo),
                            reasons(x).union(&reasons(y)).cloned().collect()
                        );
                        let stored: crate::shared::Shared<Vec<Ty>> = vec![left.clone()].into();
                        assert_eq!(reasons(&stored[0]), expected);
                        assert!(crate::wire_ty::storage_types(
                            std::slice::from_ref(&left),
                            std::slice::from_ref(&right)
                        ));
                        checks += 1;
                    }
                }
            }
        }
    }
    println!(
        "PROVENANCE-EVIDENCE {{\"shapes\":7,\"permutation_cases\":{checks},\"lost_origins\":0}}"
    );
}

#[test]
#[ignore = "new stable producer-id candidate audit, explicitly enabled"]
fn producer_reason_provenance_contract_writer_ids() {
    use crate::{
        analyze::producer_evidence as audit,
        span::{FileId, SourceFile, Span},
    };
    fn one(extra_file: bool) -> serde_json::Value {
        let source = SourceFile {
            path: "fixture.rb".into(),
            text: "def m; unknown; end".into(),
        };
        let files = if extra_file {
            vec![
                SourceFile {
                    path: "irrelevant.rb".into(),
                    text: "".into(),
                },
                source,
            ]
        } else {
            vec![source]
        };
        audit::reset(&files);
        let span = Span {
            file: FileId(if extra_file { 2 } else { 1 }),
            start: 0,
            end: 19,
        };
        let _guard = audit::writer(
            &ClassId(Symbol::from("Owner")),
            &Symbol::from("m"),
            false,
            span,
        );
        for t in [Ty::pending_untyped(), Ty::unresolved(), Ty::gradual()] {
            audit::input(Some(&t));
        }
        audit::snapshot()
    }
    let first = one(false);
    let repeat = one(false);
    let changed_index = one(true);
    assert_eq!(first["writers"], repeat["writers"]);
    assert_eq!(first["writers"], changed_index["writers"]);
    assert_eq!(first["visits"], repeat["visits"]);
    assert_eq!(
        first["writers"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(first["unscoped_inputs"], 0);
    println!("PROVENANCE-WRITERS {first}");
}

#[test]
#[ignore = "new raw parameter entrance conservation contract, explicitly enabled"]
fn producer_reason_provenance_contract_parameter_entrance() {
    use crate::analyze::producer_evidence as audit;
    audit::reset(&[crate::span::SourceFile {
        path: "fixture.rb".into(),
        text: "call(x)".into(),
    }]);
    let owner = ClassId(Symbol::from("Owner"));
    let mut arg = crate::expr::Expr::new(
        crate::span::Span::synthetic(),
        crate::expr::ExprNode::Lit {
            value: crate::expr::Literal::Nil,
        },
    );
    arg.ty = Some(Ty::gradual());
    audit::parameter_call(
        Some(&owner),
        std::slice::from_ref(&owner),
        &Symbol::from("call"),
        "Instance",
        crate::span::Span {
            file: crate::span::FileId(1),
            start: 0,
            end: 7,
        },
        &[arg],
    );
    let mut other_arg = crate::expr::Expr::new(
        crate::span::Span::synthetic(),
        crate::expr::ExprNode::Lit {
            value: crate::expr::Literal::Nil,
        },
    );
    other_arg.ty = Some(Ty::gradual());
    audit::parameter_call(
        Some(&owner),
        &[ClassId(Symbol::from("ChangedRoute"))],
        &Symbol::from("call"),
        "Instance",
        crate::span::Span {
            file: crate::span::FileId(1),
            start: 0,
            end: 7,
        },
        &[other_arg],
    );
    let output = crate::analyze::unify_param_ty(Ty::Int, Ty::gradual());
    assert_eq!(
        output,
        Ty::Int,
        "observational phase must retain the old absorption policy"
    );
    let snapshot = audit::snapshot();
    assert_eq!(snapshot["parameter_type_origin_losses"], 1);
    assert_eq!(
        snapshot["writers"].as_object().unwrap().len(),
        1,
        "route discovery changed the writer identity"
    );
    assert!(
        snapshot["writers"].as_object().unwrap().values().any(|v| v
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "$:Gradual")),
        "raw producer evidence disappeared"
    );
    println!("PROVENANCE-PARAMETERS {snapshot}");
}

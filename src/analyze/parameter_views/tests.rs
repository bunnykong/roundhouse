use super::*;
use crate::{
    dialect::MethodReceiver,
    ident::{Symbol, TyVar},
    span::FileId,
};

fn setup() -> (ClassId, Symbol) {
    assert!(
        on() && fold::on(),
        "run native laws with the frozen research flags"
    );
    assert_eq!(std::env::var("RH_PROVENANCE_READER_VIEW").as_deref(), Ok("1"));
    fold::reset();
    reset(&[SourceFile {
        path: "writers.rb".into(),
        text: String::new(),
    }]);
    let c = ClassId(Symbol::from("ViewsLaw"));
    let m = Symbol::from("run");
    fold::add_rec_methods(vec![(c.clone(), m.clone())]);
    (c, m)
}
fn span(w: u32) -> Span {
    Span {
        file: FileId(1),
        start: w * 10,
        end: w * 10 + 9,
    }
}
fn key(c: &ClassId, m: &Symbol) -> SlotKey {
    SlotKey::Param {
        class: c.clone(),
        method: m.clone(),
        side: MethodReceiver::Instance,
        index: 0,
    }
}
fn seed(c: &ClassId, m: &Symbol, w: u32, raw: Option<Ty>) -> Option<Ty> {
    fold::declare_param(c, m, MethodReceiver::Instance, 0, span(w));
    let control = raw.as_ref().map(|t| view(t.clone()));
    fold::param_seed(c, m, MethodReceiver::Instance, 0, span(w), raw, control)
}
fn stored(c: &ClassId, m: &Symbol) -> Option<Ty> {
    fold::slot_id(&key(c, m)).and_then(|s| fold::value_of(s, &HashMap::new()))
}
fn check(a: Ty, b: Ty) {
    assert!(same(&a, &b), "{a:?} != {b:?}");
}

#[test]
#[ignore = "RETURN_HELPERS advance prediction native law, research flags required"]
fn current_return_views_same_writer_replaces() {
    let (c, m) = setup();
    seed(&c, &m, 1, Some(Ty::Int));
    seed(&c, &m, 1, Some(Ty::Str));
    check(stored(&c, &m).unwrap(), Ty::Str);
    LEDGER.with(|l| {
        assert_eq!(
            l.borrow().destinations[&destination(&key(&c, &m))]
                .cells
                .len(),
            1
        )
    });
}
#[test]
#[ignore = "RETURN_HELPERS advance prediction native law, research flags required"]
fn current_return_views_second_writer_survives() {
    let (c, m) = setup();
    seed(&c, &m, 1, Some(Ty::Int));
    seed(&c, &m, 2, Some(Ty::Str));
    seed(&c, &m, 1, Some(Ty::Nil));
    check(stored(&c, &m).unwrap(), view(pc::join(Ty::Nil, Ty::Str)));
}
#[test]
#[ignore = "RETURN_HELPERS advance prediction native law, research flags required"]
fn current_return_views_permutations_agree() {
    for order in [
        vec![1, 2],
        vec![2, 1],
        vec![1, 2, 3],
        vec![1, 3, 2],
        vec![2, 1, 3],
        vec![2, 3, 1],
        vec![3, 1, 2],
        vec![3, 2, 1],
    ] {
        let (c, m) = setup();
        for w in &order {
            seed(
                &c,
                &m,
                *w,
                Some(match w {
                    1 => Ty::Int,
                    2 => Ty::Str,
                    _ => Ty::gradual(),
                }),
            );
        }
        let expected = if order.len() == 2 {
            pc::join(Ty::Int, Ty::Str)
        } else {
            pc::join(pc::join(Ty::Int, Ty::Str), Ty::gradual())
        };
        check(stored(&c, &m).unwrap(), view(expected));
    }
}
#[test]
#[ignore = "RETURN_HELPERS advance prediction native law, research flags required"]
fn current_return_views_pending_identity_normalizer() {
    let _ = setup();
    let samples = vec![
        Ty::Bottom,
        Ty::pending_untyped(),
        Ty::Int,
        Ty::Str,
        Ty::Nil,
        Ty::unresolved(),
        Ty::gradual(),
        Ty::Var { var: TyVar(0) },
        Ty::Array {
            elem: Arc::new(Ty::Str),
        },
        pc::join(Ty::Int, Ty::unresolved()),
    ];
    for a in &samples {
        check(
            pc::normalize(pc::normalize(a.clone())),
            pc::normalize(a.clone()),
        );
        check(
            current_join(Ty::pending_untyped(), a.clone()),
            pc::normalize(a.clone()),
        );
        for b in &samples {
            for c in &samples {
                check(
                    current_join(current_join(a.clone(), b.clone()), c.clone()),
                    current_join(a.clone(), current_join(b.clone(), c.clone())),
                );
            }
        }
    }
}
#[test]
#[ignore = "RETURN_HELPERS advance prediction native law, research flags required"]
fn current_return_views_view_of_current_never_of_history() {
    for history in [
        Ty::Int,
        Ty::unresolved(),
        Ty::Nil,
        pc::join(Ty::gradual(), Ty::Nil),
    ] {
        let (c, m) = setup();
        seed(&c, &m, 1, Some(history));
        let now = pc::join(Ty::Str, Ty::unresolved());
        seed(&c, &m, 1, Some(now.clone()));
        check(stored(&c, &m).unwrap(), view(now));
    }
}
#[test]
#[ignore = "RETURN_HELPERS advance prediction native law, research flags required"]
fn current_return_views_absent_row_is_identity_no_slot() {
    let (c, m) = setup();
    assert!(seed(&c, &m, 1, None).is_none());
    assert!(fold::slot_id(&key(&c, &m)).is_none());
    assert!(seed(&c, &m, 1, Some(Ty::pending_untyped())).is_some());
    let slot = fold::slot_id(&key(&c, &m)).unwrap();
    check(stored(&c, &m).unwrap(), Ty::pending_untyped());
    seed(&c, &m, 1, Some(Ty::Rec { slot }));
    check(stored(&c, &m).unwrap(), Ty::pending_untyped());
    seed(&c, &m, 1, Some(Ty::Str));
    seed(&c, &m, 1, None);
    assert_eq!(fold::slot_id(&key(&c, &m)), Some(slot));
    check(stored(&c, &m).unwrap(), Ty::pending_untyped());
}
#[test]
#[ignore = "RETURN_HELPERS advance prediction native law, research flags required"]
fn current_return_views_destination_keys_preserved() {
    // Compare the unchanged C allocation path to V, including self stripping.
    let (c, m) = setup();
    let r = fold::param_ref(&c, &m, MethodReceiver::Instance, 0, Ty::pending_untyped());
    fold::param_ref(&c, &m, MethodReceiver::Instance, 0, r);
    let c_keys = fold::contributor_values()
        .into_iter()
        .map(|(k, _)| k)
        .collect::<BTreeSet<_>>();
    let (c, m) = setup();
    let r = seed(&c, &m, 1, Some(Ty::pending_untyped())).unwrap();
    seed(&c, &m, 1, Some(r));
    seed(&c, &m, 1, None);
    let v_keys = fold::contributor_values()
        .into_iter()
        .map(|(k, _)| k)
        .collect::<BTreeSet<_>>();
    assert_eq!(c_keys, v_keys);
}
#[test]
#[ignore = "RETURN_HELPERS advance prediction native law, research flags required"]
fn current_return_views_reseed_is_same_writer() {
    let (c, m) = setup();
    seed(&c, &m, 7, Some(Ty::Str));
    seed(&c, &m, 7, Some(Ty::Int));
    LEDGER.with(|l| {
        let l = l.borrow();
        assert_eq!(l.declared.len(), 1);
        assert_eq!(l.declared, l.observed);
        assert_eq!(l.destinations[&destination(&key(&c, &m))].cells.len(), 1);
    });
    check(stored(&c, &m).unwrap(), Ty::Int);
}

#[test]
#[ignore = "RETURN_HELPERS binding-note regression, research flags required"]
fn current_return_views_single_writer_default_projection() {
    let (c, m) = setup();
    let control = super::super::param_ty_with_default(
        Some(view(Ty::unresolved())),
        &crate::dialect::Param {
            name: Symbol::from("p"),
            default: Some(crate::expr::Expr {
                ty: Some(Ty::Str),
                ..crate::expr::Expr::new(
                    Span::synthetic(),
                    crate::expr::ExprNode::Lit {
                        value: crate::expr::Literal::Nil,
                    },
                )
            }),
            keyword: false,
            rest: false,
            forwarding: false,
            from_keyword: false,
            from_kwrest: false,
        },
    );
    // The native incorporation function is also covered at its existing unit tests;
    // this witness pins the ordering boundary without an additional writer.
    assert!(
        origins(control.as_ref().unwrap())
            .iter()
            .any(|o| o.ends_with(":Unresolved"))
    );
    let raw = pc::join(Ty::unresolved(), Ty::Str);
    fold::declare_param(&c, &m, MethodReceiver::Instance, 0, span(1));
    fold::param_seed(
        &c,
        &m,
        MethodReceiver::Instance,
        0,
        span(1),
        Some(raw),
        control,
    );
    check(stored(&c, &m).unwrap(), Ty::Str);
}

#[test]
#[ignore = "RETURN_HELPERS binding-note regression, research flags required"]
fn current_return_views_raw_origins_survive_before_view() {
    let (c, m) = setup();
    seed(&c, &m, 1, Some(Ty::unresolved()));
    seed(&c, &m, 2, Some(Ty::Str));
    let raw = LEDGER.with(|l| {
        l.borrow().destinations[&destination(&key(&c, &m))]
            .aggregate
            .clone()
    });
    let raw = record(&raw, &HashMap::new(), &mut true);
    assert!(
        raw["resolved_origins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o.as_str().unwrap().ends_with(":Unresolved"))
    );
    check(stored(&c, &m).unwrap(), Ty::Str);
    assert!(origins(&stored(&c, &m).unwrap()).is_empty());
}

use super::*;
use crate::{
    dialect::MethodReceiver,
    expr::{ExprNode, Literal},
    ident::{ClassId, Symbol, TyVar},
    span::Span,
    ty::{Param, ParamKind, Provenance, Row},
};

fn var() -> Ty {
    Ty::Var { var: TyVar(0) }
}
fn union(variants: Vec<Ty>) -> Ty {
    Ty::Union {
        variants: variants.into(),
    }
}
fn hash(value: Ty) -> Ty {
    Ty::Hash {
        key: Arc::new(var()),
        value: Arc::new(value),
    }
}
fn norm(t: &Ty) -> Ty {
    canonical(t, &mut HashMap::new())
}
fn same(a: &Ty, b: &Ty) -> bool {
    crate::wire_ty::storage_types(std::slice::from_ref(a), std::slice::from_ref(b))
}
fn typed(t: Ty) -> Expr {
    let mut e = Expr::new(
        Span::synthetic(),
        ExprNode::Lit {
            value: Literal::Nil,
        },
    );
    e.ty = Some(t);
    e
}

#[test]
#[ignore = "cause test requires RH_CURRENT_CALL_ROWS=1"]
fn canonical_unknown_argument_tagging_depends_on_container_order() {
    assert!(super::super::parameter_conservation::row_current_on());
    let argument = super::super::parameter_conservation::argument;
    let bare = argument(&typed(var()));
    assert!(matches!(
        bare,
        Ty::Untyped {
            why: Provenance::Unresolved
        }
    ));
    let early = hash(union(vec![Ty::Int, bare]));
    let late = argument(&typed(hash(union(vec![Ty::Int, var()]))));
    assert!(
        !same(&early, &late),
        "tagging acts only before the Var is nested"
    );
    assert!(
        same(&norm(&early), &norm(&late)),
        "stored values must agree"
    );
}

#[test]
fn canonical_unknown_nested_conversion_orders_agree() {
    // Forem's stats accumulator: early bare argument tagging introduces U;
    // later container construction can retain a nested Var, plus a second U.
    let early = hash(union(vec![Ty::Int, Ty::unresolved()]));
    let late = hash(union(vec![Ty::Int, var(), Ty::unresolved()]));
    assert!(!same(&early, &late));
    assert!(same(&norm(&early), &norm(&late)));
    assert!(same(&norm(&norm(&late)), &norm(&late)));
    // Structural empty-container holes are not evidence of an unknown value.
    assert!(same(&norm(&hash(var())), &hash(var())));
}

#[test]
fn canonical_unknown_empty_hash_restamp_retains_first_unknown_representation() {
    use crate::{
        analyze::{BodyTyper, ClassInfo, Ctx},
        expr::LValue,
        ident::VarId,
    };
    let synth = |node| Expr::new(Span::synthetic(), node);
    let local = || {
        synth(ExprNode::Var {
            id: VarId(1),
            name: Symbol::from("stats"),
        })
    };
    let body = || {
        synth(ExprNode::Seq {
            exprs: vec![
                synth(ExprNode::Assign {
                    target: LValue::Var {
                        id: VarId(1),
                        name: Symbol::from("stats"),
                    },
                    value: synth(ExprNode::Hash {
                        entries: vec![],
                        kwargs: false,
                    }),
                }),
                synth(ExprNode::Assign {
                    target: LValue::Index {
                        recv: local(),
                        index: synth(ExprNode::Lit {
                            value: Literal::Sym {
                                value: Symbol::from("day"),
                            },
                        }),
                    },
                    value: synth(ExprNode::Send {
                        recv: None,
                        method: Symbol::from("read"),
                        args: vec![],
                        block: None,
                        parenthesized: true,
                    }),
                }),
                local(),
            ],
        })
    };
    let class = ClassId(Symbol::from("CanonAccumulator"));
    let ctx = Ctx {
        self_ty: Some(Ty::Class {
            id: class.clone(),
            args: vec![].into(),
        }),
        ..Default::default()
    };
    let mut first_raw = body();
    let mut first_tagged = body();
    let mut classes = HashMap::from([(class.clone(), ClassInfo::default())]);
    for (tree, first) in [
        (&mut first_raw, var()),
        (&mut first_tagged, Ty::unresolved()),
    ] {
        // The same final registry, but different answers before the first restamp.
        for unknown in [first, Ty::unresolved()] {
            classes
                .get_mut(&class)
                .unwrap()
                .instance_methods
                .insert(Symbol::from("read"), hash(union(vec![Ty::Int, unknown])));
            BodyTyper::new(&classes).analyze_expr(tree, &ctx);
        }
    }
    let seed_ty = |tree: &Expr| {
        let ExprNode::Seq { exprs } = &*tree.node else {
            unreachable!()
        };
        let ExprNode::Assign { value, .. } = &*exprs[0].node else {
            unreachable!()
        };
        value.ty.clone().unwrap()
    };
    assert!(!same(&seed_ty(&first_raw), &seed_ty(&first_tagged)));
    assert!(!same(
        first_raw.ty.as_ref().unwrap(),
        first_tagged.ty.as_ref().unwrap()
    ));
    expr(&mut first_raw);
    expr(&mut first_tagged);
    assert!(same(&seed_ty(&first_raw), &seed_ty(&first_tagged)));
    assert!(same(
        first_raw.ty.as_ref().unwrap(),
        first_tagged.ty.as_ref().unwrap()
    ));
}

#[test]
fn canonical_unknown_preserves_pending_and_every_container() {
    let leaf = union(vec![Ty::Int, var(), Ty::pending_untyped()]);
    let want = norm(&leaf);
    let Ty::Union { variants } = &want else {
        panic!("kept union")
    };
    assert!(variants.iter().any(|t| matches!(
        t,
        Ty::Untyped {
            why: Provenance::Pending
        }
    )));
    assert!(variants.iter().any(|t| matches!(
        t,
        Ty::Untyped {
            why: Provenance::Unresolved
        }
    )));
    assert!(same(&norm(&Ty::pending_untyped()), &Ty::pending_untyped()));
    let wrap = |t: Ty| Ty::Fn {
        params: vec![Param {
            name: Symbol::from("p"),
            kind: ParamKind::Required,
            ty: Arc::new(Ty::Array {
                elem: Arc::new(t.clone()),
            }),
        }]
        .into(),
        block: Some(Arc::new(Ty::Tuple {
            elems: vec![t.clone()].into(),
        })),
        ret: Arc::new(Ty::Class {
            id: ClassId(Symbol::from("Box")),
            args: vec![Ty::Record {
                row: Row {
                    fields: vec![(Symbol::from("v"), hash(t))].into_iter().collect(),
                    rest: None,
                },
            }]
            .into(),
        }),
        effects: Default::default(),
    };
    assert!(same(&norm(&wrap(leaf)), &wrap(want)));
}

#[test]
fn canonical_unknown_rewrites_native_storage() {
    let mut app = App::new();
    let mut analyzer = Analyzer::new(&app);
    let class = ClassId(Symbol::from("Canon"));
    let method = Symbol::from("run");
    let binding = Symbol::from("@stats");
    let key = (class.clone(), method.clone(), MethodReceiver::Instance);
    let before = hash(union(vec![Ty::Int, var()]));
    let after = hash(union(vec![Ty::Int, Ty::unresolved()]));
    analyzer
        .classes
        .entry(class.clone())
        .or_default()
        .instance_methods
        .insert(method.clone(), before.clone());
    analyzer
        .inferred_params
        .insert(key.clone(), vec![before.clone()]);
    app.inferred_method_params
        .insert((class.clone(), method.clone()), vec![before.clone()]);
    app.seeds = Some(typed(before.clone()));
    analyzer.controller_action_meta_cache.insert(
        class.clone(),
        (
            HashMap::from([(
                method.clone(),
                HashMap::from([(binding.clone(), before.clone())]),
            )]),
            HashMap::from([(method.clone(), typed(before.clone()))]),
        ),
    );
    analyzer.refined_action_bindings.insert(
        (class.clone(), method.clone()),
        HashMap::from([(binding.clone(), before)]),
    );
    rewrite_storage(&mut analyzer, &mut app);
    assert!(same(
        &analyzer.classes[&class].instance_methods[&method],
        &after
    ));
    assert!(same(&analyzer.inferred_params[&key][0], &after));
    assert!(same(
        &app.inferred_method_params[&(class.clone(), method.clone())][0],
        &after
    ));
    assert!(same(
        app.seeds.as_ref().unwrap().ty.as_ref().unwrap(),
        &after
    ));
    let (bindings, bodies) = &analyzer.controller_action_meta_cache[&class];
    assert!(same(&bindings[&method][&binding], &after));
    assert!(same(bodies[&method].ty.as_ref().unwrap(), &after));
    assert!(same(
        &analyzer.refined_action_bindings[&(class, method)][&binding],
        &after
    ));
}

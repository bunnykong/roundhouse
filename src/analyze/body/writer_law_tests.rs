use super::*;
use crate::analyze::writer_laws as laws;
use crate::ident::VarId;

fn lambda(body: Expr) -> Expr {
    laws::expr(ExprNode::Lambda {
        params: vec![Symbol::from("x"), Symbol::from("y")],
        rest_param: None,
        extra_params: vec![],
        block_param: None,
        body,
        block_style: Default::default(),
    })
}

fn local() -> Expr {
    laws::expr(ExprNode::Var {
        id: VarId(0),
        name: Symbol::from("x"),
    })
}

fn row(ctx: &Ctx) -> Ty {
    Ty::Tuple {
        elems: ["x", "y"]
            .iter()
            .map(|n| {
                ctx.local_bindings
                    .get(&Symbol::from(*n))
                    .cloned()
                    .unwrap_or_else(laws::pending)
            })
            .collect(),
    }
}

fn join_rows(a: Ty, b: Ty) -> Ty {
    let (Ty::Tuple { elems: a }, Ty::Tuple { elems: b }) = (a, b) else {
        unreachable!()
    };
    Ty::Tuple {
        elems: a
            .into_iter()
            .zip(b)
            .map(|(a, b)| join_ivar_slot(a, b))
            .collect(),
    }
}

fn check_closure(method: &str) {
    let classes = HashMap::new();
    let typer = BodyTyper::new(&classes);
    let block = lambda(laws::stamped(Ty::Nil));
    let method = Symbol::from(method);
    let write = |ctx: &Ctx, t: &Ty| typer.block_ctx_for(ctx, Some(t), &method, &[], false, &block);
    let transfer = |t: &Ty| row(&write(&Ctx::default(), t));
    laws::check_transfer(
        &laws::universe(),
        transfer,
        join_rows,
        row(&Ctx::default()),
        |t| {
            let once = write(&Ctx::default(), t);
            let twice = write(&once, t);
            (row(&once), row(&twice))
        },
    )
    .finish(&format!("block_ctx_for/{}", method.as_str()));
}

#[test]
#[ignore = "known: block parameter selection from receiver alternatives does not preserve joins"]
fn writer_law_closure_each() {
    check_closure("each");
}

#[test]
#[ignore = "known: block parameter selection from receiver alternatives does not preserve joins"]
fn writer_law_closure_each_with_index() {
    check_closure("each_with_index");
}

#[test]
#[ignore = "known: block parameter selection from receiver alternatives does not preserve joins"]
fn writer_law_closure_then() {
    check_closure("then");
}

/// All three collectors really walk into the closure. They feed the Seq
/// accumulator's union/refinement and retro-stamp, not a model of that gate.
fn collected(t: &Ty, kind: usize) -> Ty {
    let write = match kind {
        0 => laws::expr(ExprNode::Send {
            recv: Some(local()),
            method: Symbol::from("push"),
            args: vec![laws::stamped(t.clone())],
            block: None,
            parenthesized: false,
        }),
        1 => laws::expr(ExprNode::Assign {
            target: LValue::Index {
                recv: local(),
                index: laws::stamped(Ty::Str),
            },
            value: laws::stamped(t.clone()),
        }),
        _ => laws::expr(ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: Symbol::from("x"),
            },
            value: laws::stamped(t.clone()),
        }),
    };
    let block = lambda(write);
    let mut types = Vec::new();
    match kind {
        0 => {
            let mut out = vec![];
            collect_array_pushes(&block, &mut out);
            types.extend(out.into_iter().map(|(_, _, t)| t));
        }
        1 => {
            let mut out = vec![];
            collect_hash_index_writes(&block, &mut out);
            types.extend(out.into_iter().map(|(_, _, t)| t));
        }
        _ => {
            let mut out = vec![];
            collect_local_assignment_tys(&block, &mut out);
            types.extend(out.into_iter().map(|(_, t)| t));
        }
    }
    types
        .into_iter()
        .reduce(join_ivar_slot)
        .unwrap_or_else(laws::pending)
}

fn check_collector(kind: usize, name: &str) {
    let f = |t: &Ty| collected(t, kind);
    laws::check_transfer(
        &laws::universe(),
        &f,
        join_ivar_slot,
        laws::pending(),
        |t| {
            let once = f(t);
            let twice = f(&once);
            (once, twice)
        },
    )
    .finish(name);
}

#[test]
fn writer_law_closure_array_pushes() {
    check_collector(0, "collect_array_pushes");
}

#[test]
fn writer_law_closure_hash_writes() {
    check_collector(1, "collect_hash_index_writes");
}

#[test]
fn writer_law_closure_local_writes() {
    check_collector(2, "collect_local_assignment_tys");
}

#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_condition_local_overwrite() {
    laws::check_binary("collect_var_assignments_into", &laws::universe(), |a, b| {
        let mut out = HashMap::from([(Symbol::from("x"), a)]);
        collect_var_assignments_into(
            &laws::expr(ExprNode::Assign {
                target: LValue::Var {
                    id: VarId(0),
                    name: Symbol::from("x"),
                },
                value: laws::stamped(b),
            }),
            &mut out,
        );
        out.remove(&Symbol::from("x")).unwrap()
    });
}

fn check_stamp(array: bool) {
    let classes = HashMap::new();
    let typer = BodyTyper::new(&classes);
    let samples: Vec<Ty> = laws::universe()
        .into_iter()
        .map(|t| {
            if array {
                laws::arr(t)
            } else {
                laws::hash(Ty::Str, t)
            }
        })
        .collect();
    laws::check_binary(
        if array {
            "empty_array/stamp_readback"
        } else {
            "empty_hash/stamp_readback"
        },
        &samples,
        |a, b| {
            let mut literal = laws::expr(if array {
                ExprNode::Array {
                    elements: vec![],
                    style: Default::default(),
                }
            } else {
                ExprNode::Hash {
                    entries: vec![],
                    kwargs: false,
                }
            });
            literal.ty = Some(a);
            propagate_expected_to_empty_container(&mut literal, &b);
            typer.analyze_expr(&mut literal, &Ctx::default())
        },
    );
}

#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_empty_array_stamp() {
    check_stamp(true);
}

#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_empty_hash_stamp() {
    check_stamp(false);
}

fn check_match(required: bool) {
    let classes = HashMap::new();
    let typer = BodyTyper::new(&classes);
    let name = Symbol::from("x");
    laws::check_binary(
        if required {
            "propagate_match_bindings/required"
        } else {
            "propagate_match_bindings/conditional"
        },
        &laws::universe(),
        |a, b| {
            let mut ctx = Ctx::default();
            ctx.local_bindings.insert(name.clone(), a);
            let pattern = MatchPattern::Bind { name: name.clone() };
            let write = laws::expr(if required {
                ExprNode::MatchRequired {
                    value: laws::stamped(b),
                    pattern,
                }
            } else {
                ExprNode::MatchPredicate {
                    value: laws::stamped(b),
                    pattern,
                }
            });
            typer.propagate_match_bindings(&write, &mut ctx, true);
            ctx.local_bindings.remove(&name).unwrap()
        },
    );
}

#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_match_required() {
    check_match(true);
}

#[test]
#[ignore = "known: conditional pattern union retains pending arms"]
fn writer_law_match_conditional() {
    check_match(false);
}

#[test]
#[ignore = "known: this writer replaces the destination; contribution order and replay change it"]
fn writer_law_ir_type_overwrite() {
    let classes = HashMap::new();
    let typer = BodyTyper::new(&classes);
    laws::check_binary(
        "BodyTyper::analyze_expr/ordinary-stamp",
        &laws::universe(),
        |a, b| {
            let mut e = local();
            e.ty = Some(a);
            let mut ctx = Ctx::default();
            ctx.local_bindings.insert(Symbol::from("x"), b);
            let returned = typer.analyze_expr(&mut e, &ctx);
            assert_eq!(e.ty.as_ref(), Some(&returned));
            e.ty.unwrap()
        },
    );
}

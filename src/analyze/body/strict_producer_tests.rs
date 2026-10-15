//! advance prediction protocol probes. Every count is printed before the combined assertion.
use super::*;
use crate::analyze::{handoff, harvest_return, strict_producers as strict};
use crate::span::Span;
use crate::ty::Provenance;

fn carries_evidence(t: &Ty) -> bool {
    match t {
        Ty::Untyped { why } => why.reporting_bits() & 6 != 0,
        Ty::Array { elem } => carries_evidence(elem),
        Ty::Hash { key, value } => carries_evidence(key) || carries_evidence(value),
        Ty::Tuple { elems } | Ty::Union { variants: elems } | Ty::Class { args: elems, .. } =>
            elems.iter().any(carries_evidence),
        Ty::Record { row } => row.fields.values().any(carries_evidence),
        Ty::Fn { params, block, ret, .. } => params.iter().any(|p| carries_evidence(&p.ty))
            || block.as_ref().is_some_and(|b| carries_evidence(b)) || carries_evidence(ret),
        _ => false,
    }
}
fn storage_eq(a: &Ty, b: &Ty) -> bool {
    crate::wire_ty::storage_types(std::slice::from_ref(a), std::slice::from_ref(b))
}
fn raw_union(a: Ty, b: Ty) -> Ty { Ty::Union { variants: vec![a, b].into() } }

#[test]
#[ignore]
fn provenance_strict_producer_strictness() {
    let classes = HashMap::new();
    let typer = BodyTyper::new(&classes);
    let p = Ty::pending_untyped();
    let g = Ty::gradual();
    let u = Ty::unresolved();
    let name = Symbol::from("x");
    let block = Expr::new(Span::synthetic(), ExprNode::Lambda {
        params: vec![name.clone()], rest_param: None, extra_params: vec![], block_param: None,
        body: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        block_style: Default::default(),
    });
    let ctx = typer.block_ctx_for(&Ctx::default(), Some(&p), &Symbol::from("each"), &[], false, &block);
    let p1 = usize::from(carries_evidence(&ctx.local_bindings[&name]));
    let p2 = usize::from(carries_evidence(&multiassign_target_ty(&Some(p.clone()), 0).unwrap()));
    let p3 = usize::from(carries_evidence(&typer.dispatch_on(Some(&p), &Symbol::from("to_h"), None, &[], false)));
    let mut arg = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
    arg.ty = Some(p.clone());
    let p4 = usize::from(carries_evidence(&typer.dispatch_on(Some(&Ty::Date), &Symbol::from("+"), None, &[arg], true)));
    let array = Ty::Array { elem: std::sync::Arc::new(p.clone()) };
    let p5 = usize::from(carries_evidence(&typer.dispatch_on(Some(&array), &Symbol::from("sum"), None, &[], true)));
    let p6 = usize::from(carries_evidence(&handoff::join(raw_union(Ty::Nil, p.clone()), Ty::Nil)));
    let p7 = usize::from(carries_evidence(&raw_union(Ty::Var { var: TyVar(0) }, p.clone()).strip_unknown()));
    let p7b = usize::from(carries_evidence(&raw_union(p.clone(), p.clone()).strip_unknown()));
    let p8 = usize::from(!storage_eq(&Ty::canonical_min(p.clone(), g.clone()), &g))
        + usize::from(!storage_eq(&Ty::canonical_min(g.clone(), p.clone()), &g));
    let method = Symbol::from("m");
    let mut table = HashMap::from([(method.clone(), p)]);
    harvest_return::insert_inferred_return(&mut table, &method, u);
    let p9 = usize::from(!storage_eq(&table[&method], &Ty::unresolved()));
    let gu = Ty::untyped(Provenance::Gradual.union(Provenance::Unresolved));
    let p10 = !strict::same(&raw_union(g, Ty::Nil), &raw_union(gu, Ty::Nil));
    let stubs = usize::from(strict::stubs_on());
    let xfer = usize::from(strict::xfer_on());
    let congruent = usize::from(strict::congruent_on());
    let got = serde_json::json!({
        "flags": {"stubs":stubs,"xfer":xfer,"congruent":congruent},
        "strictness":{"p1":p1,"p2":p2,"p3":p3,"p4":p4,"p5":p5,"p6":p6,"p7b":p7b},
        "deferred_var":{"p7":p7},"identity":{"p8":p8,"p9":p9},"equality":{"p10_distinguishes":p10}
    });
    println!("provenance-STRICT {got}");
    assert_eq!([p1,p2,p3,p4,p5,p6,p7b], [usize::from(xfer == 0); 7]);
    assert_eq!(p7, 1);
    assert_eq!(p8, usize::from(xfer == 0));
    assert_eq!(p9, usize::from(stubs == 0 && xfer == 0));
    assert_eq!(p10, congruent == 1);
}

/// The card's transfer rule includes noncanonical all-P unions as well as bare P.
#[test]
#[ignore]
fn provenance_strict_producer_strictness_union_shapes() {
    if !strict::xfer_on() { return; }
    let classes = HashMap::new();
    let typer = BodyTyper::new(&classes);
    let pending = raw_union(Ty::pending_untyped(), Ty::pending_untyped());
    let mut arg = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
    arg.ty = Some(pending.clone());
    for method in ["+", "-", ">>", "<<"] {
        let result = typer.dispatch_on(Some(&Ty::Date), &Symbol::from(method), None, &[arg.clone()], true);
        assert!(strict::is_p(&result), "Date {method}: {result:?}");
    }
    let key = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
    let result = super::send::indexed_pluck(&Symbol::from("pluck"), &pending, &[key]).unwrap();
    assert!(matches!(&result, Ty::Array { elem } if strict::is_p(elem)), "{result:?}");
}

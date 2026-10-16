//! Frozen structure collector: history against a fresh typing, using the native IR observer.
use super::*;
use crate::ident::VarId;
use crate::span::{FileId, Span};
use crate::analyze::fixpoint_check::{ir_rows, type_hash};

fn e(node: ExprNode) -> Expr { Expr::new(Span::synthetic(), node) }
fn var(name: &str) -> Expr { e(ExprNode::Var { id: VarId(0), name: name.into() }) }
fn ivar() -> Expr { e(ExprNode::Ivar { name: "@a".into() }) }
fn array() -> Expr { e(ExprNode::Array { elements: vec![], style: Default::default() }) }
fn hash() -> Expr { e(ExprNode::Hash { entries: vec![], kwargs: false }) }
fn assign(name: &str, value: Expr, iv: bool) -> Expr {
    e(ExprNode::Assign { target: if iv { LValue::Ivar { name: name.into() } }
        else { LValue::Var { id: VarId(0), name: name.into() } }, value })
}
fn send(recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
    e(ExprNode::Send { recv: Some(recv), method: method.into(), args, block: None, parenthesized: true })
}
fn seq(exprs: Vec<Expr>) -> Expr { e(ExprNode::Seq { exprs }) }
fn body(case: char, reverse: bool) -> Expr {
    let mut b = match case {
        'A' | 'H' | 'I' => seq(vec![assign("s", array(), false), send(var("s"), "<<", vec![var("x")]), var("s")]),
        'B' | 'J' => seq(vec![assign("h", hash(), false), send(var("h"), "[]=", vec![e(ExprNode::Lit { value: Literal::Sym { value: "k".into() } }), var("x")]), var("h")]),
        'C' => {
            let block = e(ExprNode::Lambda { params: vec![], rest_param: None, extra_params: vec![],
                block_param: None, body: assign("v", var("x"), false), block_style: Default::default() });
            let each = e(ExprNode::Send { recv: Some(var("items")), method: "each".into(), args: vec![],
                block: Some(block), parenthesized: true });
            seq(vec![assign("v", e(ExprNode::Lit { value: Literal::Nil }), false), each, var("v")])
        }
        'D' | 'E' | 'F' => seq(vec![assign("@a", array(), true), send(ivar(), "<<", vec![var("y")]), ivar()]),
        'G' => array(),
        'K' => {
            let (a,b) = if reverse { ("b", "a") } else { ("a", "b") };
            seq(vec![assign("s", array(), false), send(var("s"), "<<", vec![var(a)]), send(var("s"), "<<", vec![var(b)]), var("s")])
        }
        _ => unreachable!(),
    };
    fn spans(b: &mut Expr, n: &mut u32) {
        *n += 1; b.span = Span { file: FileId(1), start: *n, end: *n + 1 };
        b.node.for_each_child_mut(&mut |c| spans(c, n));
    }
    spans(&mut b, &mut 0); b
}
fn contexts(case: char) -> (Ctx, Ctx) {
    let mut c1 = Ctx::default(); let mut c2 = Ctx::default();
    for c in [&mut c1, &mut c2] {
        c.local_bindings.insert("items".into(), Ty::Array { elem: Ty::Int.into() });
        c.local_bindings.insert("a".into(), Ty::Int);
        c.local_bindings.insert("b".into(), Ty::Str);
    }
    c1.local_bindings.insert("x".into(), if case == 'J' { Ty::Str } else { Ty::Int });
    c2.local_bindings.insert("x".into(), if case == 'I' { Ty::Int } else if case == 'J' { union_of(Ty::Str, Ty::Nil) } else { Ty::Str });
    c1.local_bindings.insert("y".into(), Ty::Int); c2.local_bindings.insert("y".into(), Ty::Str);
    if matches!(case, 'D' | 'E') { c1.ivar_bindings.insert("@a".into(), Ty::Array { elem: Ty::Int.into() }); }
    if case == 'D' { c2.ivar_bindings = c1.ivar_bindings.clone(); }
    (c1, c2)
}
fn run(case: char, law: bool) -> serde_json::Value {
    let classes = HashMap::new();
    let typer = BodyTyper::new(&classes).with_literal_law(law);
    let (c1,c2) = contexts(case);
    let mut history = body(case, false);
    let mut fresh = body(case, case == 'K');
    let (hr,fr) = if case == 'K' {
        (typer.analyze_expr(&mut history, &c1), typer.analyze_expr(&mut fresh, &c1))
    } else {
        typer.analyze_expr(&mut history, &c1);
        if case == 'H' { history = history.clone(); }
        (typer.analyze_expr(&mut history, &c2), typer.analyze_expr(&mut fresh, &c2))
    };
    let mut h = vec![]; let mut f = vec![];
    ir_rows(&history, &mut h); ir_rows(&fresh, &mut f);
    if case == 'K' {
        // Only the seed literal and the return: the two programs intentionally reorder sends.
        h.retain(|r| r.1 == 3); f.retain(|r| r.1 == 3);
    }
    let differences: Vec<_> = h.iter().zip(&f).filter(|(a,b)| a != b).collect();
    let first = differences.first().map(|(a,b)| serde_json::json!({"span":[a.0,a.1,a.2], "history":a, "fresh":b}));
    serde_json::json!({"case":case.to_string(),"rule":if law {"law"} else {"legacy"},
        "equal":differences.is_empty() && h.len()==f.len() && type_hash(&hr)==type_hash(&fr),
        "diff_rows":differences.len(),"first_diff":first,"history_return":type_hash(&hr),"fresh_return":type_hash(&fr)})
}
#[test]
fn literal_transfer_purity_census() {
    for case in 'A'..='K' { for law in [false,true] { println!("{}", run(case, law)); } }
}
macro_rules! law_test { ($name:ident,$case:literal) => { #[test] fn $name() {
    let result=run($case,true); assert_eq!(result["equal"],true,"{result}");
} }; }
law_test!(literal_transfer_law_a,'A'); law_test!(literal_transfer_law_b,'B'); law_test!(literal_transfer_law_c,'C');
law_test!(literal_transfer_law_d,'D'); law_test!(literal_transfer_law_e,'E'); law_test!(literal_transfer_law_f,'F');
law_test!(literal_transfer_law_g,'G'); law_test!(literal_transfer_law_h,'H'); law_test!(literal_transfer_law_i,'I');
law_test!(literal_transfer_law_j,'J'); law_test!(literal_transfer_law_k,'K');
#[test]
fn literal_transfer_law_declared_hash_and_legacy_default() {
    let classes=HashMap::new(); let expected=Ty::Hash { key: Ty::Bottom.into(), value: Ty::Bottom.into() };
    let mut h=hash(); h.ty=Some(Ty::Hash {key:Ty::Sym.into(),value:Ty::Int.into()});
    let mut legacy=h.clone();
    assert_eq!(BodyTyper::new(&classes).analyze_expr(&mut legacy,&Ctx::default()),h.ty.clone().unwrap());
    assert_eq!(BodyTyper::new(&classes).with_literal_law(true).with_declared_empty_hash(expected.clone())
        .analyze_expr(&mut h,&Ctx::default()),expected);
}

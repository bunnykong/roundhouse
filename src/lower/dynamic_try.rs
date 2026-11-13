//! `recv.try(name, ...)` whose name is a value (Devise's `apply_to_attribute_or_variable`: `send(attr).try(method)`),
//! written as ActiveSupport's own body at the call: nil for a nil receiver, else `public_send(name, ...)` where the
//! receiver `respond_to?(name)`, else nil. A literal name is `try_guard`'s, and a name a loop binds `send_dispatch`'s.
//! The Ruby family's trees only: Spinel resolves the `respond_to?` and `public_send` of a computed name over the
//! names the program spells.

use crate::app::App;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};

pub fn apply_dynamic_try(app: &mut App, target: crate::build_target::BuildTarget) {
    if !target.has_runtime_relation() {
        return;
    }
    crate::lower::for_each_emit_body(app, &mut rewrite);
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else { return };
    if method.as_str() != "try" {
        return;
    }
    let Some((name, rest)) = args.split_first() else { return };
    // Not a name read twice that could run code: it is asked once and sent once.
    if !matches!(&*name.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) {
        return;
    }
    let span = e.span;
    let held = Symbol::from(format!("__rh_try_{}", span.start));
    let read = || Expr::new(span, ExprNode::Var { id: VarId(0), name: held.clone() });
    let nil = || Expr::new(span, ExprNode::Lit { value: Literal::Nil });
    let send = |method: &str, args: Vec<Expr>| {
        Expr::new(span, ExprNode::Send { recv: Some(read()), method: Symbol::from(method), args, block: None, parenthesized: true })
    };
    // Not the other arguments evaluated in the send's branch only: Ruby evaluates them, in order, before `try` runs.
    let mut binds = Vec::new();
    let mut sent = vec![name.clone()];
    for (i, arg) in rest.iter().enumerate() {
        // Not a variable either: a later argument may assign it before the send reads it.
        if matches!(&*arg.node, ExprNode::Lit { .. } | ExprNode::SelfRef) {
            sent.push(arg.clone());
            continue;
        }
        let local = Symbol::from(format!("__rh_try_{}_{i}", span.start));
        binds.push(Expr::new(span, ExprNode::Assign { target: LValue::Var { id: VarId(0), name: local.clone() }, value: arg.clone() }));
        sent.push(Expr::new(span, ExprNode::Var { id: VarId(0), name: local }));
    }
    let answer = Expr::new(span, ExprNode::If {
        cond: send("respond_to?", vec![name.clone()]),
        then_branch: send("public_send", sent),
        else_branch: nil(),
    });
    let guarded = Expr::new(span, ExprNode::If { cond: send("nil?", Vec::new()), then_branch: nil(), else_branch: answer });
    let hold = Expr::new(span, ExprNode::Assign { target: LValue::Var { id: VarId(0), name: held.clone() }, value: recv.clone() });
    let mut exprs = vec![hold];
    exprs.extend(binds);
    exprs.push(guarded);
    *e = Expr::new(span, ExprNode::Seq { exprs });
}

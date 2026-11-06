//! `redirect_back(fallback_location: x, ...)` → `redirect_back_or_to(x, ...)`.
//!
//! Rails 7 defines the first as the second (actionpack
//! redirecting.rb: `redirect_back_or_to(fallback_location,
//! allow_other_host:, **args)`), and only the second has a runtime and a
//! place among the controller lowering's response terminals.
//! mission_control-jobs' queue pauses answer this way. A call with a key
//! the runtime does not take (`allow_other_host:`) is left as written, and
//! only a controller's own calls are rewritten.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

const PASSED: &[&str] = &["notice", "alert", "status"];

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &mut *expr.node else { return };
    if method.as_str() != "redirect_back" || args.len() != 1 {
        return;
    }
    let ExprNode::Hash { entries, kwargs: true } = &*args[0].node else { return };
    let key = |k: &Expr| match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    };
    if !entries.iter().all(|(k, _)| key(k).is_some_and(|k| k == "fallback_location" || PASSED.contains(&k.as_str()))) {
        return;
    }
    let Some(at) = entries.iter().position(|(k, _)| key(k).as_deref() == Some("fallback_location")) else { return };
    // Not past a value with effects: the positional fallback would then be evaluated before it.
    let pure = |v: &Expr| matches!(&*v.node, ExprNode::Lit { .. } | ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Const { .. });
    if !entries[..at].iter().all(|(_, v)| pure(v)) {
        return;
    }
    let fallback = entries[at].1.clone();
    let rest: Vec<(Expr, Expr)> = entries.iter().filter(|(k, _)| key(k).as_deref() != Some("fallback_location")).cloned().collect();
    let span = args[0].span;
    let mut new_args = vec![fallback];
    if !rest.is_empty() {
        new_args.push(Expr::new(span, ExprNode::Hash { entries: rest, kwargs: true }));
    }
    *method = Symbol::from("redirect_back_or_to");
    *args = new_args;
}

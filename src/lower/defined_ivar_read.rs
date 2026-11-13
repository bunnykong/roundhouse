//! `instance_variable_defined?(:@x) && @x` (OmniAuth's `Strategy::ClassMethods#args`) is `@x`: an instance variable
//! no write has made reads nil, which `&&` answers for the undefined one too, so the guard changes nothing Ruby sees.
//! Spinel has no `instance_variable_defined?` on a class, and the strict targets none at all.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};

pub fn apply_defined_ivar_read_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut rewrite);
    let ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, left, right, .. } = &*e.node else { return };
    let ExprNode::Send { recv, method, args, block: None, .. } = &*left.node else { return };
    if method.as_str() != "instance_variable_defined?" || !recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) {
        return;
    }
    let [name] = args.as_slice() else { return };
    let asked = match &*name.node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
        ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
        _ => return,
    };
    let ExprNode::Ivar { name: read } = &*right.node else { return };
    if asked.strip_prefix('@') != Some(read.as_str()) && asked != read.as_str() {
        return;
    }
    *e = right.clone();
}

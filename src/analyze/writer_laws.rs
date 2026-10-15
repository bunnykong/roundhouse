//! Only the expr/stamped helpers required by the unchanged breaker assertions.
use crate::{expr::{Expr, ExprNode, Literal}, span::Span, ty::Ty};
pub(super) fn expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

pub(super) fn stamped(ty: Ty) -> Expr {
    let mut e = expr(ExprNode::Lit {
        value: Literal::Nil,
    });
    e.ty = Some(ty);
    e
}

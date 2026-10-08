//! Opt-in syntax-shaped returns and immutable empty-literal inputs.
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;
use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;
use crate::App;

static ENABLED: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("RH_SHAPE").is_ok_and(|v| v == "1")
});
pub(super) fn on() -> bool { *ENABLED }

/// A syntax-shaped tuple is still an Array for Ruby concatenation.
/// Keep the classifier used by diagnostics and emitters in agreement with
/// dispatch, which already reads tuple receivers through their Array view.
pub(crate) fn collection_elem(ty: &Ty) -> Option<Ty> {
    if on() {
        if let Ty::Tuple { elems } = ty {
            return Some(elems.iter().cloned().reduce(super::body::union_of).unwrap_or(Ty::Bottom));
        }
    }
    ty.collection_elem()
}

thread_local! {
    static LITERALS: RefCell<HashMap<super::fold::SiteId, Option<Ty>>> = RefCell::new(HashMap::new());
}

pub(super) fn capture_literals(app: &App) {
    if !on() { return; }
    fn walk(e: &Expr, m: &mut HashMap<super::fold::SiteId, Option<Ty>>) {
        if matches!(&*e.node, ExprNode::Hash { entries, .. } if entries.is_empty())
            || matches!(&*e.node, ExprNode::Array { elements, .. } if elements.is_empty()) {
            m.entry(super::fold::site_of(&e.span)).or_insert_with(|| e.ty.clone());
        }
        e.node.for_each_child(&mut |c| walk(c, m));
    }
    LITERALS.with(|m| {
        let mut m = m.borrow_mut();
        m.clear();
        crate::lower::for_each_emit_body_ref(app, &mut |e| walk(e, &mut m));
    });
}

pub(super) fn literal_seed(e: &Expr) -> Option<Ty> {
    // Missing means no authored seed. Never fall back to an inferred stamp.
    LITERALS.with(|m| m.borrow().get(&super::fold::site_of(&e.span)).cloned().flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shape_literal_stamp_is_not_an_input() {
        let mut e = Expr::new(crate::span::Span::synthetic(), ExprNode::Array {
            elements: Vec::new(), style: Default::default(),
        });
        LITERALS.with(|m| { m.borrow_mut().insert(super::super::fold::site_of(&e.span), None); });
        e.ty = Some(Ty::Array { elem: std::sync::Arc::new(Ty::Int) });
        assert_eq!(literal_seed(&e), None);
        LITERALS.with(|m| { m.borrow_mut().clear(); });
        assert_eq!(literal_seed(&e), None);
    }
}

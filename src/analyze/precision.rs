//! Opt-in expression census after reference expansion, over the emit-body inventory.
//! Categories partition expressions with a type. `Bottom` counts as fully typed;
//! these counts measure opacity, not soundness or agreement with runtime values.

use std::sync::LazyLock;

use crate::{App, expr::Expr, ty::Ty};

static ENABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var("RH_PRECISION_CENSUS").as_deref() == Ok("1"));

pub(super) fn on() -> bool {
    *ENABLED
}

fn bits(t: &Ty) -> u8 {
    let mut b = if t.is_unknown() { 2 } else { 0 };
    if t.is_unknown() && !matches!(t, Ty::Var { .. }) {
        b |= 1;
    }
    if matches!(t, Ty::Bottom) {
        b |= 4;
    }
    let mut child = |t: &Ty| b |= bits(t);
    match t {
        Ty::Array { elem } => child(elem),
        Ty::Hash { key, value } => {
            child(key);
            child(value);
        }
        Ty::Tuple { elems } => elems.iter().for_each(child),
        Ty::Union { variants } => variants.iter().for_each(child),
        Ty::Record { row } => row.fields.values().for_each(child),
        Ty::Class { args, .. } => args.iter().for_each(child),
        Ty::Fn { params, block, ret, .. } => {
            params.iter().for_each(|p| child(&p.ty));
            if let Some(t) = block {
                child(t);
            }
            child(ret);
        }
        _ => {}
    }
    b
}

#[derive(Default, serde::Serialize)]
pub(super) struct Census {
    typed: u64,
    missing: u64,
    untyped_anywhere: u64,
    bare_untyped: u64,
    var_without_untyped: u64,
    fully_typed: u64,
    bottom_anywhere: u64,
}

impl Census {
    fn expr(&mut self, e: &Expr) {
        if let Some(t) = &e.ty {
            self.typed += 1;
            let b = bits(t);
            self.untyped_anywhere += u64::from(b & 1 != 0);
            self.var_without_untyped += u64::from(b & 1 == 0 && b & 2 != 0);
            self.fully_typed += u64::from(b & 3 == 0);
            self.bottom_anywhere += u64::from(b & 4 != 0);
            self.bare_untyped += u64::from(t.is_unknown() && !matches!(t, Ty::Var { .. }));
        } else {
            self.missing += 1;
        }
        e.node.for_each_child(&mut |e| self.expr(e));
    }
}

pub(super) fn census(app: &App) -> Census {
    let mut c = Census::default();
    crate::lower::for_each_emit_body_ref(app, &mut |e| c.expr(e));
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expr::{ExprNode, Literal}, ident::TyVar, span::Span};
    use std::sync::Arc;

    #[test]
    fn categories_partition_typed_expressions_without_counting_missing() {
        let mut c = Census::default();
        for ty in [
            Some(Ty::Int),
            Some(Ty::Bottom),
            Some(Ty::Var { var: TyVar(1) }),
            Some(Ty::Array { elem: Arc::new(Ty::Var { var: TyVar(2) }) }),
            Some(Ty::gradual()),
            Some(Ty::Array { elem: Arc::new(Ty::Union {
                variants: vec![Ty::pending_untyped(), Ty::Var { var: TyVar(3) }].into(),
            }) }),
            None,
        ] {
            let mut e = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
            e.ty = ty;
            c.expr(&e);
        }
        assert_eq!((c.typed, c.missing), (6, 1));
        assert_eq!((c.fully_typed, c.untyped_anywhere, c.var_without_untyped), (2, 2, 2));
        assert_eq!((c.bare_untyped, c.bottom_anywhere), (1, 1));
        assert_eq!(c.fully_typed + c.untyped_anywhere + c.var_without_untyped, c.typed);
    }

    #[test]
    fn nested_untyped_provenances_and_variables_are_detected() {
        for t in [Ty::pending_untyped(), Ty::gradual(), Ty::unresolved()] {
            let nested = Ty::Hash {
                key: Arc::new(Ty::Str),
                value: Arc::new(Ty::Tuple {
                    elems: vec![Ty::Var { var: TyVar(9) }, t, Ty::Bottom].into(),
                }),
            };
            assert_eq!(bits(&nested), 7);
        }
    }
}

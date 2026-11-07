use crate::expr::Expr;
use crate::ident::Symbol;
use crate::ty::Ty;

pub(crate) fn deep_stringify_keys(recv: &Ty, method: &Symbol, args: &[Expr]) -> Option<Ty> {
    if method.as_str() != "deep_stringify_keys" || !args.is_empty() {
        return None;
    }
    let hash_receiver = match recv {
        Ty::Hash { .. } => true,
        Ty::Union { variants } => {
            variants.iter().any(|ty| matches!(ty, Ty::Hash { .. }))
                && variants.iter().all(|ty| matches!(ty, Ty::Hash { .. } | Ty::Nil))
        }
        _ => false,
    };
    if !hash_receiver {
        return None;
    }
    Some(Ty::Hash {
        key: std::sync::Arc::new(Ty::Str),
        value: std::sync::Arc::new(Ty::Untyped),
    })
}

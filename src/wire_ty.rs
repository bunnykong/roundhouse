//! Storage equality: structural equality including untyped provenance
//! and record insertion order. Interning and the join memo use the same
//! memoized walk; semantic equality remains provenance-insensitive.
use crate::ident::Symbol;
use crate::ty::{Param, Ty};
use indexmap::IndexMap;
use std::collections::HashMap;
#[derive(Default)]
struct Memo {
    pairs: HashMap<(usize, usize), bool>,
    provenance: bool,
}

pub(crate) fn storage_types(a: &[Ty], b: &[Ty]) -> bool {
    slice(a, b, &mut Memo { provenance: true, ..Memo::default() })
}
pub(crate) fn storage_params(a: &[Param], b: &[Param]) -> bool {
    let mut memo = Memo { provenance: true, ..Memo::default() };
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.name == y.name && x.kind == y.kind && go(&x.ty, &y.ty, &mut memo))
}
pub(crate) fn storage_fields(a: &IndexMap<Symbol, Ty>, b: &IndexMap<Symbol, Ty>) -> bool {
    fields_memo(a, b, &mut Memo { provenance: true, ..Memo::default() })
}
fn fields_memo(a: &IndexMap<Symbol, Ty>, b: &IndexMap<Symbol, Ty>, memo: &mut Memo) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|((ka, ta), (kb, tb))| ka == kb && go(ta, tb, memo))
}
fn slice(a: &[Ty], b: &[Ty], memo: &mut Memo) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| go(x, y, memo))
}
fn go(a: &Ty, b: &Ty, memo: &mut Memo) -> bool {
    if std::ptr::eq(a, b) {
        return true;
    }
    let key = (a as *const Ty as usize, b as *const Ty as usize);
    if let Some(value) = memo.pairs.get(&key) {
        return *value;
    }
    if a != b {
        memo.pairs.insert(key, false);
        return false;
    }
    let result = match (a, b) {
        (Ty::Untyped { why: x }, Ty::Untyped { why: y }) => !memo.provenance || x == y,
        (Ty::Array { elem: x }, Ty::Array { elem: y }) => go(x, y, memo),
        (Ty::Hash { key: kx, value: vx }, Ty::Hash { key: ky, value: vy }) => {
            go(kx, ky, memo) && go(vx, vy, memo)
        }
        (Ty::Tuple { elems: x }, Ty::Tuple { elems: y }) => x.ptr_eq(y) || slice(x, y, memo),
        (Ty::Union { variants: x }, Ty::Union { variants: y }) => x.ptr_eq(y) || slice(x, y, memo),
        (Ty::Class { args: x, .. }, Ty::Class { args: y, .. }) => x.ptr_eq(y) || slice(x, y, memo),
        (Ty::Record { row: x }, Ty::Record { row: y }) => {
            x.fields.ptr_eq(&y.fields) || fields_memo(&x.fields, &y.fields, memo)
        }
        (
            Ty::Fn {
                params: px,
                block: bx,
                ret: rx,
                ..
            },
            Ty::Fn {
                params: py,
                block: by,
                ret: ry,
                ..
            },
        ) => {
            (px.ptr_eq(py) || px.iter().zip(py).all(|(x, y)| go(&x.ty, &y.ty, memo)))
                && match (bx, by) {
                    (Some(x), Some(y)) => go(x, y, memo),
                    _ => true,
                }
                && go(rx, ry, memo)
        }
        _ => true,
    };
    memo.pairs.insert(key, result);
    result
}

#[cfg(test)]
pub(crate) fn equal(a: &Ty, b: &Ty) -> bool {
    go(a, b, &mut Memo::default())
}

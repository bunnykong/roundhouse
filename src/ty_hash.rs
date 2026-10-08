//! Structural hashing that visits each shared node once.
//!
//! [`Ty`]'s `Hash` goes through [`hash`]: a node's fingerprint is memoized
//! on its address for the duration of the outermost hash, so hashing a DAG
//! costs its distinct nodes rather than its paths.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::ty::Ty;

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static MEMO: RefCell<HashMap<usize, u64>> = RefCell::new(HashMap::new());
}

struct Scope(bool);

impl Drop for Scope {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get() - 1));
        if self.0 {
            MEMO.with(|m| m.borrow_mut().clear());
        }
    }
}

/// Runs `f` inside one memo scope: nested hashes share the memo, and the
/// outermost scope clears it on return.
pub(crate) fn scope<T>(f: impl FnOnce() -> T) -> T {
    let root = DEPTH.with(|d| {
        let root = d.get() == 0;
        d.set(d.get() + 1);
        root
    });
    let _scope = Scope(root);
    f()
}

pub(crate) fn hash<H: Hasher>(ty: &Ty, state: &mut H) {
    let fp = scope(|| fingerprint(ty));
    fp.hash(state);
}

fn fingerprint(ty: &Ty) -> u64 {
    let key = ty as *const Ty as usize;
    if let Some(fp) = MEMO.with(|m| m.borrow().get(&key).copied()) {
        return fp;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let tag: u8 = match ty {
        Ty::Int => 0,
        Ty::Float => 1,
        Ty::Bool => 2,
        Ty::Str => 3,
        Ty::Sym => 4,
        Ty::Date => 5,
        Ty::Time => 6,
        Ty::Nil => 7,
        Ty::Relation { .. } => 8,
        Ty::Array { .. } => 9,
        Ty::Hash { .. } => 10,
        Ty::Tuple { .. } => 11,
        Ty::Record { .. } => 12,
        Ty::Union { .. } => 13,
        Ty::SelfInstance => 14,
        Ty::Class { .. } => 15,
        Ty::Fn { .. } => 16,
        Ty::Var { .. } => 17,
        Ty::Untyped => 18,
        Ty::Bottom => 19,
    };
    tag.hash(&mut h);
    match ty {
        Ty::Relation { of } => of.hash(&mut h),
        Ty::Array { elem } => elem.hash(&mut h),
        Ty::Hash { key, value } => {
            key.hash(&mut h);
            value.hash(&mut h);
        }
        Ty::Tuple { elems } => elems.hash(&mut h),
        Ty::Record { row } => row.hash(&mut h),
        Ty::Union { variants } => variants.hash(&mut h),
        Ty::Class { id, args } => {
            id.hash(&mut h);
            args.hash(&mut h);
        }
        Ty::Fn {
            params,
            block,
            ret,
            effects,
        } => {
            params.hash(&mut h);
            block.hash(&mut h);
            ret.hash(&mut h);
            effects.hash(&mut h);
        }
        Ty::Var { var } => var.hash(&mut h),
        _ => {}
    }
    let fp = h.finish();
    MEMO.with(|m| {
        m.borrow_mut().insert(key, fp);
    });
    fp
}

//! Memoized joins within one outermost `union_of`.
//!
//! A join of two DAG-shaped types meets the same pair of shared subtrees
//! along many paths. Keys own their inputs, and compare by wire equality
//! (`wire_ty`), so a cached result is reused only where it serializes
//! exactly as a fresh join would: record field order is part of the key.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::ty::Ty;

#[derive(Clone)]
struct Wire(Ty);

impl PartialEq for Wire {
    fn eq(&self, other: &Self) -> bool {
        crate::wire_ty::equal(&self.0, &other.0)
    }
}

impl Eq for Wire {}

impl Hash for Wire {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.0.hash(h);
    }
}

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static MEMO: RefCell<HashMap<(Wire, Wire), Ty>> = RefCell::new(HashMap::new());
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

/// `compute(a, b)`, or the result of an equal pair joined earlier inside
/// the same outermost join.
pub(crate) fn join(a: Ty, b: Ty, compute: impl FnOnce(Ty, Ty) -> Ty) -> Ty {
    let root = DEPTH.with(|d| {
        let root = d.get() == 0;
        d.set(d.get() + 1);
        root
    });
    let _scope = Scope(root);
    let key = (Wire(a.clone()), Wire(b.clone()));
    if let Some(value) = MEMO.with(|m| m.borrow().get(&key).cloned()) {
        return value;
    }
    let value = compute(a, b);
    MEMO.with(|m| {
        m.borrow_mut().insert(key, value.clone());
    });
    value
}

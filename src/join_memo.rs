//! Memoized joins within one outermost `union_of`.
//!
//! A join of two DAG-shaped types meets the same pair of shared subtrees
//! along many paths. Keys own their inputs and compare by storage equality
//! (`wire_ty`): untyped provenance at every depth and record field order
//! are both part of the key, so a cached result preserves the fresh join.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::ty::Ty;

#[derive(Clone)]
struct Wire(Ty);

impl PartialEq for Wire {
    fn eq(&self, other: &Self) -> bool {
        crate::wire_ty::storage_types(
            std::slice::from_ref(&self.0),
            std::slice::from_ref(&other.0),
        )
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

#[cfg(test)]
mod tests {
    use crate::analyze::union_of;
    use crate::ident::Symbol;
    use crate::ty::{Provenance, Row, Ty};
    use std::sync::Arc;

    fn tuple(why: Provenance) -> Ty {
        Ty::Tuple {
            elems: vec![Ty::Untyped { why }].into(),
        }
    }

    fn hash(key: Ty, value: Ty) -> Ty {
        Ty::Hash {
            key: Arc::new(key),
            value: Arc::new(value),
        }
    }

    fn tuple_leaf(t: &Ty) -> Provenance {
        let Ty::Union { variants } = t else {
            panic!("expected union: {t:?}")
        };
        variants
            .iter()
            .find_map(|v| match v {
                Ty::Tuple { elems } => elems[0].provenance(),
                _ => None,
            })
            .expect("tuple leaf")
    }

    fn check_provenance(first: Provenance, second: Provenance) {
        // The key and value joins visit wire-equal pairs in one outermost join.
        let joined = union_of(hash(tuple(first), tuple(second)), hash(Ty::Nil, Ty::Nil));
        let Ty::Hash { key, value } = joined else {
            panic!("expected hash")
        };
        assert_eq!(tuple_leaf(&key), first);
        assert_eq!(tuple_leaf(&value), second);
    }

    #[test]
    fn pending_pair_does_not_supply_gradual_pair_result() {
        check_provenance(Provenance::Pending, Provenance::Gradual);
    }

    #[test]
    fn gradual_pair_does_not_supply_pending_pair_result() {
        check_provenance(Provenance::Gradual, Provenance::Pending);
    }

    fn record(names: [&str; 2]) -> Ty {
        Ty::Record {
            row: Row {
                fields: names
                    .into_iter()
                    .map(|name| (Symbol::from(name), Ty::Int))
                    .collect(),
                rest: None,
            },
        }
    }

    fn record_json(t: &Ty) -> String {
        let Ty::Union { variants } = t else {
            panic!("expected union: {t:?}")
        };
        let record = variants
            .iter()
            .find(|v| matches!(v, Ty::Record { .. }))
            .expect("record");
        serde_json::to_string(record).unwrap()
    }

    #[test]
    fn record_field_order_remains_part_of_the_key() {
        let first = record(["alpha", "beta"]);
        let second = record(["beta", "alpha"]);
        assert_eq!(first, second);
        let first_json = serde_json::to_string(&first).unwrap();
        let second_json = serde_json::to_string(&second).unwrap();
        assert_ne!(first_json, second_json);
        let joined = union_of(hash(first, second), hash(Ty::Nil, Ty::Nil));
        let Ty::Hash { key, value } = joined else {
            panic!("expected hash")
        };
        assert_eq!(record_json(&key), first_json);
        assert_eq!(record_json(&value), second_json);
    }
}

//! Equality and ordering of types that visit each pair of shared nodes once.
//!
//! With [`Arc`](std::sync::Arc) children, a type is a DAG: the same node
//! can be reached along many paths, and a tree walk visits it once per
//! path. Each operation memoizes on the node addresses it compares, for the
//! duration of the outermost call, so the cost follows the number of
//! distinct nodes. No global interner, widening or normalization.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::ty::Ty;

/// A multiply-rotate hasher for the pointer-pair memo keys: SipHash
/// dominated the memoized comparisons.
#[derive(Default, Clone, Copy)]
struct PairHasher(u64);

impl std::hash::Hasher for PairHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(*b as u64);
        }
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_usize(&mut self, x: usize) {
        self.write_u64(x as u64);
    }
}

type PairHash = std::hash::BuildHasherDefault<PairHasher>;

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static MEMO: RefCell<HashMap<(usize, usize), bool, PairHash>> = RefCell::new(HashMap::default());
    static CMP_DEPTH: Cell<usize> = const { Cell::new(0) };
    static CMP_MEMO: RefCell<HashMap<(usize, usize), std::cmp::Ordering>> =
        RefCell::new(HashMap::new());
}

/// Leaves the memo of one operation when its outermost call returns.
struct Scope {
    root: bool,
    depth: &'static std::thread::LocalKey<Cell<usize>>,
    clear: fn(),
}

impl Scope {
    fn enter(depth: &'static std::thread::LocalKey<Cell<usize>>, clear: fn()) -> Self {
        let root = depth.with(|d| {
            let root = d.get() == 0;
            d.set(d.get() + 1);
            root
        });
        Scope { root, depth, clear }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.depth.with(|d| d.set(d.get() - 1));
        if self.root {
            (self.clear)();
        }
    }
}

/// Structural equality of `a` and `b`.
pub(crate) fn equal(a: &Ty, b: &Ty) -> bool {
    let _scope = Scope::enter(&DEPTH, || MEMO.with(|m| m.borrow_mut().clear()));
    if std::ptr::eq(a, b) {
        return true;
    }
    // Leaves and differing heads need no memo entry.
    if std::mem::discriminant(a) != std::mem::discriminant(b) {
        return false;
    }
    match (a, b) {
        (Ty::Int, _)
        | (Ty::Float, _)
        | (Ty::Bool, _)
        | (Ty::Str, _)
        | (Ty::Sym, _)
        | (Ty::Date, _)
        | (Ty::Time, _)
        | (Ty::Nil, _)
        | (Ty::SelfInstance, _)
        | (Ty::Untyped { .. }, _)
        | (Ty::Bottom, _) => return true,
        (Ty::Var { var: x }, Ty::Var { var: y }) => return x == y,
        (Ty::Rec { slot: x }, Ty::Rec { slot: y }) => return x == y,
        (Ty::Relation { of: x }, Ty::Relation { of: y }) => return x == y,
        _ => {}
    }
    let key = (a as *const Ty as usize, b as *const Ty as usize);
    if let Some(value) = MEMO.with(|m| m.borrow().get(&key).copied()) {
        return value;
    }
    let result = match (a, b) {
        (Ty::Int, Ty::Int)
        | (Ty::Float, Ty::Float)
        | (Ty::Bool, Ty::Bool)
        | (Ty::Str, Ty::Str)
        | (Ty::Sym, Ty::Sym)
        | (Ty::Date, Ty::Date)
        | (Ty::Time, Ty::Time)
        | (Ty::Nil, Ty::Nil)
        | (Ty::SelfInstance, Ty::SelfInstance)
        | (Ty::Untyped { .. }, Ty::Untyped { .. })
        | (Ty::Bottom, Ty::Bottom) => true,
        (Ty::Relation { of: x }, Ty::Relation { of: y }) => x == y,
        (Ty::Array { elem: x }, Ty::Array { elem: y }) => x == y,
        (Ty::Hash { key: kx, value: vx }, Ty::Hash { key: ky, value: vy }) => kx == ky && vx == vy,
        (Ty::Tuple { elems: x }, Ty::Tuple { elems: y }) => x == y,
        (Ty::Record { row: x }, Ty::Record { row: y }) => x == y,
        (Ty::Union { variants: x }, Ty::Union { variants: y }) => x == y,
        (Ty::Class { id: ix, args: ax }, Ty::Class { id: iy, args: ay }) => ix == iy && ax == ay,
        (
            Ty::Fn {
                params: px,
                block: bx,
                ret: rx,
                effects: ex,
            },
            Ty::Fn {
                params: py,
                block: by,
                ret: ry,
                effects: ey,
            },
        ) => px == py && bx == by && rx == ry && ex == ey,
        (Ty::Var { var: x }, Ty::Var { var: y }) => x == y,
        (Ty::Rec { slot: x }, Ty::Rec { slot: y }) => x == y,
        _ => false,
    };
    MEMO.with(|m| {
        m.borrow_mut().insert(key, result);
    });
    result
}

/// The total order of `ty.rs`'s `cmp_ty`, with `compute` the unmemoized
/// comparison of `a` and `b`.
pub(crate) fn compare(
    a: &Ty,
    b: &Ty,
    compute: impl FnOnce() -> std::cmp::Ordering,
) -> std::cmp::Ordering {
    let _scope = Scope::enter(&CMP_DEPTH, || CMP_MEMO.with(|m| m.borrow_mut().clear()));
    if std::ptr::eq(a, b) {
        return std::cmp::Ordering::Equal;
    }
    let key = (a as *const Ty as usize, b as *const Ty as usize);
    if let Some(result) = CMP_MEMO.with(|m| m.borrow().get(&key).copied()) {
        return result;
    }
    let result = compute();
    CMP_MEMO.with(|m| {
        m.borrow_mut().insert(key, result);
    });
    result
}

//! Shared type payloads: copy-on-write vectors and maps for `Ty`'s
//! children, with a cached structural digest and bounded weak hash-consing.
//!
//! A `Shared<T>` clones in O(1), and a mutation copies the payload only
//! when another owner holds it. Building one interns it: an existing live
//! payload that is wire-equal (`wire_ty`, so record field order counts) is
//! reused, so equal subtrees built in different places share one node.
//! Equality checks the digest first and caches results by payload
//! identity; a payload gets a fresh identity whenever it is mutated.
use crate::ident::Symbol;
use crate::ty::{Param, Ty};
use serde::{Deserialize, Serialize};
use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, DerefMut};
use std::sync::{
    Arc, OnceLock, Weak,
    atomic::{AtomicU64, Ordering},
};

mod sealed {
    pub trait Sealed {}
    impl Sealed for Vec<super::Ty> {}
    impl Sealed for Vec<super::Param> {}
    impl Sealed for indexmap::IndexMap<super::Symbol, super::Ty> {}
}
/// Only the known type payloads have equality stable under shared borrowing.
/// Wire equality also preserves record insertion order when choosing a representative.
pub trait PayloadValue: sealed::Sealed + Clone + Eq + Send + Sync + 'static {
    fn structural_hash<H: Hasher>(&self, state: &mut H);
    fn wire_equal(&self, other: &Self) -> bool {
        self == other
    }
}
impl PayloadValue for Vec<Ty> {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.hash(state);
    }
    fn wire_equal(&self, other: &Self) -> bool {
        crate::wire_ty::types(self, other)
    }
}
impl PayloadValue for Vec<Param> {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.hash(state);
    }
    fn wire_equal(&self, other: &Self) -> bool {
        crate::wire_ty::params(self, other)
    }
}
impl PayloadValue for indexmap::IndexMap<Symbol, Ty> {
    fn structural_hash<H: Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        let mut fields: Vec<_> = self.iter().collect();
        fields.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (name, ty) in fields {
            name.hash(state);
            ty.hash(state);
        }
    }
    fn wire_equal(&self, other: &Self) -> bool {
        crate::wire_ty::fields(self, other)
    }
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
fn next_id() -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    assert!(id != u64::MAX, "shared payload identity exhausted");
    id
}
const CACHE_LIMIT: usize = 262_144;
type Erased = dyn Any + Send + Sync;
thread_local! {
    static EQUAL: RefCell<HashMap<(u64,u64),bool>> = RefCell::new(HashMap::new());
    static INTERN: RefCell<HashMap<(TypeId,u64),Vec<Weak<Erased>>>> = RefCell::new(HashMap::new());
    static INTERN_SLOTS: Cell<usize> = const { Cell::new(0) };
}
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct Payload<T> {
    value: T,
    #[serde(skip, default = "next_id")]
    id: u64,
    #[serde(skip)]
    digest: OnceLock<u64>,
}
impl<T: Clone> Clone for Payload<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            id: next_id(),
            digest: self.digest.clone(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Shared<T>(Arc<Payload<T>>);
impl<T: std::fmt::Debug> std::fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Shared").field(&self.0.value).finish()
    }
}
fn digest<T: PayloadValue>(value: &T) -> u64 {
    let mut state = std::collections::hash_map::DefaultHasher::new();
    crate::ty_hash::scope(|| value.structural_hash(&mut state));
    state.finish()
}
impl<T: PayloadValue> Shared<T> {
    fn digest(&self) -> u64 {
        *self.0.digest.get_or_init(|| digest(&self.0.value))
    }
}
impl<T: PayloadValue> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.0, &other.0) {
            return true;
        }
        if self.digest() != other.digest() {
            return false;
        }
        let key = if self.0.id < other.0.id {
            (self.0.id, other.0.id)
        } else {
            (other.0.id, self.0.id)
        };
        if let Some(result) = EQUAL.with(|m| m.borrow().get(&key).copied()) {
            return result;
        }
        let result = self.0.value == other.0.value;
        EQUAL.with(|m| {
            let mut m = m.borrow_mut();
            if m.len() >= CACHE_LIMIT {
                m.clear();
            }
            m.insert(key, result);
        });
        result
    }
}
impl<T: PayloadValue> Eq for Shared<T> {}
impl<T: PayloadValue> Hash for Shared<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.digest().hash(state);
    }
}
impl<T: PayloadValue> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        let fp = digest(&value);
        let key = (TypeId::of::<T>(), fp);
        // Do not retain a RefCell borrow during recursive value comparisons.
        let candidates = INTERN.with(|m| m.borrow().get(&key).cloned().unwrap_or_default());
        for weak in candidates {
            if let Some(any) = weak.upgrade()
                && let Ok(node) = Arc::downcast::<Payload<T>>(any)
                && node.value.wire_equal(&value)
            {
                return Self(node);
            }
        }
        let node = Arc::new(Payload {
            value,
            id: next_id(),
            digest: OnceLock::from(fp),
        });
        let erased: Arc<Erased> = node.clone();
        let weak = Arc::downgrade(&erased);
        INTERN.with(|m| {
            let mut m = m.borrow_mut();
            if INTERN_SLOTS.with(|n| n.get() >= CACHE_LIMIT) {
                m.clear();
                INTERN_SLOTS.with(|n| n.set(0));
            }
            let bucket = m.entry(key).or_default();
            let before = bucket.len();
            bucket.retain(|w| w.strong_count() > 0);
            INTERN_SLOTS.with(|n| n.set(n.get() - (before - bucket.len()) + 1));
            bucket.push(weak);
        });
        Self(node)
    }
}
impl<T: Default + PayloadValue> Default for Shared<T> {
    fn default() -> Self {
        T::default().into()
    }
}
impl<T> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0.value
    }
}
impl<T: Clone> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        let node = Arc::make_mut(&mut self.0);
        node.id = next_id();
        node.digest = OnceLock::new();
        &mut node.value
    }
}
impl<A, T: FromIterator<A> + PayloadValue> FromIterator<A> for Shared<T> {
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        T::from_iter(iter).into()
    }
}
impl<T: Clone + IntoIterator> IntoIterator for Shared<T> {
    type Item = T::Item;
    type IntoIter = T::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        Arc::unwrap_or_clone(self.0).value.into_iter()
    }
}
impl<'a, T> IntoIterator for &'a Shared<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&self.0.value).into_iter()
    }
}
impl<T> Shared<T> {
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl<T: Clone> Shared<T> {
    pub fn into_owned(self) -> T {
        Arc::unwrap_or_clone(self.0).value
    }
}
impl<T: Clone> From<Shared<Vec<T>>> for Vec<T> {
    fn from(value: Shared<Vec<T>>) -> Self {
        value.into_owned()
    }
}

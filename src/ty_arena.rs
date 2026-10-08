//! An exact type DAG at a `Ty` boundary. Ids never escape their arena.
//!
//! Storage identity retains record order and untyped provenance. A second
//! identity follows `Ty::eq`, which ignores those two distinctions. This
//! lets a caller compare by id without choosing a different wire value.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::num::NonZeroU32;
use std::sync::{Arc, LazyLock};

use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol, TyVar};
use crate::ty::{ParamKind, Provenance, Ty};

static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_ARENA").is_ok_and(|v| v == "1"));

#[inline]
pub(crate) fn on() -> bool {
    *ON
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TyId(NonZeroU32);

impl TyId {
    fn index(self) -> usize {
        self.0.get() as usize - 1
    }
}

// Used only for internally allocated ids and addresses, never source strings.
#[derive(Default)]
struct IdHasher(u64);
impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u64(*byte as u64);
        }
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_u32(&mut self, value: u32) {
        self.write_u64(value as u64);
    }
    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}
type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;

#[derive(Clone, Eq, Hash, PartialEq)]
enum Key {
    Leaf(u8),
    Untyped(Provenance),
    Relation(ClassId),
    Array(u32),
    Hash(u32, u32),
    Tuple(Vec<u32>),
    Union(Vec<u32>),
    Record(Vec<(Symbol, u32)>, Option<TyVar>),
    Class(ClassId, Vec<u32>),
    Fn(Vec<(Symbol, ParamKind, u32)>, Option<u32>, u32, EffectSet),
    Var(TyVar),
    Rec(u32),
}

struct Node {
    value: Ty,
    semantic: u32,
}

pub(crate) struct Arena {
    nodes: Vec<Node>,
    wire: HashMap<Key, TyId>,
    semantic: HashMap<Key, u32>,
    // Retaining each Arc prevents address reuse and forces copy-on-write if
    // its owner mutates. A raw-address cache without this owner is unsound.
    arcs: IdMap<usize, (Arc<Ty>, TyId)>,
    // Shared payload ids are globally unique and refreshed on every mutation.
    shared: IdMap<(u8, u64, Option<u32>), TyId>,
    pub(crate) requests: u64,
    pub(crate) shared_hits: u64,
    pub(crate) arc_hits: u64,
}

const LEAVES: [Ty; 13] = [
    Ty::Int,
    Ty::Float,
    Ty::Bool,
    Ty::Str,
    Ty::Sym,
    Ty::Date,
    Ty::Time,
    Ty::Nil,
    Ty::SelfInstance,
    Ty::Bottom,
    Ty::pending_untyped(),
    Ty::gradual(),
    Ty::unresolved(),
];

impl Default for Arena {
    fn default() -> Self {
        let mut arena = Self {
            nodes: Vec::new(),
            wire: HashMap::new(),
            semantic: HashMap::new(),
            arcs: IdMap::default(),
            shared: IdMap::default(),
            requests: 0,
            shared_hits: 0,
            arc_hits: 0,
        };
        for (index, ty) in LEAVES.into_iter().enumerate() {
            let key = match ty {
                Ty::Untyped { why } => Key::Untyped(why),
                _ => Key::Leaf(index as u8),
            };
            arena.insert(ty, key);
        }
        arena
    }
}

impl Arena {
    pub(crate) fn get(&self, id: TyId) -> &Ty {
        &self.nodes[id.index()].value
    }
    pub(crate) fn same(&self, a: TyId, b: TyId) -> bool {
        a == b || self.nodes[a.index()].semantic == self.nodes[b.index()].semantic
    }
    pub(crate) fn semantic(&mut self, value: &Ty) -> u32 {
        let id = self.intern(value);
        self.nodes[id.index()].semantic
    }
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }
    pub(crate) fn cache_size(&self) -> usize {
        self.nodes.len() + self.arcs.len() + self.shared.len()
    }

    fn semantic_id(&self, wire: u32) -> u32 {
        self.nodes[wire as usize - 1].semantic
    }

    fn semantic_key(&self, mut key: Key) -> Key {
        match &mut key {
            Key::Untyped(why) => *why = Provenance::Gradual,
            Key::Array(id) => *id = self.semantic_id(*id),
            Key::Hash(k, v) => {
                *k = self.semantic_id(*k);
                *v = self.semantic_id(*v);
            }
            Key::Tuple(ids) | Key::Union(ids) | Key::Class(_, ids) => {
                ids.iter_mut().for_each(|id| *id = self.semantic_id(*id));
            }
            Key::Record(fields, _) => {
                for (_, id) in fields.iter_mut() {
                    *id = self.semantic_id(*id);
                }
                fields.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
            }
            Key::Fn(params, block, ret, _) => {
                for (_, _, id) in params {
                    *id = self.semantic_id(*id);
                }
                if let Some(id) = block {
                    *id = self.semantic_id(*id);
                }
                *ret = self.semantic_id(*ret);
            }
            _ => {}
        }
        key
    }

    fn insert(&mut self, value: Ty, key: Key) -> TyId {
        if let Some(id) = self.wire.get(&key) {
            return *id;
        }
        let sem_key = self.semantic_key(key.clone());
        let next = u32::try_from(self.semantic.len()).expect("type identity capacity");
        let semantic = *self.semantic.entry(sem_key).or_insert(next);
        let id = TyId(
            NonZeroU32::new(u32::try_from(self.nodes.len() + 1).expect("type arena capacity"))
                .unwrap(),
        );
        self.nodes.push(Node { value, semantic });
        self.wire.insert(key, id);
        id
    }

    fn arc(&mut self, value: &Arc<Ty>) -> u32 {
        let address = Arc::as_ptr(value) as usize;
        if let Some((_, id)) = self.arcs.get(&address) {
            self.arc_hits += 1;
            return id.0.get();
        }
        let id = self.intern(value);
        self.arcs.insert(address, (value.clone(), id));
        id.0.get()
    }

    pub(crate) fn intern(&mut self, value: &Ty) -> TyId {
        self.requests += 1;
        let leaf = match value {
            Ty::Int => Some(0),
            Ty::Float => Some(1),
            Ty::Bool => Some(2),
            Ty::Str => Some(3),
            Ty::Sym => Some(4),
            Ty::Date => Some(5),
            Ty::Time => Some(6),
            Ty::Nil => Some(7),
            Ty::SelfInstance => Some(8),
            Ty::Bottom => Some(9),
            Ty::Untyped {
                why: Provenance::Pending,
            } => Some(10),
            Ty::Untyped {
                why: Provenance::Gradual,
            } => Some(11),
            Ty::Untyped {
                why: Provenance::Unresolved,
            } => Some(12),
            _ => None,
        };
        if let Some(index) = leaf {
            return TyId(NonZeroU32::new(index + 1).unwrap());
        }
        let shared_key = match value {
            Ty::Tuple { elems } => Some((0, elems.identity(), None)),
            Ty::Union { variants } => Some((1, variants.identity(), None)),
            Ty::Record { row } => Some((2, row.fields.identity(), row.rest.as_ref().map(|v| v.0))),
            _ => None,
        };
        if let Some(key) = shared_key {
            if let Some(id) = self.shared.get(&key) {
                self.shared_hits += 1;
                return *id;
            }
        }
        let key = match value {
            Ty::Relation { of } => Key::Relation(of.clone()),
            Ty::Array { elem } => Key::Array(self.arc(elem)),
            Ty::Hash { key, value } => Key::Hash(self.arc(key), self.arc(value)),
            Ty::Tuple { elems } => {
                Key::Tuple(elems.iter().map(|t| self.intern(t).0.get()).collect())
            }
            Ty::Union { variants } => {
                Key::Union(variants.iter().map(|t| self.intern(t).0.get()).collect())
            }
            Ty::Record { row } => Key::Record(
                row.fields
                    .iter()
                    .map(|(name, t)| (name.clone(), self.intern(t).0.get()))
                    .collect(),
                row.rest.clone(),
            ),
            Ty::Class { id, args } => Key::Class(
                id.clone(),
                args.iter().map(|t| self.intern(t).0.get()).collect(),
            ),
            Ty::Fn {
                params,
                block,
                ret,
                effects,
            } => Key::Fn(
                params
                    .iter()
                    .map(|p| (p.name.clone(), p.kind.clone(), self.arc(&p.ty)))
                    .collect(),
                block.as_ref().map(|b| self.arc(b)),
                self.arc(ret),
                effects.clone(),
            ),
            Ty::Var { var } => Key::Var(var.clone()),
            Ty::Rec { slot } => Key::Rec(*slot),
            _ => unreachable!("leaves already interned"),
        };
        let id = self.insert(value.clone(), key);
        if let Some(key) = shared_key {
            self.shared.insert(key, id);
        }
        id
    }
}

// One cache owns every id used in its keys or values. It can be discarded
// between operations, but never in the middle of a recursively memoized one.
const CACHE_LIMIT: usize = 262_144;

#[derive(Default)]
struct Memo {
    arena: Arena,
    binary: IdMap<(TyId, TyId), TyId>,
    hits: u64,
}

thread_local! {
    static MEMO: RefCell<Memo> = RefCell::new(Memo::default());
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct Scope;

impl Scope {
    fn enter() -> Self {
        let root = DEPTH.with(|depth| {
            let root = depth.get() == 0;
            depth.set(depth.get() + 1);
            root
        });
        if root {
            MEMO.with(|memo| {
                let mut memo = memo.borrow_mut();
                if memo.arena.cache_size() + memo.binary.len() >= CACHE_LIMIT {
                    *memo = Memo::default();
                }
            });
        }
        Self
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
}

/// Forget one analysis' memo. No id may survive this boundary.
pub(crate) fn reset() {
    DEPTH.with(|depth| assert_eq!(depth.get(), 0, "reset inside a type operation"));
    MEMO.with(|memo| *memo.borrow_mut() = Memo::default());
}

/// Use identities for one operation. The caller must not return an id.
pub(crate) fn with_arena<R>(operation: impl FnOnce(&mut Arena) -> R) -> R {
    let _scope = Scope::enter();
    MEMO.with(|memo| operation(&mut memo.borrow_mut().arena))
}

/// The union operation with ordered, exact keys. Computing outside the
/// RefCell borrow permits recursive calls to the same operation.
pub(crate) fn memo_union(a: Ty, b: Ty, compute: impl FnOnce(Ty, Ty) -> Ty) -> Ty {
    if matches!(a, Ty::Bottom) {
        return b;
    }
    if matches!(b, Ty::Bottom) {
        return a;
    }
    let _scope = Scope::enter();
    let (key, hit) = MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        let key = (memo.arena.intern(&a), memo.arena.intern(&b));
        if memo.arena.same(key.0, key.1) {
            // Native union keeps the left representative on semantic equality.
            return (key, Some(a.clone()));
        }
        let hit = memo.binary.get(&key).copied().map(|id| {
            memo.hits += 1;
            memo.arena.get(id).clone()
        });
        (key, hit)
    });
    if let Some(value) = hit {
        return value;
    }
    let result = compute(a, b);
    MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        let id = memo.arena.intern(&result);
        memo.binary.insert(key, id);
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::Effect;
    use crate::ty::{Param, Row};

    fn array(t: Ty) -> Ty {
        Ty::Array { elem: Arc::new(t) }
    }
    fn record(reverse: bool, value: Ty) -> Ty {
        let mut fields = vec![(Symbol::from("a"), value), (Symbol::from("b"), Ty::Str)];
        if reverse {
            fields.reverse();
        }
        Ty::Record {
            row: Row {
                fields: fields.into_iter().collect(),
                rest: None,
            },
        }
    }

    #[test]
    fn wire_order_and_semantic_equality_are_separate_at_every_depth() {
        let a = array(record(false, Ty::Int));
        let b = array(record(true, Ty::Int));
        assert_eq!(a, b);
        let bytes_a = serde_json::to_vec(&a).unwrap();
        let bytes_b = serde_json::to_vec(&b).unwrap();
        assert_ne!(bytes_a, bytes_b);
        let mut arena = Arena::default();
        let ia = arena.intern(&a);
        let ib = arena.intern(&b);
        assert_ne!(ia, ib);
        assert!(arena.same(ia, ib));
        assert_eq!(serde_json::to_vec(arena.get(ia)).unwrap(), bytes_a);
        assert_eq!(serde_json::to_vec(arena.get(ib)).unwrap(), bytes_b);
    }

    #[test]
    fn provenance_is_preserved_even_though_equality_ignores_it() {
        let mut arena = Arena::default();
        let values = [Ty::pending_untyped(), Ty::gradual(), Ty::unresolved()];
        let ids: Vec<_> = values.iter().map(|t| arena.intern(t)).collect();
        for (a, ta) in ids.iter().zip(&values) {
            assert_eq!(arena.get(*a).provenance(), ta.provenance());
            for (b, tb) in ids.iter().zip(&values) {
                assert!(arena.same(*a, *b));
                assert_eq!(a == b, ta.provenance() == tb.provenance());
            }
        }
        let pending = arena.intern(&array(Ty::pending_untyped()));
        let gradual = arena.intern(&array(Ty::gradual()));
        assert_ne!(pending, gradual);
        assert!(arena.same(pending, gradual));
    }

    #[test]
    fn mutating_arc_or_shared_payload_never_reuses_an_old_identity() {
        let mut arena = Arena::default();
        let mut arc = Arc::new(Ty::Int);
        let first = arena.intern(&Ty::Array { elem: arc.clone() });
        *Arc::make_mut(&mut arc) = Ty::Str;
        let second = arena.intern(&Ty::Array { elem: arc });
        assert!(!arena.same(first, second));
        assert_eq!(arena.get(first), &array(Ty::Int));
        let mut tuple = Ty::Tuple {
            elems: vec![Ty::Int].into(),
        };
        let before = arena.intern(&tuple);
        if let Ty::Tuple { elems } = &mut tuple {
            elems[0] = Ty::Str;
        }
        let after = arena.intern(&tuple);
        assert!(!arena.same(before, after));
        assert_eq!(
            arena.get(before),
            &Ty::Tuple {
                elems: vec![Ty::Int].into()
            }
        );
    }

    #[test]
    fn reference_ids_stay_nominal_and_functions_retain_all_metadata() {
        let mut arena = Arena::default();
        let r1 = arena.intern(&Ty::Rec { slot: 1 });
        let r2 = arena.intern(&Ty::Rec { slot: 2 });
        assert!(!arena.same(r1, r2));
        let f = Ty::Fn {
            params: vec![Param {
                name: Symbol::from("arg"),
                kind: ParamKind::Required,
                ty: Arc::new(record(false, Ty::Int)),
            }]
            .into(),
            block: Some(Arc::new(Ty::Bool)),
            ret: Arc::new(Ty::Nil),
            effects: EffectSet::pure(),
        };
        let mut variants = vec![f.clone()];
        if let Ty::Fn { effects, .. } = variants.last_mut().unwrap() {
            effects.insert(Effect::Io);
        }
        variants.push(f.clone());
        if let Ty::Fn { block, .. } = variants.last_mut().unwrap() {
            *block = None;
        }
        variants.push(f.clone());
        if let Ty::Fn { params, .. } = variants.last_mut().unwrap() {
            params[0].kind = ParamKind::Rest;
        }
        variants.push(f.clone());
        if let Ty::Fn { params, .. } = variants.last_mut().unwrap() {
            params[0].name = Symbol::from("other");
        }
        let id = arena.intern(&f);
        for variant in variants {
            let other = arena.intern(&variant);
            assert!(!arena.same(id, other));
            assert_eq!(
                serde_json::to_vec(arena.get(other)).unwrap(),
                serde_json::to_vec(&variant).unwrap()
            );
        }
    }

    #[test]
    fn identities_match_existing_equality_on_a_generated_nested_corpus() {
        let mut types = LEAVES.to_vec();
        types.extend([
            Ty::Var { var: TyVar(0) },
            Ty::Var { var: TyVar(1) },
            Ty::Rec { slot: 0 },
            Ty::Rec { slot: 1 },
            Ty::Relation {
                of: ClassId(Symbol::from("A")),
            },
            record(false, Ty::Int),
            record(true, Ty::Int),
        ]);
        let mut seed = 11u64;
        for i in 0..160 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let a = types[seed as usize % types.len()].clone();
            let b = types[(seed >> 32) as usize % types.len()].clone();
            types.push(match i % 8 {
                0 => array(a),
                1 => Ty::Hash {
                    key: Arc::new(a),
                    value: Arc::new(b),
                },
                2 => Ty::Tuple {
                    elems: vec![a, b].into(),
                },
                3 => Ty::Union {
                    variants: vec![a, b].into(),
                },
                4 => record(i % 3 == 0, a),
                5 => Ty::Class {
                    id: ClassId(Symbol::from("Container")),
                    args: vec![a, b].into(),
                },
                6 => Ty::Fn {
                    params: vec![Param {
                        name: Symbol::from("x"),
                        kind: ParamKind::Keyword { required: true },
                        ty: Arc::new(a),
                    }]
                    .into(),
                    block: None,
                    ret: Arc::new(b),
                    effects: EffectSet::pure(),
                },
                _ => Ty::Record {
                    row: Row {
                        fields: [(Symbol::from("x"), a)].into_iter().collect(),
                        rest: Some(TyVar((i % 4) as u32)),
                    },
                },
            });
        }
        let mut arena = Arena::default();
        let ids: Vec<_> = types.iter().map(|t| arena.intern(t)).collect();
        for (a, ia) in types.iter().zip(&ids) {
            assert_eq!(arena.intern(a), *ia);
            assert_eq!(
                serde_json::to_vec(arena.get(*ia)).unwrap(),
                serde_json::to_vec(a).unwrap()
            );
            for (b, ib) in types.iter().zip(&ids) {
                assert_eq!(arena.same(*ia, *ib), a == b);
            }
        }
    }

    #[test]
    fn shared_dag_is_interned_once_per_distinct_child() {
        let mut t = Ty::Int;
        for _ in 0..30 {
            let child = Arc::new(t);
            t = Ty::Hash {
                key: child.clone(),
                value: child,
            };
        }
        let mut arena = Arena::default();
        let id = arena.intern(&t);
        assert_eq!(arena.len(), LEAVES.len() + 30);
        assert_eq!(arena.intern(&t), id);
        assert!(arena.arc_hits >= 30);
        assert!(arena.cache_size() < 100);
    }

    #[test]
    fn memo_preserves_order_provenance_and_separate_analyses() {
        reset();
        let a = record(false, Ty::Int);
        let b = record(true, Ty::Int);
        let ab = memo_union(a.clone(), b.clone(), |left, _| left);
        let ba = memo_union(b.clone(), a.clone(), |left, _| left);
        assert_ne!(
            serde_json::to_vec(&ab).unwrap(),
            serde_json::to_vec(&ba).unwrap()
        );
        memo_union(a.clone(), b.clone(), |_, _| {
            panic!("must hit exact ordered key")
        });
        let p = memo_union(Ty::pending_untyped(), Ty::Int, |left, _| left);
        let g = memo_union(Ty::gradual(), Ty::Int, |left, _| left);
        assert_ne!(p.provenance(), g.provenance());
        reset();
        let mut computed = false;
        memo_union(a, Ty::Bool, |left, _| {
            computed = true;
            left
        });
        assert!(computed);
        reset();
    }

    #[test]
    fn recursive_memo_calls_compute_without_holding_a_borrow() {
        reset();
        fn combine(a: Ty, b: Ty) -> Ty {
            memo_union(a, b, |a, b| match (a, b) {
                (Ty::Array { elem: a }, Ty::Array { elem: b }) => {
                    array(combine((*a).clone(), (*b).clone()))
                }
                (a, b) => Ty::Tuple {
                    elems: vec![a, b].into(),
                },
            })
        }
        let a = array(array(Ty::Int));
        let b = array(array(Ty::Str));
        let first = combine(a.clone(), b.clone());
        let second = combine(a, b);
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&second).unwrap()
        );
        assert_eq!(MEMO.with(|m| m.borrow().binary.len()), 3);
        assert_eq!(MEMO.with(|m| m.borrow().hits), 1);
        reset();
    }
}

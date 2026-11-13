//! Effect classification: the `Effect` variants and the `EffectSet`
//! the analyzer attaches to every expression and to `Ty::Fn`
//! signatures — per-table `DbRead`/`DbWrite` (from the adapter's
//! Active Record method classification) plus deliberately coarse
//! Io/Time/Random/Net/Log/Raises where per-site precision isn't
//! earned. Populated by `analyze/effects`; downstream, purity is a
//! license — lowerer hook passes re-evaluate or drop a receiver only
//! when its subtree is effect-free — and the per-table Db grain is
//! what lets IDE/MCP consumers report which tables an action reads
//! and writes. The empty set means pure and is the serde default, so
//! pure nodes serialize with no effects field at all.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::ident::{ClassId, EffectVar, TableRef};

/// A single side-effect class. Precise where useful (which table is read/written),
/// coarse where precision isn't earned (Io, Time, Random).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    Io,
    DbRead { table: TableRef },
    DbWrite { table: TableRef },
    Time,
    Random,
    Raises { class: ClassId },
    Net { host: Option<String> },
    Log,
    Var { var: EffectVar },
}

/// The set of effects a computation may perform. Empty set == pure.
///
/// Every `Expr` carries one and almost none has an effect (0.07% of the
/// nodes of a large app), so the set sits behind one pointer and the
/// empty set allocates nothing: 8 bytes instead of a `BTreeSet`'s 24,
/// which also shrinks `Ty` (its `Fn` holds one) from 56 to 40. Equality,
/// hashing, `Debug` and the serialized shape (`{"effects": [...]}`) are
/// those of the `BTreeSet` it holds.
#[derive(Clone, Default)]
pub struct EffectSet {
    /// `None` when empty; a `Some` set is never empty.
    set: Option<Box<BTreeSet<Effect>>>,
}

static EMPTY: BTreeSet<Effect> = BTreeSet::new();

impl EffectSet {
    pub fn pure() -> Self {
        Self::default()
    }

    pub fn is_pure(&self) -> bool {
        self.set.is_none()
    }

    pub fn singleton(e: Effect) -> Self {
        Self::from(BTreeSet::from([e]))
    }

    pub fn insert(&mut self, e: Effect) {
        self.set.get_or_insert_with(Default::default).insert(e);
    }

    pub fn union(mut self, other: Self) -> Self {
        self.extend(other.effects().iter().cloned());
        self
    }

    /// The effects, in `Effect` order.
    pub fn effects(&self) -> &BTreeSet<Effect> {
        self.set.as_deref().unwrap_or(&EMPTY)
    }

    pub fn extend(&mut self, effects: impl IntoIterator<Item = Effect>) {
        let mut effects = effects.into_iter().peekable();
        if effects.peek().is_some() {
            self.set.get_or_insert_with(Default::default).extend(effects);
        }
    }
}

impl From<BTreeSet<Effect>> for EffectSet {
    fn from(set: BTreeSet<Effect>) -> Self {
        Self { set: (!set.is_empty()).then(|| Box::new(set)) }
    }
}

impl PartialEq for EffectSet {
    fn eq(&self, other: &Self) -> bool {
        self.effects() == other.effects()
    }
}

impl Eq for EffectSet {}

impl std::hash::Hash for EffectSet {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.effects().hash(state);
    }
}

impl PartialOrd for EffectSet {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EffectSet {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.effects().cmp(other.effects())
    }
}

impl std::fmt::Debug for EffectSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectSet").field("effects", self.effects()).finish()
    }
}

/// The serialized shape, as when the set was a plain `effects` field.
#[derive(Serialize)]
#[serde(rename = "EffectSet")]
struct EffectSetRef<'a> {
    effects: &'a BTreeSet<Effect>,
}

#[derive(Deserialize)]
#[serde(rename = "EffectSet")]
struct EffectSetOwned {
    effects: BTreeSet<Effect>,
}

impl Serialize for EffectSet {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        EffectSetRef { effects: self.effects() }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for EffectSet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(EffectSetOwned::deserialize(d)?.effects.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{BuildHasher, Hash, Hasher};
    use crate::ident::Symbol;

    fn sets() -> Vec<BTreeSet<Effect>> {
        let table = |name: &str| TableRef(Symbol::from(name));
        vec![
            BTreeSet::new(),
            BTreeSet::from([Effect::Io]),
            BTreeSet::from([Effect::Io, Effect::Log]),
            BTreeSet::from([Effect::DbRead { table: table("posts") }, Effect::Time]),
            BTreeSet::from([Effect::DbRead { table: table("posts") }, Effect::DbWrite { table: table("posts") }]),
            BTreeSet::from([Effect::Net { host: None }]),
        ]
    }

    #[test]
    fn the_empty_set_is_one_pointer_and_allocates_nothing() {
        assert_eq!(std::mem::size_of::<EffectSet>(), std::mem::size_of::<usize>());
        assert!(EffectSet::pure().set.is_none());
        assert!(EffectSet::from(BTreeSet::new()).set.is_none());
        let mut s = EffectSet::pure();
        s.extend(std::iter::empty());
        assert!(s.set.is_none() && s.is_pure());
        assert!(EffectSet::pure().union(EffectSet::pure()).set.is_none());
    }

    #[test]
    fn equality_hashing_order_and_serde_are_the_btreesets() {
        // One hasher state for both sides of each comparison.
        let state = std::collections::hash_map::RandomState::new();
        let hash = |value: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
            let mut h = state.build_hasher();
            value(&mut h);
            h.finish()
        };
        for a in sets() {
            let ea = EffectSet::from(a.clone());
            assert_eq!(ea.is_pure(), a.is_empty());
            assert_eq!(ea.effects(), &a);
            assert_eq!(hash(&|h| ea.hash(h)), hash(&|h| a.hash(h)));
            assert_eq!(format!("{ea:?}"), format!("EffectSet {{ effects: {a:?} }}"));
            #[derive(Serialize)]
            struct Before<'a> {
                effects: &'a BTreeSet<Effect>,
            }
            let json = serde_json::to_string(&ea).unwrap();
            assert_eq!(json, serde_json::to_string(&Before { effects: &a }).unwrap());
            assert_eq!(serde_json::from_str::<EffectSet>(&json).unwrap(), ea);
            let mut built = EffectSet::pure();
            for e in a.iter().rev() {
                built.insert(e.clone());
            }
            assert_eq!(built, ea);
            assert!(built.effects().iter().eq(a.iter()));
            for b in sets() {
                let eb = EffectSet::from(b.clone());
                assert_eq!(ea == eb, a == b);
                assert_eq!(ea.cmp(&eb), a.cmp(&b));
                let union: BTreeSet<Effect> = a.union(&b).cloned().collect();
                assert_eq!(ea.clone().union(eb).effects(), &union);
            }
        }
    }
}

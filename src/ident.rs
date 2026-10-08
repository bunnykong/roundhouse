//! Identifier newtypes shared by every IR layer: `Symbol` for names,
//! `ClassId` / `TableRef` for class and table references, and the
//! `VarId` / `TyVar` / `EffectVar` counters the analyzer allocates
//! during inference. Ingest mints the named ones; everything
//! downstream keys its maps and registries by them. They wrap plain
//! strings today, but the newtypes are the point: a class reference
//! can never be confused with a table name or a bare method symbol in
//! a signature, and the representation can switch to true interning
//! later without touching a single consumer.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// A textual name (methods, variables, types). Newtype so the internal
/// representation can switch to an interned form later without breaking consumers.
/// Clones share immutable text; equality, ordering, hashing and serialization
/// continue to use the text rather than allocation identity.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Symbol(Arc<str>);

impl Symbol {
    pub fn new(s: impl Into<String>) -> Self {
        Self(Arc::from(s.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for Symbol {
    fn from(s: &str) -> Self {
        Symbol(Arc::from(s))
    }
}

impl From<String> for Symbol {
    fn from(s: String) -> Self {
        Symbol(Arc::from(s))
    }
}

/// Locally-unique id for a variable binding.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VarId(pub u32);

/// Type inference variable.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TyVar(pub u32);

/// Effect inference variable.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EffectVar(pub u32);

/// Stable reference to a class by name.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClassId(pub Symbol);

impl fmt::Display for ClassId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Stable reference to a database table by name.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TableRef(pub Symbol);

impl fmt::Display for TableRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::Symbol;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    #[test]
    fn shared_symbols_preserve_text_hash_and_wire_format() {
        for text in ["", "Account::Status", "ready?", "café"] {
            let symbol = Symbol::from(text);
            let owned = Symbol::from(text.to_owned());
            let mut before = DefaultHasher::new();
            text.to_owned().hash(&mut before);
            let mut after = DefaultHasher::new();
            symbol.hash(&mut after);
            assert_eq!(before.finish(), after.finish());
            assert_eq!(symbol, owned);
            let encoded = serde_json::to_string(&symbol).unwrap();
            assert_eq!(encoded, serde_json::to_string(text).unwrap());
            assert_eq!(symbol, serde_json::from_str::<Symbol>(&encoded).unwrap());
            assert_eq!(format!("{symbol:?}"), format!("Symbol({text:?})"));
        }
    }
}

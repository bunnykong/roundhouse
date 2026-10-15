//! Versioned native storage for chronological warm replay.
//!
//! Ordinary IR/emitter serialization remains unchanged. Only calls through
//! this API enable reason bits and ordered record fields, recursively through
//! the same serde implementation used for expressions and fold writes.
use std::cell::{Cell, RefCell};

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::Value;

use crate::{Symbol, Ty, shared::Shared, ty::Provenance};

pub const LOSSLESS_SCHEMA: &str =
    "rh-warm-native-v1:pending=1,gradual=2,unresolved=4;row=ordered-pairs";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarmCodec {
    LegacyV2,
    LosslessV3,
}

#[derive(Clone, Default, Serialize)]
pub struct Counters {
    encoded_values: u64,
    decoded_values: u64,
    encoded_reason_leaves: u64,
    decoded_reason_leaves: u64,
    encoded_ordered_rows: u64,
    decoded_ordered_rows: u64,
}

thread_local! {
    static LOSSLESS: Cell<bool> = const { Cell::new(false) };
    static COUNTERS: RefCell<Counters> = RefCell::new(Counters::default());
}

struct Scope(bool);
impl Scope {
    fn enter(lossless: bool) -> Self {
        Self(LOSSLESS.with(|active| active.replace(lossless)))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        LOSSLESS.with(|active| active.set(self.0));
    }
}

pub(crate) fn active() -> bool {
    LOSSLESS.with(Cell::get)
}

impl WarmCodec {
    pub fn selected() -> Self {
        if std::env::var("RH_WARM_LOSSLESS_CODEC").as_deref() == Ok("1") {
            Self::LosslessV3
        } else {
            Self::LegacyV2
        }
    }

    pub fn cache_schema(self) -> u32 {
        match self {
            Self::LegacyV2 => 2,
            Self::LosslessV3 => 3,
        }
    }

    pub fn identity(self) -> &'static str {
        match self {
            Self::LegacyV2 => "",
            Self::LosslessV3 => LOSSLESS_SCHEMA,
        }
    }

    pub fn to_value<T: Serialize + ?Sized>(self, value: &T) -> serde_json::Result<Value> {
        let _scope = Scope::enter(self == Self::LosslessV3);
        let value = serde_json::to_value(value)?;
        if active() {
            COUNTERS.with(|c| c.borrow_mut().encoded_values += 1);
        }
        Ok(value)
    }

    pub fn from_value<T: DeserializeOwned>(self, value: Value) -> serde_json::Result<T> {
        if self == Self::LosslessV3 && !self.valid(&value) {
            return Err(serde::de::Error::custom(
                "ambiguous or incompatible native type storage",
            ));
        }
        let _scope = Scope::enter(self == Self::LosslessV3);
        let value = serde_json::from_value(value)?;
        if active() {
            COUNTERS.with(|c| c.borrow_mut().decoded_values += 1);
        }
        Ok(value)
    }

    /// Reject omitted/unknown reasons and legacy or duplicate record fields.
    /// Source and recursive-slot identities are independently checked by wire.
    pub fn valid(self, value: &Value) -> bool {
        if self == Self::LegacyV2 {
            return true;
        }
        match value {
            Value::Object(map) => {
                if map.get("kind").and_then(Value::as_str) == Some("untyped")
                    && !map
                        .get("why")
                        .and_then(Value::as_u64)
                        .is_some_and(|bits| (1..=7).contains(&bits))
                {
                    return false;
                }
                if map.get("kind").and_then(Value::as_str) == Some("record")
                    && map.contains_key("row")
                {
                    let Some(fields) = map["row"].get("fields").and_then(Value::as_array) else {
                        return false;
                    };
                    let mut names = std::collections::HashSet::new();
                    for pair in fields {
                        let Some(pair) = pair.as_array().filter(|p| p.len() == 2) else {
                            return false;
                        };
                        let Some(name) = pair[0].as_str() else {
                            return false;
                        };
                        if !names.insert(name) {
                            return false;
                        }
                    }
                }
                map.values().all(|v| self.valid(v))
            }
            Value::Array(values) => values.iter().all(|v| self.valid(v)),
            _ => true,
        }
    }

    pub fn counters(self) -> Value {
        let mut value = COUNTERS.with(|c| serde_json::to_value(c.borrow().clone()).unwrap());
        value["identity"] = Value::from(self.identity());
        value["cache_schema"] = Value::from(self.cache_schema());
        value
    }
}

pub(crate) fn reset_counters() {
    COUNTERS.with(|c| *c.borrow_mut() = Counters::default());
}

pub(crate) fn skip_reasons(_: &Provenance) -> bool {
    !active()
}

pub(crate) fn serialize_reasons<S: Serializer>(
    why: &Provenance,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if active() {
        COUNTERS.with(|c| c.borrow_mut().encoded_reason_leaves += 1);
    }
    serializer.serialize_u8(why.reporting_bits())
}

pub(crate) fn deserialize_reasons<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Provenance, D::Error> {
    if !active() {
        let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
        return Ok(Provenance::default());
    }
    let bits = u8::deserialize(deserializer)?;
    let why = Provenance::from_bits(bits)
        .ok_or_else(|| serde::de::Error::custom("invalid reason bits"))?;
    if active() {
        COUNTERS.with(|c| c.borrow_mut().decoded_reason_leaves += 1);
    }
    Ok(why)
}

pub(crate) fn serialize_fields<S: Serializer>(
    fields: &Shared<IndexMap<Symbol, Ty>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if active() {
        COUNTERS.with(|c| c.borrow_mut().encoded_ordered_rows += 1);
        fields.iter().collect::<Vec<_>>().serialize(serializer)
    } else {
        fields.serialize(serializer)
    }
}

pub(crate) fn deserialize_fields<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Shared<IndexMap<Symbol, Ty>>, D::Error> {
    if active() {
        let pairs = Vec::<(Symbol, Ty)>::deserialize(deserializer)?;
        let len = pairs.len();
        let fields: IndexMap<_, _> = pairs.into_iter().collect();
        if fields.len() != len {
            return Err(serde::de::Error::custom("duplicate record field"));
        }
        COUNTERS.with(|c| c.borrow_mut().decoded_ordered_rows += 1);
        Ok(fields.into())
    } else {
        Shared::<IndexMap<Symbol, Ty>>::deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_codec_rejects_ambiguous_payloads_and_restores_scope() {
        let codec = WarmCodec::LosslessV3;
        for value in [
            serde_json::json!({"kind":"untyped"}),
            serde_json::json!({"kind":"untyped","why":0}),
            serde_json::json!({"kind":"untyped","why":8}),
            serde_json::json!({"kind":"record","row":{"fields":{"a":{"kind":"int"}},"rest":null}}),
            serde_json::json!({"kind":"record","row":{"fields":[["a",{"kind":"int"}],["a",{"kind":"str"}]],"rest":null}}),
        ] {
            assert!(codec.from_value::<Ty>(value).is_err());
        }
        assert_eq!(
            serde_json::to_value(Ty::pending_untyped()).unwrap(),
            serde_json::json!({"kind":"untyped"})
        );
        assert_eq!(
            serde_json::from_value::<Ty>(serde_json::json!({"kind":"untyped","why":4}))
                .unwrap()
                .provenance(),
            Some(Provenance::Gradual),
            "ordinary reader must retain its legacy skipped-field behavior"
        );
        assert!(
            codec
                .from_value::<Ty>(
                    serde_json::json!({"kind":"tuple","elems":[{"kind":"not_a_type"}]})
                )
                .is_err()
        );
        assert!(!active(), "failed serde decode leaked the storage scope");
        let value = codec.to_value(&Ty::unresolved()).unwrap();
        assert_eq!(value["why"], 4);
        assert_eq!(
            codec.from_value::<Ty>(value).unwrap().provenance(),
            Some(Provenance::Unresolved)
        );
        assert!(!active());
    }
}

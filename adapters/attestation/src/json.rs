//! Bounded I-JSON parsing for attestation payloads and content identities.
use crate::{AttestationError, error};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = UniqueValue;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("I-JSON without duplicate keys")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(value)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(value.into())))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(value)))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                if value > MAX_SAFE_INTEGER {
                    return Err(E::custom(
                        "integer exceeds the interoperable contract range",
                    ));
                }
                Ok(UniqueValue(Value::Number(value.into())))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                if value.unsigned_abs() > MAX_SAFE_INTEGER {
                    return Err(E::custom(
                        "integer exceeds the interoperable contract range",
                    ));
                }
                Ok(UniqueValue(Value::Number(value.into())))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                let number = serde_json::Number::from_f64(value)
                    .ok_or_else(|| E::custom("non-finite JSON number"))?;
                Ok(UniqueValue(Value::Number(number)))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut names = BTreeSet::new();
                let mut values = Map::new();
                while let Some(name) = map.next_key::<String>()? {
                    if !names.insert(name.clone()) {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                    values.insert(name, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}

pub fn parse(bytes: &[u8]) -> Result<Value, AttestationError> {
    if bytes.len() > crate::dsse::MAX_PAYLOAD_BYTES {
        return Err(error("attestation.payload_limit"));
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value =
        UniqueValue::deserialize(&mut decoder).map_err(|_| error("attestation.invalid_json"))?;
    decoder
        .end()
        .map_err(|_| error("attestation.invalid_json"))?;
    Ok(value.0)
}
/// RFC8785 serialization uses the library; the contract profile rejects unsafe integer literals.
pub fn canonical_bytes(bytes: &[u8]) -> Result<Vec<u8>, AttestationError> {
    serde_jcs::to_vec(&parse(bytes)?).map_err(|_| error("attestation.canonical_json_failed"))
}

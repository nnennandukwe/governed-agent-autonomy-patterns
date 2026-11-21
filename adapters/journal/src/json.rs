use crate::{Error, Result, canonical};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

// Check depth before descending and reject duplicates before Serde can discard them.
struct AtDepth(usize);

impl<'de> DeserializeSeed<'de> for AtDepth {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> std::result::Result<Value, D::Error> {
        de.deserialize_any(self)
    }
}

impl AtDepth {
    fn child<E: de::Error>(&self) -> std::result::Result<AtDepth, E> {
        if self.0 >= 64 {
            return Err(E::custom("container nesting exceeds 64"));
        }
        Ok(AtDepth(self.0 + 1))
    }
}

impl<'de> Visitor<'de> for AtDepth {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded JSON with unique keys")
    }

    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Value, E> {
        Ok(value.into())
    }

    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Value, E> {
        Ok(value.into())
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Value, E> {
        if value > crate::MAX_SAFE {
            return Err(E::custom("integer exceeds the JCS safe range"));
        }
        Ok(value.into())
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Value, E> {
        if value.unsigned_abs() > crate::MAX_SAFE {
            return Err(E::custom("integer exceeds the JCS safe range"));
        }
        Ok(value.into())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut entries: A) -> std::result::Result<Value, A::Error> {
        let next = self.child()?;
        let mut items = Vec::new();
        while let Some(item) = entries.next_element_seed(AtDepth(next.0))? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> std::result::Result<Value, A::Error> {
        let next = self.child()?;
        let mut object = Map::new();
        while let Some(key) = entries.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom("duplicate object member"));
            }
            object.insert(key, entries.next_value_seed(AtDepth(next.0))?);
        }
        Ok(Value::Object(object))
    }
}

pub fn json_document(bytes: &[u8], canonical_required: bool) -> Result<Value> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let value = AtDepth(0)
        .deserialize(&mut de)
        .map_err(|_| Error::invalid("invalid JSON, member, depth or number"))?;
    de.end()
        .map_err(|_| Error::invalid("unexpected data after JSON"))?;
    if canonical_required && canonical(&value)? != bytes {
        return Err(Error::invalid("noncanonical stored bytes"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_nested_identity_bounds_and_stored_bytes() {
        assert!(json_document(br#"{"x":{"a":1,"\u0061":2}}"#, false).is_err());
        assert!(json_document(br#"{"x":1} "#, true).is_err());
        assert!(json_document(br#"{"x":1} "#, false).is_ok());
        assert!(json_document(br#"{"x":9007199254740992}"#, false).is_err());
        for (depth, valid) in [(64, true), (65, false)] {
            let data = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(json_document(data.as_bytes(), true).is_ok(), valid);
        }
    }
}

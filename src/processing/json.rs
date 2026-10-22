use crate::protocol::{Source, Tool};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt;

pub(crate) fn candidates(tool: &Tool, text: &str) -> Option<Vec<(&'static str, String)>> {
    if !matches!(tool.source, Source::Unknown | Source::Search) {
        return None;
    }
    selected_candidates(text, true, true)
}

pub(super) fn selected_candidates(
    text: &str,
    use_compact: bool,
    use_toon: bool,
) -> Option<Vec<(&'static str, String)>> {
    let value = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => value,
        Err(_) if matches!(text.trim_start().chars().next(), Some('{') | Some('[')) => {
            return use_compact.then(|| vec![("json_compact", text.to_owned())]);
        }
        Err(_) => return None,
    };
    let (compact, toon_numbers_safe) = compact_json(text);
    let mut candidates = Vec::new();
    if use_compact {
        candidates.push(("json_compact", compact));
    }

    // Value parsing can round numeric literals and discard duplicate object keys.
    if use_toon
        && toon_numbers_safe
        && serde_json::from_str::<UniqueJson>(text).is_ok()
        && let Ok(toon) = toon_format::encode_default(&value)
        && toon_format::decode_default::<serde_json::Value>(&toon)
            .is_ok_and(|decoded| decoded == value)
    {
        candidates.push(("json_toon", toon));
    }
    Some(candidates)
}

pub(crate) fn rejection(text: &str) -> Option<&'static str> {
    if !matches!(text.trim_start().chars().next(), Some('{') | Some('[')) {
        return None;
    }
    let mut deserializer = serde_json::Deserializer::from_str(text);
    match serde_json::Value::deserialize(&mut deserializer) {
        Err(_) => Some("parse_failed"),
        Ok(_) if deserializer.end().is_err() => Some("extra_content"),
        Ok(_) => None,
    }
}

fn compact_json(text: &str) -> (String, bool) {
    const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
    let bytes = text.as_bytes();
    let mut compact = String::with_capacity(text.len());
    let mut toon_numbers_safe = true;
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            let character = text[index..].chars().next().unwrap();
            compact.push(character);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += character.len_utf8();
        } else if byte == b'"' {
            in_string = true;
            compact.push('"');
            index += 1;
        } else if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            index += 1;
        } else if byte == b'-' || byte.is_ascii_digit() {
            let start = index;
            index += 1;
            while index < bytes.len()
                && !matches!(
                    bytes[index],
                    b',' | b']' | b'}' | b':' | b' ' | b'\t' | b'\r' | b'\n'
                )
            {
                index += 1;
            }
            let number = &text[start..index];
            compact.push_str(number);
            toon_numbers_safe &= number != "-0"
                && number
                    .parse::<i64>()
                    .is_ok_and(|value| (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value));
        } else {
            let character = text[index..].chars().next().unwrap();
            compact.push(character);
            index += character.len_utf8();
        }
    }
    (compact, toon_numbers_safe)
}

struct UniqueJson;

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

struct UniqueJsonVisitor;

impl<'de> Visitor<'de> for UniqueJsonVisitor {
    type Value = UniqueJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }

    fn visit_borrowed_str<E>(self, _value: &'de str) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<UniqueJson>()?.is_some() {}
        Ok(UniqueJson)
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = std::collections::HashSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate object key"));
            }
            object.next_value::<UniqueJson>()?;
        }
        Ok(UniqueJson)
    }
}

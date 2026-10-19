use crate::protocol::{Source, Tool};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt;

pub(crate) fn protected(tool: &Tool) -> bool {
    if matches!(tool.source, Source::SourceCode | Source::Configuration) {
        return true;
    }
    ["path", "file_path", "filePath"].iter().any(|key| {
        let Some(path) = tool.input.get(key).and_then(|value| value.as_str()) else {
            return false;
        };
        let path = path.replace('\\', "/").to_ascii_lowercase();
        let name = path.rsplit('/').next().unwrap_or(&path);
        let extension = name.rsplit('.').next().unwrap_or("");
        matches!(
            name,
            "dockerfile" | "makefile" | "justfile" | ".env" | ".gitignore"
        ) || matches!(
            extension,
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "py"
                | "go"
                | "c"
                | "h"
                | "cpp"
                | "hpp"
                | "java"
                | "kt"
                | "swift"
                | "rb"
                | "sh"
                | "ps1"
                | "json"
                | "jsonc"
                | "toml"
                | "yaml"
                | "yml"
                | "ini"
                | "cfg"
                | "xml"
                | "lock"
        )
    })
}

pub(crate) fn log_tail(tool: &Tool, text: &str) -> Option<String> {
    if !matches!(
        tool.source,
        Source::BuildLog | Source::TestLog | Source::RuntimeLog
    ) {
        return None;
    }
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    Some(lines[lines.len().saturating_sub(10)..].concat())
}

pub(crate) fn json_candidates(tool: &Tool, text: &str) -> Option<Vec<(&'static str, String)>> {
    if !matches!(tool.source, Source::Unknown) {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let compact = compact_json(text);
    let mut candidates = vec![("json_compact", compact)];
    if safe_toon_numbers(text)
        && serde_json::from_str::<UniqueJson>(text).is_ok()
        && let Ok(toon) = toon_format::encode_default(&value)
        && toon_format::decode_default::<serde_json::Value>(&toon)
            .is_ok_and(|decoded| decoded == value)
    {
        candidates.push(("json_toon", toon));
    }
    Some(candidates)
}

fn compact_json(text: &str) -> String {
    let mut compact = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for character in text.chars() {
        if in_string {
            compact.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
            compact.push(character);
        } else if !matches!(character, ' ' | '\t' | '\r' | '\n') {
            compact.push(character);
        }
    }
    compact
}

fn safe_toon_numbers(text: &str) -> bool {
    // Exclude decimals and integers outside IEEE-754's exact integer range.
    const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
        } else if byte == b'"' {
            in_string = true;
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
            if number == "-0"
                || number
                    .bytes()
                    .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
                || !number
                    .parse::<i64>()
                    .is_ok_and(|value| (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value))
            {
                return false;
            }
        } else {
            index += 1;
        }
    }
    true
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

//! ASCII JSON: every character outside printable ASCII is written as `\uXXXX`, one escape
//! per UTF-16 code unit, so a character outside the Basic Multilingual Plane becomes a
//! surrogate pair. U+007F is escaped too. Hex digits are lower case, as in the Node CLI.
//!
//! JSON syntax itself is ASCII, so a non-ASCII character can only stand inside a string;
//! escaping the serialized text is therefore exact, and it also covers `RawValue` parts
//! that serde_json copies through untouched.

use std::fmt::Write as _;

use serde::Serialize;

/// Escapes the non-ASCII characters of an already serialized JSON text.
pub fn to_ascii_string(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for ch in json.chars() {
        if (' '..='~').contains(&ch) || matches!(ch, '\n' | '\r' | '\t') {
            out.push(ch);
        } else {
            let mut units = [0u16; 2];
            for unit in ch.encode_utf16(&mut units) {
                // Writing to a String cannot fail.
                let _ = write!(out, "\\u{unit:04x}");
            }
        }
    }
    out
}

/// Serializes `value` compactly and escapes it to ASCII.
pub fn to_ascii_json<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    serde_json::to_string(value).map(|json| to_ascii_string(&json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serde_json::value::RawValue;

    #[test]
    fn ascii_passes_through() {
        assert_eq!(
            to_ascii_json(&json!({"a": "b c~"})).unwrap(),
            r#"{"a":"b c~"}"#
        );
    }

    #[test]
    fn turkish_letters_become_escapes() {
        assert_eq!(
            to_ascii_json(&json!("çığ")).unwrap(),
            "\"\\u00e7\\u0131\\u011f\""
        );
    }

    #[test]
    fn emoji_becomes_a_surrogate_pair() {
        // U+1F98E LIZARD
        assert_eq!(to_ascii_json(&json!("🦎")).unwrap(), "\"\\ud83e\\udd8e\"");
    }

    #[test]
    fn delete_character_is_escaped() {
        assert_eq!(to_ascii_json(&json!("a\u{7f}b")).unwrap(), r#""a\u007fb""#);
    }

    #[test]
    fn control_characters_keep_serde_json_escapes() {
        assert_eq!(
            to_ascii_json(&json!("a\nb\u{1}")).unwrap(),
            r#""a\nb\u0001""#
        );
    }

    #[test]
    fn raw_values_are_escaped_too() {
        let raw = RawValue::from_string(r#"{"name":"Gökçe"}"#.to_owned()).unwrap();
        assert_eq!(
            to_ascii_json(&json!({"data": raw})).unwrap(),
            "{\"data\":{\"name\":\"G\\u00f6k\\u00e7e\"}}"
        );
    }

    #[test]
    fn field_order_is_kept() {
        let value: serde_json::Value = serde_json::from_str(r#"{"z":1,"a":2,"m":3}"#).unwrap();
        assert_eq!(to_ascii_json(&value).unwrap(), r#"{"z":1,"a":2,"m":3}"#);
    }

    #[test]
    fn output_is_parseable_and_round_trips() {
        let original = json!({"text": "Gecko 🦎 ağ\u{7f}", "n": [1, 2]});
        let ascii = to_ascii_json(&original).unwrap();
        assert!(ascii.is_ascii());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&ascii).unwrap(),
            original
        );
    }
}

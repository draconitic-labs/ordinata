//! Canonical JSON, content hashing, and strict JSON intake.
//!
//! Every content hash Ordinata publishes is the SHA-256 of the canonical form
//! produced here. The rules are fixed by [`crate::spec::CANONICAL_JSON_RULE`]:
//!
//! * object keys ascend by UTF-8 byte order;
//! * there is no insignificant whitespace;
//! * no number is `NaN` or an infinity;
//! * negative zero is written as zero.
//!
//! The key ordering is performed here rather than delegated to `serde_json`'s map
//! type, because a hash must not depend on which Cargo features some other crate in
//! the graph happened to turn on.
//!
//! # Reading JSON, as opposed to writing it
//!
//! Every JSON document that reaches Ordinata from outside -- a `project.json`, a
//! manifest inside a bundle someone handed you -- comes in through [`parse_strict`],
//! never through `serde_json::from_slice` directly. The reason is duplicate keys.
//!
//! JSON's grammar permits `{"lambda": 0.0, "lambda": 1e9}`, and every mainstream
//! parser, `serde_json` included, quietly keeps the last one. That is a fine default
//! for a config file and a bad one for evidence: two readers of the same bytes can
//! disagree about what they say, which is precisely the property an evidence bundle
//! exists to deny. A document whose meaning depends on which parser read it is not a
//! document Ordinata will accept, so a repeated key is a refusal.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Parse JSON, refusing any object that names the same key twice.
///
/// Returns the offending key in the error, because "duplicate key" without saying
/// which one sends a user hunting through a file by hand.
///
/// # Errors
///
/// Returns the parser's own message for malformed JSON, and a message naming the
/// repeated key for a duplicate. Both come back as a plain `String` so that each
/// caller can dress it in whichever typed error suits its trust boundary: a bad
/// `project.json` is an input refusal, a bad `manifest.json` is a bundle refusal, and
/// they exit with different codes.
pub fn parse_strict(bytes: &[u8]) -> std::result::Result<Value, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Strict::deserialize(&mut deserializer).map_err(|e| e.to_string())?;
    // `from_slice` stops at the end of the first value and ignores whatever follows.
    // Trailing content means the document is not one JSON value, and a file that is
    // two documents in a trench coat is not one Ordinata will guess about.
    deserializer.end().map_err(|e| e.to_string())?;
    Ok(value.0)
}

/// A `serde_json::Value` that will not deserialize an object with a repeated key.
///
/// This mirrors `Value`'s own deserializer exactly except at the map: where `Value`
/// inserts and lets a later key overwrite an earlier one, this checks first and fails.
/// It recurses through itself, so a duplicate nested twenty levels down is caught just
/// as a top-level one is.
struct Strict(Value);

impl<'de> serde::Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::{MapAccess, SeqAccess, Visitor};
        use std::fmt;

        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Strict;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_unit<E>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_none<E>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_bool<E>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Number(v.into())))
            }
            fn visit_u64<E>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Number(v.into())))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                // A non-finite float cannot appear in JSON text at all, so this is
                // belt and braces rather than a live path.
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom(format!("{v} is not a JSON number")))
            }
            fn visit_str<E>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::String(v.to_owned())))
            }
            fn visit_string<E>(self, v: String) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::String(v)))
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut items = Vec::new();
                while let Some(Strict(item)) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Strict(Value::Array(items)))
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Strict, A::Error> {
                use serde::de::Error as _;
                let mut object = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    let Strict(value) = map.next_value()?;
                    if object.contains_key(&key) {
                        return Err(A::Error::custom(format!(
                            "the object names the key {:?} more than once; a document whose meaning depends on which duplicate a parser keeps is not evidence",
                            crate::text::for_message(&key)
                        )));
                    }
                    object.insert(key, value);
                }
                Ok(Strict(Value::Object(object)))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

use serde::Deserialize as _;

/// Serialization shape.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    /// No whitespace. This is the form that gets hashed.
    Canonical,
    /// Two-space indent. This is the form written into a bundle for a human to read.
    Pretty,
}

/// Build a JSON number from an `f64`, refusing values that cannot be canonicalized.
///
/// # Errors
///
/// Returns [`Error::Internal`] if `value` is `NaN` or infinite. Reaching this is a
/// bug: every path that can produce a non-finite number is supposed to have refused
/// already with a [`crate::error::NumericalError`].
pub fn number(value: f64) -> Result<Value> {
    if !value.is_finite() {
        return Err(Error::Internal(format!(
            "refusing to canonicalize the non-finite number {value:?}"
        )));
    }
    // -0.0 and 0.0 are the same quantity but serialize differently, and the sign of a
    // zero is an artifact of the arithmetic that produced it, not a fact about the
    // result. Collapsing it keeps repeat runs hash-stable.
    let normalized = if value == 0.0 { 0.0 } else { value };
    serde_json::Number::from_f64(normalized)
        .map(Value::Number)
        .ok_or_else(|| Error::Internal(format!("cannot represent {value:?} as JSON")))
}

/// Build a JSON value from an optional `f64`, mapping `None` to `null`.
///
/// A missing number is written as `null`, never as zero. Ordinata does not use zero
/// to mean "not available".
///
/// # Errors
///
/// As [`number`].
pub fn optional_number(value: Option<f64>) -> Result<Value> {
    match value {
        Some(v) => number(v),
        None => Ok(Value::Null),
    }
}

/// Build a JSON array from a slice of `f64`.
///
/// # Errors
///
/// As [`number`].
pub fn number_array(values: &[f64]) -> Result<Value> {
    values
        .iter()
        .map(|v| number(*v))
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

/// The canonical bytes of `value`.
///
/// # Errors
///
/// Returns [`Error::Internal`] if the value contains a number that cannot be
/// canonicalized.
pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_value(value, Style::Canonical, 0, &mut out)?;
    Ok(out)
}

/// Deterministic, human-readable bytes of `value`, with a trailing newline.
///
/// Key ordering and number formatting match [`canonical_bytes`], so this form is
/// just as reproducible; it is only easier to read.
///
/// # Errors
///
/// As [`canonical_bytes`].
pub fn pretty_bytes(value: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_value(value, Style::Pretty, 0, &mut out)?;
    out.push(b'\n');
    Ok(out)
}

/// The lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex(&digest)
}

/// The SHA-256 of the canonical form of `value`.
///
/// # Errors
///
/// As [`canonical_bytes`].
pub fn canonical_hash(value: &Value) -> Result<String> {
    Ok(sha256_hex(&canonical_bytes(value)?))
}

// There is deliberately no `hash_file(path)` here. Ordinata hashes *bytes it already
// holds*, never a path it would have to open a second time: see `ingest::read_bounded`
// for why a file that is hashed on one read and parsed on another can put the checksum
// of one file next to the answer to a different one. A convenience that streams a path
// through SHA-256 is exactly the shape of that mistake, so it does not exist.

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn write_value(value: &Value, style: Style, depth: usize, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => write_number(n, out)?,
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            if items.is_empty() {
                out.extend_from_slice(b"[]");
                return Ok(());
            }
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                newline_indent(style, depth + 1, out);
                write_value(item, style, depth + 1, out)?;
            }
            newline_indent(style, depth, out);
            out.push(b']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.extend_from_slice(b"{}");
                return Ok(());
            }
            // Sort here rather than relying on the map's own iteration order.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push(b'{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                newline_indent(style, depth + 1, out);
                write_string(key, out);
                out.push(b':');
                if style == Style::Pretty {
                    out.push(b' ');
                }
                let child = map.get(key.as_str()).ok_or_else(|| {
                    Error::Internal("object key vanished between listing and lookup".into())
                })?;
                write_value(child, style, depth + 1, out)?;
            }
            newline_indent(style, depth, out);
            out.push(b'}');
        }
    }
    Ok(())
}

fn newline_indent(style: Style, depth: usize, out: &mut Vec<u8>) {
    if style == Style::Pretty {
        out.push(b'\n');
        for _ in 0..depth {
            out.extend_from_slice(b"  ");
        }
    }
}

fn write_number(n: &serde_json::Number, out: &mut Vec<u8>) -> Result<()> {
    if n.is_f64() {
        let v = n
            .as_f64()
            .ok_or_else(|| Error::Internal("a float JSON number did not read back".into()))?;
        // Round-trips through `number` so that a value parsed back out of a bundle
        // canonicalizes to the same bytes it was written from.
        out.extend_from_slice(number(v)?.to_string().as_bytes());
    } else {
        out.extend_from_slice(n.to_string().as_bytes());
    }
    Ok(())
}

fn write_string(s: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for ch in s.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{08}' => out.extend_from_slice(b"\\b"),
            '\u{0c}' => out.extend_from_slice(b"\\f"),
            c if (c as u32) < 0x20 => {
                let mut buf = [0_u8; 6];
                let hex_digits = b"0123456789abcdef";
                buf[0] = b'\\';
                buf[1] = b'u';
                buf[2] = b'0';
                buf[3] = b'0';
                let code = c as u32;
                buf[4] = hex_digits[((code >> 4) & 0xf) as usize];
                buf[5] = hex_digits[(code & 0xf) as usize];
                out.extend_from_slice(&buf);
            }
            c => {
                let mut buf = [0_u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn object_keys_are_sorted_by_byte_order() {
        let value = json!({ "zebra": 1, "Apple": 2, "apple": 3, "_under": 4 });
        let bytes = canonical_bytes(&value).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"Apple":2,"_under":4,"apple":3,"zebra":1}"#
        );
    }

    #[test]
    fn nested_objects_are_sorted_at_every_depth() {
        let value = json!({ "b": { "z": 1, "a": 2 }, "a": [ { "y": 1, "x": 2 } ] });
        let bytes = canonical_bytes(&value).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"a":[{"x":2,"y":1}],"b":{"a":2,"z":1}}"#
        );
    }

    #[test]
    fn negative_zero_canonicalizes_to_zero() {
        let a = canonical_hash(&json!({ "v": number(-0.0).unwrap() })).unwrap();
        let b = canonical_hash(&json!({ "v": number(0.0).unwrap() })).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            String::from_utf8(canonical_bytes(&number(-0.0).unwrap()).unwrap()).unwrap(),
            "0.0"
        );
    }

    #[test]
    fn non_finite_numbers_are_refused() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = number(bad).unwrap_err();
            assert_eq!(err.code(), "internal_invariant");
        }
    }

    #[test]
    fn floats_round_trip_through_text_without_changing_the_hash() {
        // The verifier rebuilds numbers by parsing them back out of the bundle's CSV
        // and JSON. If that round trip were lossy, every verification would fail.
        let values = [
            0.1,
            1.0 / 3.0,
            1e-300,
            1e300,
            std::f64::consts::PI,
            -2.220_446_049_250_313e-16,
            5e-324,
        ];
        for v in values {
            let written = number(v).unwrap();
            let text = String::from_utf8(canonical_bytes(&written).unwrap()).unwrap();
            let reparsed: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                canonical_bytes(&reparsed).unwrap(),
                canonical_bytes(&written).unwrap(),
                "{v:?} did not round trip through {text}"
            );
            // Bit equality: the whole claim of this test is that the round trip through
            // text is *lossless*, and a tolerance here would be quietly conceding that
            // it is not.
            assert_eq!(
                text.parse::<f64>().unwrap().to_bits(),
                v.to_bits(),
                "{v:?} lost precision through {text}"
            );
        }
    }

    #[test]
    fn control_characters_and_quotes_are_escaped() {
        let value = json!({ "k": "a\"b\\c\nd\u{1}e</script>" });
        let text = String::from_utf8(canonical_bytes(&value).unwrap()).unwrap();
        assert_eq!(text, r#"{"k":"a\"b\\c\nd\u0001e</script>"}"#);
    }

    #[test]
    fn pretty_and_canonical_agree_after_reparsing() {
        let value = json!({ "b": [1, 2.5, {"z": null, "a": true}], "a": "x" });
        let pretty = String::from_utf8(pretty_bytes(&value).unwrap()).unwrap();
        let reparsed: Value = serde_json::from_str(&pretty).unwrap();
        assert_eq!(
            canonical_hash(&reparsed).unwrap(),
            canonical_hash(&value).unwrap()
        );
        assert!(pretty.contains("\n  "), "pretty form should be indented");
    }

    #[test]
    fn a_repeated_key_is_refused_instead_of_silently_last_winning() {
        // serde_json's own default would accept this and keep 1e9. That is the exact
        // behaviour being refused: two readers of these bytes must not be able to
        // disagree about what lambda is.
        let err = parse_strict(br#"{"lambda": 0.0, "lambda": 1e9}"#).unwrap_err();
        assert!(err.contains("lambda"), "{err}");
        assert!(err.contains("more than once"), "{err}");

        // And what serde_json would have done, for the record.
        let lax: Value = serde_json::from_slice(br#"{"lambda": 0.0, "lambda": 1e9}"#).unwrap();
        assert_eq!(lax["lambda"], serde_json::json!(1e9));
    }

    #[test]
    fn a_repeated_key_is_refused_at_any_depth_and_inside_arrays() {
        for hostile in [
            br#"{"a": {"b": {"c": 1, "c": 2}}}"#.as_slice(),
            br#"{"a": [ {"x": 1, "x": 2} ]}"#.as_slice(),
            br#"{"a": [[ {"x": 1, "x": 2} ]]}"#.as_slice(),
        ] {
            let err = parse_strict(hostile).unwrap_err();
            assert!(
                err.contains("more than once"),
                "{:?} should be refused: {err}",
                std::str::from_utf8(hostile)
            );
        }
    }

    #[test]
    fn distinct_keys_and_repeated_values_still_parse() {
        // The rule is about keys, not values. Two keys with the same value are fine,
        // and so is the same key name at different levels of nesting.
        let value = parse_strict(br#"{"a": 1, "b": 1, "c": {"a": 1}}"#).unwrap();
        assert_eq!(value["a"], serde_json::json!(1));
        assert_eq!(value["c"]["a"], serde_json::json!(1));
    }

    #[test]
    fn a_document_with_trailing_content_is_refused() {
        // Two JSON values in one file is not one document, and guessing which one was
        // meant is not something a verifier gets to do.
        assert!(parse_strict(br#"{"a": 1} {"a": 2}"#).is_err());
        assert!(parse_strict(br#"{"a": 1} garbage"#).is_err());
        // Trailing whitespace is not trailing content.
        assert!(parse_strict(b"{\"a\": 1}\n  \n").is_ok());
    }

    #[test]
    fn strict_parsing_agrees_with_serde_on_everything_else() {
        // The strict parser must be a drop-in for the lax one on any document that has
        // no duplicate key, or it would be quietly changing what Ordinata can read.
        for text in [
            r#"{"a":[1,2.5,{"z":null,"b":true}],"s":"x\ny"}"#,
            r"[]",
            r"{}",
            r"null",
            r#"[{"deep":{"deeper":[1,{"deepest":"é"}]}}]"#,
            r#"{"big": 9007199254740993, "neg": -17, "exp": 1e-300}"#,
        ] {
            let strict = parse_strict(text.as_bytes()).unwrap();
            let lax: Value = serde_json::from_str(text).unwrap();
            assert_eq!(strict, lax, "{text}");
        }
    }

    #[test]
    fn sha256_matches_the_published_empty_string_digest() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

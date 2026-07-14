//! Identifier and free-text rules.
//!
//! Observation IDs and parameter names are held to a deliberately narrow character
//! set. That is not fussiness: it is the reason Ordinata's CSV output cannot carry a
//! spreadsheet formula and its HTML output cannot carry a tag. An identifier that
//! cannot begin with `=`, `+`, `-`, or `@`, and cannot contain `<`, `>`, `&`, or a
//! quote, is structurally incapable of becoming an injection. The defense is in the
//! grammar, not in a downstream escaping pass that someone might forget to apply.
//!
//! Free text (titles, assumptions, limitations, citation labels) is a different
//! matter: a limitation genuinely might need to say `x < 3`. Free text is therefore
//! accepted broadly, bounded in length, stripped of control characters, and escaped
//! at every point where it is rendered.

use crate::error::{InputError, Result};
use crate::spec::{MAX_IDENTIFIER_BYTES, MAX_TEXT_BYTES};

/// Check an observation ID or parameter name.
///
/// Returns the rule that was broken, for use in an [`InputError::InvalidIdentifier`].
pub(crate) fn identifier_rule_broken(value: &str) -> Option<&'static str> {
    if value.is_empty() {
        return Some("it is empty");
    }
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Some("it is longer than 64 bytes");
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Some("it is empty");
    };
    if !(first.is_ascii_alphanumeric() || first == '_') {
        return Some("it must start with an ASCII letter, digit, or underscore, which is what keeps it from ever being read as a spreadsheet formula");
    }
    for c in value.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-') {
            return Some("only ASCII letters, digits, underscore, dot, and hyphen are accepted");
        }
    }
    None
}

/// Render a value for an error message without letting a hostile input flood the
/// terminal or smuggle control characters into it.
pub(crate) fn for_message(value: &str) -> String {
    const MAX: usize = 48;
    let mut out = String::with_capacity(MAX + 8);
    for c in value.chars().take(MAX) {
        if c.is_control() {
            out.push('.');
        } else {
            out.push(c);
        }
    }
    if value.chars().count() > MAX {
        out.push_str("...");
    }
    out
}

/// Check a free-text field: bounded, valid UTF-8 by construction, and free of
/// control characters other than the ones a human would type.
///
/// # Errors
///
/// Returns [`InputError::InvalidField`] if the value is empty, too long, or carries
/// a control character.
pub(crate) fn check_free_text(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.trim().is_empty() {
        return Err(InputError::InvalidField {
            field: field.to_string(),
            detail: "must not be empty or only whitespace".into(),
        }
        .into());
    }
    if value.len() > max_bytes {
        return Err(InputError::InvalidField {
            field: field.to_string(),
            detail: format!(
                "is {} bytes, which exceeds the {max_bytes} byte limit",
                value.len()
            ),
        }
        .into());
    }
    if let Some(bad) = value.chars().find(|c| c.is_control()) {
        return Err(InputError::InvalidField {
            field: field.to_string(),
            detail: format!(
                "contains the control character U+{:04X}; free text may not carry control characters",
                bad as u32
            ),
        }
        .into());
    }
    Ok(())
}

/// Check an optional free-text field with the standard text limit.
///
/// # Errors
///
/// As [`check_free_text`].
pub(crate) fn check_optional_text(field: &str, value: Option<&String>) -> Result<()> {
    match value {
        Some(v) => check_free_text(field, v, MAX_TEXT_BYTES),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;

    #[test]
    fn accepted_identifiers() {
        for good in [
            "a",
            "x1",
            "_private",
            "rate.constant",
            "k-1",
            "Intercept",
            "A0.b-c_d",
            &"z".repeat(64),
        ] {
            assert!(
                identifier_rule_broken(good).is_none(),
                "{good:?} should be accepted"
            );
        }
    }

    #[test]
    fn formula_injection_cannot_be_expressed_as_an_identifier() {
        // These are the classic CSV formula payloads. None of them can name a
        // parameter or an observation, so no Ordinata CSV can ever contain one.
        for hostile in [
            "=1+1",
            "=cmd|' /C calc'!A0",
            "+1",
            "-1+1",
            "@SUM(A1)",
            "\tSUM",
            "\r=1",
        ] {
            assert!(
                identifier_rule_broken(hostile).is_some(),
                "{hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn html_injection_cannot_be_expressed_as_an_identifier() {
        for hostile in [
            "<script>",
            "a<b",
            "a&b",
            "a\"b",
            "a'b",
            "</td><td>",
            "javascript:x",
        ] {
            assert!(
                identifier_rule_broken(hostile).is_some(),
                "{hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn separators_and_padding_cannot_be_expressed_as_an_identifier() {
        for hostile in ["a,b", "a b", " a", "a\nb", "a;b", "", &"z".repeat(65)] {
            assert!(
                identifier_rule_broken(hostile).is_some(),
                "{hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn non_ascii_identifiers_are_refused_so_homoglyphs_cannot_collide() {
        // Two parameters whose names look identical but differ in code points would
        // be a silent trap in a result table.
        for hostile in ["\u{0430}", "e\u{301}", "\u{FF41}"] {
            assert!(
                identifier_rule_broken(hostile).is_some(),
                "{hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn free_text_accepts_prose_and_refuses_control_characters() {
        assert!(check_free_text("t", "Valid only for x < 3 & T > 0.", 200).is_ok());
        assert!(check_free_text("t", "", 200).is_err());
        assert!(check_free_text("t", "   ", 200).is_err());
        assert!(check_free_text("t", &"a".repeat(201), 200).is_err());
        assert!(check_free_text("t", "line\u{0}break", 200).is_err());
    }

    #[test]
    fn error_message_rendering_stays_bounded_and_printable() {
        let hostile = format!("{}\u{7}", "x".repeat(200));
        let rendered = for_message(&hostile);
        assert!(rendered.chars().count() <= 51, "{rendered}");
        assert!(!rendered.chars().any(char::is_control));
    }
}

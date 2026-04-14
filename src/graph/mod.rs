//! Structural Introspection — AI-oriented graph analysis and verification.
//!
//! `taida graph` outputs AI-oriented unified JSON for codebase comprehension.
//! `taida verify` runs structural verification checks on Taida source files.

pub mod ai_format;
pub mod verify;

// Internal modules used by verify — not part of the public API.
pub(crate) mod extract;
pub(crate) mod model;
pub(crate) mod query;
pub(crate) mod tail_pos;

/// Escape special characters for JSON strings (RFC 8259 compliant).
///
/// Per RFC 8259 section 7, the following characters MUST be escaped:
/// - `"` and `\` — with a reverse solidus prefix
/// - Control characters U+0000..U+001F — as `\uXXXX` or named escapes
pub(crate) fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if c.is_control() && (c as u32) < 0x20 => {
                // Remaining control characters (U+0000..U+001F) use \uXXXX
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_json_backslash_and_quote() {
        assert_eq!(escape_json(r#"a\b"c"#), r#"a\\b\"c"#);
    }

    #[test]
    fn test_escape_json_newline_tab_cr() {
        assert_eq!(escape_json("a\nb\tc\rd"), "a\\nb\\tc\\rd");
    }

    #[test]
    fn test_escape_json_backspace_formfeed() {
        assert_eq!(escape_json("a\x08b\x0Cc"), "a\\bb\\fc");
    }

    #[test]
    fn test_escape_json_null_and_low_control_chars() {
        // NUL (U+0000), SOH (U+0001), BEL (U+0007)
        assert_eq!(escape_json("\x00\x01\x07"), "\\u0000\\u0001\\u0007");
    }

    #[test]
    fn test_escape_json_all_control_chars_produce_valid_json() {
        // Build a string with every control character U+0000..U+001F
        let input: String = (0u8..0x20).map(|b| b as char).collect();
        let escaped = escape_json(&input);
        let json_str = format!("\"{}\"", escaped);
        let result: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
        assert!(
            result.is_ok(),
            "All control chars should produce valid JSON, got error: {:?}\nEscaped: {}",
            result.err(),
            escaped
        );
    }

    #[test]
    fn test_escape_json_ascii_passthrough() {
        assert_eq!(escape_json("hello world 123!@#"), "hello world 123!@#");
    }

    #[test]
    fn test_escape_json_multibyte_passthrough() {
        assert_eq!(escape_json("abc"), "abc");
    }
}

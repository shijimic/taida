//! UTF-16 conversion utilities for LSP protocol compliance.
//!
//! The LSP specification defines `Position.character` as a 0-based offset
//! measured in UTF-16 code units. Taida's lexer uses char-based (Unicode
//! scalar value) indexing internally. Characters outside the Basic
//! Multilingual Plane (e.g., emoji) occupy 2 UTF-16 code units but only
//! 1 Rust `char`, so a direct cast would misplace the cursor.
//!
//! This module provides bidirectional conversion between the two systems.

/// Convert a 0-based UTF-16 code unit offset to a 0-based char index
/// within `line_text`.
///
/// If the offset falls in the middle of a surrogate pair, the result is
/// clamped to the start of that character (i.e. the char whose surrogate
/// pair contains the offset).
pub fn utf16_offset_to_char_index(line_text: &str, utf16_offset: usize) -> usize {
    let mut utf16_cursor: usize = 0;
    let mut char_count: usize = 0;
    for (i, ch) in line_text.chars().enumerate() {
        let width = ch.len_utf16();
        if utf16_cursor + width > utf16_offset {
            // The offset falls within this character (at its first code
            // unit or, for a surrogate pair, at the second code unit).
            return i;
        }
        utf16_cursor += width;
        char_count = i + 1;
    }
    // offset at or beyond end of line -> return total char count
    char_count
}

/// Convert a 0-based char index to a 0-based UTF-16 code unit offset
/// within `line_text`.
///
/// If `char_index` exceeds the number of characters in the line the
/// returned offset equals the total UTF-16 length of the line.
pub fn char_index_to_utf16_offset(line_text: &str, char_index: usize) -> usize {
    line_text
        .chars()
        .take(char_index)
        .map(|ch| ch.len_utf16())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ASCII (1 char == 1 UTF-16 code unit) ──

    #[test]
    fn test_ascii_roundtrip() {
        let line = "hello world";
        for i in 0..line.len() {
            assert_eq!(utf16_offset_to_char_index(line, i), i);
            assert_eq!(char_index_to_utf16_offset(line, i), i);
        }
    }

    // ── BMP multi-byte (1 char == 1 UTF-16 code unit, but >1 byte in UTF-8) ──

    #[test]
    fn test_japanese_roundtrip() {
        // Each Hiragana char is 1 UTF-16 code unit but 3 UTF-8 bytes.
        let line = "x <= \"こんにちは\"";
        // char indices:  0  1  2  3  4  5  6  7  8  9  10  11  12
        // chars:         x     <  =     "  こ  ん  に  ち   は   "
        // UTF-16 offsets: same as char indices (all BMP)
        assert_eq!(utf16_offset_to_char_index(line, 6), 6); // '"' -> char 6
        assert_eq!(utf16_offset_to_char_index(line, 7), 7); // 'こ' -> char 7
        assert_eq!(char_index_to_utf16_offset(line, 7), 7);
    }

    // ── Astral plane (1 char == 2 UTF-16 code units) ──

    #[test]
    fn test_emoji_utf16_to_char() {
        // U+1F600 (GRINNING FACE) is 2 UTF-16 code units.
        let line = "a\u{1F600}b";
        // char indices: 0(a), 1(emoji), 2(b)
        // UTF-16 units: 0(a), 1-2(emoji surrogate pair), 3(b)
        assert_eq!(utf16_offset_to_char_index(line, 0), 0); // 'a'
        assert_eq!(utf16_offset_to_char_index(line, 1), 1); // start of emoji
        assert_eq!(utf16_offset_to_char_index(line, 2), 1); // mid-surrogate -> clamp to emoji
        assert_eq!(utf16_offset_to_char_index(line, 3), 2); // 'b'
    }

    #[test]
    fn test_emoji_char_to_utf16() {
        let line = "a\u{1F600}b";
        assert_eq!(char_index_to_utf16_offset(line, 0), 0); // before 'a'
        assert_eq!(char_index_to_utf16_offset(line, 1), 1); // after 'a', before emoji
        assert_eq!(char_index_to_utf16_offset(line, 2), 3); // after emoji, before 'b'
        assert_eq!(char_index_to_utf16_offset(line, 3), 4); // after 'b'
    }

    // ── Edge cases ──

    #[test]
    fn test_empty_line() {
        assert_eq!(utf16_offset_to_char_index("", 0), 0);
        assert_eq!(utf16_offset_to_char_index("", 5), 0);
        assert_eq!(char_index_to_utf16_offset("", 0), 0);
        assert_eq!(char_index_to_utf16_offset("", 5), 0);
    }

    #[test]
    fn test_offset_beyond_end() {
        let line = "abc";
        assert_eq!(utf16_offset_to_char_index(line, 10), 3);
        assert_eq!(char_index_to_utf16_offset(line, 10), 3);
    }

    #[test]
    fn test_mixed_bmp_and_astral() {
        // "こんに\u{1F600}は"
        // char indices: 0(こ), 1(ん), 2(に), 3(emoji), 4(は)
        // UTF-16:       0(こ), 1(ん), 2(に), 3-4(emoji), 5(は)
        let line = "こんに\u{1F600}は";
        assert_eq!(utf16_offset_to_char_index(line, 3), 3); // emoji start
        assert_eq!(utf16_offset_to_char_index(line, 5), 4); // 'は'
        assert_eq!(char_index_to_utf16_offset(line, 3), 3); // before emoji
        assert_eq!(char_index_to_utf16_offset(line, 4), 5); // after emoji
    }
}

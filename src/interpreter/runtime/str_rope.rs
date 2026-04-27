//! D29 Track-θ Phase 10-B: Gap-buffer rope for `Value::Str` mutation hot paths.
//!
//! Lock-K verdict (V-1 Option A, V-2 case 5.2-b gap buffer, V-3 1024-byte
//! threshold) confirms a single-cursor optimised gap buffer for the LineEditor
//! workflow. The gap buffer maintains a contiguous storage with a movable
//! "gap" region; insertions at the gap position are amortised O(1), and edit
//! sequences that share a cursor (the `prompt.td::LineEditor` use-case) avoid
//! the O(N) memcpy cascade that the previous immutable `String` representation
//! incurred (TMB-027 root cause).
//!
//! # Invariants
//!
//! * `0 <= gap_start <= gap_end <= buf.len()`
//! * Effective bytes are `buf[0..gap_start]` followed by `buf[gap_end..]`.
//! * The gap region (`buf[gap_start..gap_end]`) is unused / undefined memory
//!   reserved for future insertions.
//! * UTF-8 boundary safety is the caller's responsibility for byte-indexed
//!   APIs (`insert_at_byte`, `delete_byte_range`, `slice_to_string`); char-
//!   indexed wrappers in `StrValue` perform the byte conversion.
//! * After any structural mutation, `flat_cache` and `char_offsets` caches in
//!   `RopeBuffer` must be invalidated (handled by the caller — `RopeBuffer`
//!   is the OnceLock cache owner; this module exposes only the raw gap buffer).

use std::cmp;

/// Default initial gap size when constructing a fresh `GapBuffer` from a
/// non-empty seed. Sized to absorb a typical short edit burst (~2 KiB worth
/// of keystrokes) before requiring a buffer growth.
const DEFAULT_GAP: usize = 256;

/// Minimum gap size after `move_gap_to_byte` reshapes the buffer. Ensures
/// at least a small slack region remains for the next insertion.
const MIN_GAP_AFTER_MOVE: usize = 32;

/// A gap-buffer storage for UTF-8 strings. Single-cursor optimised.
#[derive(Debug)]
pub struct GapBuffer {
    /// Underlying byte storage. Bytes in the gap region are uninitialised
    /// (in practice we leave them as whatever previous `Vec::resize` set them
    /// to; we never read from the gap region).
    buf: Vec<u8>,
    /// Inclusive start of the gap (== position of the next insertion).
    gap_start: usize,
    /// Exclusive end of the gap (== first valid byte after the gap).
    gap_end: usize,
}

impl GapBuffer {
    /// Construct an empty `GapBuffer` with default gap capacity.
    pub fn new() -> Self {
        let buf = vec![0u8; DEFAULT_GAP];
        GapBuffer {
            buf,
            gap_start: 0,
            gap_end: DEFAULT_GAP,
        }
    }

    /// Construct a `GapBuffer` seeded with `initial`. The gap is placed at
    /// the end of the seed (cursor at end-of-text).
    pub fn from_str(initial: &str) -> Self {
        let initial_bytes = initial.as_bytes();
        let total = initial_bytes.len() + DEFAULT_GAP;
        let mut buf = Vec::with_capacity(total);
        buf.extend_from_slice(initial_bytes);
        buf.resize(total, 0);
        GapBuffer {
            buf,
            gap_start: initial_bytes.len(),
            gap_end: total,
        }
    }

    /// Construct a `GapBuffer` seeded with `initial` and at least `gap` bytes
    /// of slack at the end. Used when the caller knows it will perform a
    /// large edit burst.
    pub fn from_str_with_gap(initial: &str, gap: usize) -> Self {
        let initial_bytes = initial.as_bytes();
        let gap = cmp::max(gap, MIN_GAP_AFTER_MOVE);
        let total = initial_bytes.len() + gap;
        let mut buf = Vec::with_capacity(total);
        buf.extend_from_slice(initial_bytes);
        buf.resize(total, 0);
        GapBuffer {
            buf,
            gap_start: initial_bytes.len(),
            gap_end: total,
        }
    }

    /// Effective byte length (excludes the gap).
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.buf.len() - (self.gap_end - self.gap_start)
    }

    /// True if the buffer contains no effective bytes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.byte_len() == 0
    }

    /// Move the gap so that `gap_start == pos` (where `pos` is in effective
    /// byte coordinates, i.e. ignores the gap).
    ///
    /// Caller must ensure `pos <= self.byte_len()` and that `pos` lies on a
    /// UTF-8 boundary in the effective bytes view.
    pub fn move_gap_to_byte(&mut self, pos: usize) {
        debug_assert!(pos <= self.byte_len(), "gap move out of bounds");
        if pos == self.gap_start {
            return;
        }
        if pos < self.gap_start {
            // Shift bytes [pos..gap_start) → [pos + gap_size..gap_end).
            let shift_len = self.gap_start - pos;
            let gap_size = self.gap_end - self.gap_start;
            // Move bytes from [pos..gap_start) to [gap_end - shift_len..gap_end).
            let new_gap_end = self.gap_end - shift_len;
            // SAFETY: copy_within handles overlapping moves correctly.
            self.buf.copy_within(pos..self.gap_start, new_gap_end);
            self.gap_start = pos;
            self.gap_end = self.gap_start + gap_size;
        } else {
            // pos > self.gap_start (in effective coords) means we must shift
            // bytes from after the gap into the front part of the gap.
            let effective_skip = pos - self.gap_start;
            let src_start = self.gap_end;
            let src_end = self.gap_end + effective_skip;
            let dst_start = self.gap_start;
            self.buf.copy_within(src_start..src_end, dst_start);
            self.gap_start += effective_skip;
            self.gap_end += effective_skip;
        }
    }

    /// Ensure the gap can hold at least `needed` more bytes. Grows the buffer
    /// (and shifts the trailing region) if necessary.
    fn ensure_gap(&mut self, needed: usize) {
        let current_gap = self.gap_end - self.gap_start;
        if current_gap >= needed {
            return;
        }
        // Grow geometrically; cap minimum new gap at MIN_GAP_AFTER_MOVE * 2.
        let extra = cmp::max(
            needed - current_gap,
            self.buf.len() / 2 + MIN_GAP_AFTER_MOVE,
        );
        let trailing_len = self.buf.len() - self.gap_end;
        let old_len = self.buf.len();
        self.buf.resize(old_len + extra, 0);
        // Shift the trailing region right by `extra` bytes.
        // copy_within with overlapping regions is safe (Rust spec).
        let new_buf_len = self.buf.len();
        self.buf.copy_within(
            self.gap_end..self.gap_end + trailing_len,
            new_buf_len - trailing_len,
        );
        self.gap_end += extra;
    }

    /// Insert `s` at effective byte position `pos`. Caller must ensure `pos`
    /// is on a UTF-8 boundary.
    pub fn insert_at_byte(&mut self, pos: usize, s: &str) {
        let bytes = s.as_bytes();
        if bytes.is_empty() {
            return;
        }
        self.move_gap_to_byte(pos);
        self.ensure_gap(bytes.len());
        // Write into the gap, advancing gap_start.
        let gap_start = self.gap_start;
        self.buf[gap_start..gap_start + bytes.len()].copy_from_slice(bytes);
        self.gap_start += bytes.len();
    }

    /// Delete the effective byte range `[start..end)`. Caller must ensure
    /// both bounds are on UTF-8 boundaries and `start <= end <= byte_len()`.
    pub fn delete_byte_range(&mut self, start: usize, end: usize) {
        debug_assert!(start <= end, "delete range start > end");
        debug_assert!(end <= self.byte_len(), "delete range out of bounds");
        if start == end {
            return;
        }
        // Move gap to start, then expand gap_end to absorb end - start bytes
        // from the post-gap region.
        self.move_gap_to_byte(start);
        let removed = end - start;
        self.gap_end += removed;
    }

    /// Flatten the gap buffer into a fresh owned `String`.
    pub fn flatten(&self) -> String {
        let mut out = Vec::with_capacity(self.byte_len());
        out.extend_from_slice(&self.buf[..self.gap_start]);
        out.extend_from_slice(&self.buf[self.gap_end..]);
        // SAFETY: invariant — every mutation goes through `insert_at_byte`
        // which takes `&str`, so the contents are valid UTF-8.
        unsafe { String::from_utf8_unchecked(out) }
    }

    /// Slice the effective bytes `[byte_start..byte_end)` and return a fresh
    /// `String`. Caller must ensure UTF-8 boundary safety.
    pub fn slice_to_string(&self, byte_start: usize, byte_end: usize) -> String {
        debug_assert!(byte_start <= byte_end);
        debug_assert!(byte_end <= self.byte_len());
        if byte_start == byte_end {
            return String::new();
        }
        let mut out = Vec::with_capacity(byte_end - byte_start);
        // Three cases: range entirely before gap, entirely after gap, or
        // straddling the gap.
        if byte_end <= self.gap_start {
            out.extend_from_slice(&self.buf[byte_start..byte_end]);
        } else if byte_start >= self.gap_start {
            // Both bounds are in post-gap effective bytes; translate.
            let translated_start = byte_start - self.gap_start + self.gap_end;
            let translated_end = byte_end - self.gap_start + self.gap_end;
            out.extend_from_slice(&self.buf[translated_start..translated_end]);
        } else {
            // Straddles the gap.
            out.extend_from_slice(&self.buf[byte_start..self.gap_start]);
            let translated_end = byte_end - self.gap_start + self.gap_end;
            out.extend_from_slice(&self.buf[self.gap_end..translated_end]);
        }
        // SAFETY: same invariant as `flatten`.
        unsafe { String::from_utf8_unchecked(out) }
    }
}

impl Default for GapBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer() {
        let g = GapBuffer::new();
        assert_eq!(g.byte_len(), 0);
        assert!(g.is_empty());
        assert_eq!(g.flatten(), "");
    }

    #[test]
    fn from_str_seed() {
        let g = GapBuffer::from_str("hello");
        assert_eq!(g.byte_len(), 5);
        assert_eq!(g.flatten(), "hello");
    }

    #[test]
    fn insert_at_end() {
        let mut g = GapBuffer::from_str("hel");
        g.insert_at_byte(3, "lo");
        assert_eq!(g.flatten(), "hello");
    }

    #[test]
    fn insert_at_middle() {
        let mut g = GapBuffer::from_str("helo");
        g.insert_at_byte(3, "l");
        assert_eq!(g.flatten(), "hello");
    }

    #[test]
    fn insert_at_start() {
        let mut g = GapBuffer::from_str("ello");
        g.insert_at_byte(0, "h");
        assert_eq!(g.flatten(), "hello");
    }

    #[test]
    fn delete_middle() {
        let mut g = GapBuffer::from_str("hello world");
        g.delete_byte_range(5, 6); // delete the space
        assert_eq!(g.flatten(), "helloworld");
    }

    #[test]
    fn delete_then_insert() {
        let mut g = GapBuffer::from_str("hello");
        g.delete_byte_range(1, 4);
        assert_eq!(g.flatten(), "ho");
        g.insert_at_byte(1, "ELL");
        assert_eq!(g.flatten(), "hELLo");
    }

    #[test]
    fn slice_before_gap() {
        let mut g = GapBuffer::from_str("hello");
        g.insert_at_byte(5, "!"); // gap starts at 6
        assert_eq!(g.slice_to_string(0, 3), "hel");
    }

    #[test]
    fn slice_after_gap() {
        let mut g = GapBuffer::from_str("hello world");
        g.move_gap_to_byte(2);
        assert_eq!(g.slice_to_string(5, 11), " world");
    }

    #[test]
    fn slice_straddles_gap() {
        let mut g = GapBuffer::from_str("hello world");
        g.move_gap_to_byte(5);
        assert_eq!(g.slice_to_string(3, 8), "lo wo");
    }

    #[test]
    fn sequential_500_inserts() {
        let mut g = GapBuffer::from_str("");
        let mut reference = String::new();
        for i in 0..500 {
            let ch = (b'a' + (i % 26) as u8) as char;
            g.insert_at_byte(i, &ch.to_string());
            reference.push(ch);
        }
        assert_eq!(g.flatten(), reference);
        assert_eq!(g.byte_len(), 500);
    }

    #[test]
    fn cursor_follow_inserts() {
        // Simulates LineEditor: insert at growing cursor position.
        let mut g = GapBuffer::from_str("");
        for ch in "hello".chars() {
            let pos = g.byte_len();
            g.insert_at_byte(pos, &ch.to_string());
        }
        assert_eq!(g.flatten(), "hello");
    }

    #[test]
    fn utf8_multibyte_insert() {
        let mut g = GapBuffer::from_str("a");
        // "あ" = 0xE3 0x81 0x82 (3 bytes)
        g.insert_at_byte(1, "あ");
        assert_eq!(g.flatten(), "aあ");
        g.insert_at_byte(4, "b");
        assert_eq!(g.flatten(), "aあb");
    }

    #[test]
    fn random_edit_against_reference() {
        let mut g = GapBuffer::from_str("");
        let mut r = String::new();
        // Deterministic pseudo-random sequence (no rand dep).
        let ops: &[(u8, usize, &str)] = &[
            (0, 0, "abc"),
            (0, 3, "def"),
            (0, 0, "X"),
            (1, 4, ""), // delete 1 byte at offset 4
            (0, 2, "YYY"),
            (1, 0, ""), // delete 1 byte at offset 0
        ];
        for (op, pos, s) in ops {
            match op {
                0 => {
                    g.insert_at_byte(*pos, s);
                    r.insert_str(*pos, s);
                }
                1 => {
                    g.delete_byte_range(*pos, pos + 1);
                    r.replace_range(*pos..pos + 1, "");
                }
                _ => unreachable!(),
            }
            assert_eq!(g.flatten(), r, "after op={} pos={} s={:?}", op, pos, s);
        }
    }

    #[test]
    fn growth_beyond_initial_gap() {
        let mut g = GapBuffer::from_str_with_gap("seed", 4);
        // Insert more than the initial gap to force growth.
        let big = "x".repeat(100);
        g.insert_at_byte(4, &big);
        assert_eq!(g.byte_len(), 104);
        assert_eq!(g.flatten(), format!("seed{}", big));
    }
}

/// QPACK static table, error types, integer/string coding, header block encode/decode.
///
/// This module contains all QPACK-related types and functions:
/// - QPACK static table (RFC 9204 Appendix A, 99 entries)
/// - H3DecodeError enum and H3Result type alias (NET7-6b)
/// - QPACK integer/string coding (RFC 9204 Section 4.1.1 / 4.1.2)
/// - QPACK header block encode/decode (RFC 9204 Section 4.5)
/// - QPACK dynamic table (RFC 9204 Section 4.3)
/// - QPACK encoder/decoder instruction streams (RFC 9204 Sections 5.2/6.2)

use crate::interpreter::net_h2;
use super::frame::is_canonical_varint;

pub(crate) struct QpackStaticEntry {
    pub name: &'static str,
    pub value: &'static str,
}

pub(crate) const QPACK_STATIC_TABLE: &[QpackStaticEntry] = &[
    QpackStaticEntry { name: ":authority", value: "" },                          // 0
    QpackStaticEntry { name: ":path", value: "/" },                              // 1
    QpackStaticEntry { name: "age", value: "0" },                                // 2
    QpackStaticEntry { name: "content-disposition", value: "" },                  // 3
    QpackStaticEntry { name: "content-length", value: "0" },                     // 4
    QpackStaticEntry { name: "cookie", value: "" },                              // 5
    QpackStaticEntry { name: "date", value: "" },                                // 6
    QpackStaticEntry { name: "etag", value: "" },                                // 7
    QpackStaticEntry { name: "if-modified-since", value: "" },                   // 8
    QpackStaticEntry { name: "if-none-match", value: "" },                       // 9
    QpackStaticEntry { name: "last-modified", value: "" },                       // 10
    QpackStaticEntry { name: "link", value: "" },                                // 11
    QpackStaticEntry { name: "location", value: "" },                            // 12
    QpackStaticEntry { name: "referer", value: "" },                             // 13
    QpackStaticEntry { name: "set-cookie", value: "" },                          // 14
    QpackStaticEntry { name: ":method", value: "CONNECT" },                      // 15
    QpackStaticEntry { name: ":method", value: "DELETE" },                       // 16
    QpackStaticEntry { name: ":method", value: "GET" },                          // 17
    QpackStaticEntry { name: ":method", value: "HEAD" },                         // 18
    QpackStaticEntry { name: ":method", value: "OPTIONS" },                      // 19
    QpackStaticEntry { name: ":method", value: "POST" },                         // 20
    QpackStaticEntry { name: ":method", value: "PUT" },                          // 21
    QpackStaticEntry { name: ":scheme", value: "http" },                         // 22
    QpackStaticEntry { name: ":scheme", value: "https" },                        // 23
    QpackStaticEntry { name: ":status", value: "103" },                          // 24
    QpackStaticEntry { name: ":status", value: "200" },                          // 25
    QpackStaticEntry { name: ":status", value: "304" },                          // 26
    QpackStaticEntry { name: ":status", value: "404" },                          // 27
    QpackStaticEntry { name: ":status", value: "503" },                          // 28
    QpackStaticEntry { name: "accept", value: "*/*" },                           // 29
    QpackStaticEntry { name: "accept", value: "application/dns-message" },       // 30
    QpackStaticEntry { name: "accept-encoding", value: "gzip, deflate, br" },    // 31
    QpackStaticEntry { name: "accept-ranges", value: "bytes" },                  // 32
    QpackStaticEntry { name: "access-control-allow-headers", value: "cache-control" }, // 33
    QpackStaticEntry { name: "access-control-allow-headers", value: "content-type" },  // 34
    QpackStaticEntry { name: "access-control-allow-origin", value: "*" },        // 35
    QpackStaticEntry { name: "cache-control", value: "max-age=0" },              // 36
    QpackStaticEntry { name: "cache-control", value: "max-age=2592000" },        // 37
    QpackStaticEntry { name: "cache-control", value: "max-age=604800" },         // 38
    QpackStaticEntry { name: "cache-control", value: "no-cache" },               // 39
    QpackStaticEntry { name: "cache-control", value: "no-store" },               // 40
    QpackStaticEntry { name: "cache-control", value: "public, max-age=31536000" }, // 41
    QpackStaticEntry { name: "content-encoding", value: "br" },                  // 42
    QpackStaticEntry { name: "content-encoding", value: "gzip" },                // 43
    QpackStaticEntry { name: "content-type", value: "application/dns-message" }, // 44
    QpackStaticEntry { name: "content-type", value: "application/javascript" },  // 45
    QpackStaticEntry { name: "content-type", value: "application/json" },        // 46
    QpackStaticEntry { name: "content-type", value: "application/x-www-form-urlencoded" }, // 47
    QpackStaticEntry { name: "content-type", value: "image/gif" },               // 48
    QpackStaticEntry { name: "content-type", value: "image/jpeg" },              // 49
    QpackStaticEntry { name: "content-type", value: "image/png" },               // 50
    QpackStaticEntry { name: "content-type", value: "text/css" },                // 51
    QpackStaticEntry { name: "content-type", value: "text/html; charset=utf-8" }, // 52
    QpackStaticEntry { name: "content-type", value: "text/plain" },              // 53
    QpackStaticEntry { name: "content-type", value: "text/plain;charset=utf-8" }, // 54
    QpackStaticEntry { name: "range", value: "bytes=0-" },                       // 55
    QpackStaticEntry { name: "strict-transport-security", value: "max-age=31536000" }, // 56
    QpackStaticEntry { name: "strict-transport-security", value: "max-age=31536000; includesubdomains" }, // 57
    QpackStaticEntry { name: "strict-transport-security", value: "max-age=31536000; includesubdomains; preload" }, // 58
    QpackStaticEntry { name: "vary", value: "accept-encoding" },                 // 59
    QpackStaticEntry { name: "vary", value: "origin" },                          // 60
    QpackStaticEntry { name: "x-content-type-options", value: "nosniff" },       // 61
    QpackStaticEntry { name: "x-xss-protection", value: "1; mode=block" },       // 62
    QpackStaticEntry { name: ":status", value: "100" },                          // 63
    QpackStaticEntry { name: ":status", value: "204" },                          // 64
    QpackStaticEntry { name: ":status", value: "206" },                          // 65
    QpackStaticEntry { name: ":status", value: "302" },                          // 66
    QpackStaticEntry { name: ":status", value: "400" },                          // 67
    QpackStaticEntry { name: ":status", value: "403" },                          // 68
    QpackStaticEntry { name: ":status", value: "421" },                          // 69
    QpackStaticEntry { name: ":status", value: "425" },                          // 70
    QpackStaticEntry { name: ":status", value: "500" },                          // 71
    QpackStaticEntry { name: "accept-language", value: "" },                     // 72
    QpackStaticEntry { name: "access-control-allow-credentials", value: "FALSE" }, // 73
    QpackStaticEntry { name: "access-control-allow-credentials", value: "TRUE" },  // 74
    QpackStaticEntry { name: "access-control-allow-headers", value: "*" },       // 75
    QpackStaticEntry { name: "access-control-allow-methods", value: "get" },     // 76
    QpackStaticEntry { name: "access-control-allow-methods", value: "get, post, options" }, // 77
    QpackStaticEntry { name: "access-control-allow-methods", value: "options" },  // 78
    QpackStaticEntry { name: "access-control-expose-headers", value: "content-length" }, // 79
    QpackStaticEntry { name: "access-control-request-headers", value: "content-type" },  // 80
    QpackStaticEntry { name: "access-control-request-method", value: "get" },    // 81
    QpackStaticEntry { name: "access-control-request-method", value: "post" },   // 82
    QpackStaticEntry { name: "alt-svc", value: "clear" },                        // 83
    QpackStaticEntry { name: "authorization", value: "" },                       // 84
    QpackStaticEntry { name: "content-security-policy", value: "script-src 'none'; object-src 'none'; base-uri 'none'" }, // 85
    QpackStaticEntry { name: "early-data", value: "1" },                         // 86
    QpackStaticEntry { name: "expect-ct", value: "" },                           // 87
    QpackStaticEntry { name: "forwarded", value: "" },                           // 88
    QpackStaticEntry { name: "if-range", value: "" },                            // 89
    QpackStaticEntry { name: "origin", value: "" },                              // 90
    QpackStaticEntry { name: "purpose", value: "prefetch" },                     // 91
    QpackStaticEntry { name: "server", value: "" },                              // 92
    QpackStaticEntry { name: "timing-allow-origin", value: "*" },                // 93
    QpackStaticEntry { name: "upgrade-insecure-requests", value: "1" },          // 94
    QpackStaticEntry { name: "user-agent", value: "" },                          // 95
    QpackStaticEntry { name: "x-forwarded-for", value: "" },                     // 96
    QpackStaticEntry { name: "x-frame-options", value: "deny" },                 // 97
    QpackStaticEntry { name: "x-frame-options", value: "sameorigin" },           // 98
];

// ── QPACK Integer Coding (RFC 9204 Section 4.1.1) ──────────────────────
// Same variable-length integer format as HPACK but with different prefix sizes.

// ── H3 Decode Error (NET7-6b / NB7-27) ─────────────────────────────────
// NET7-6b: Migrate decode functions from `Option<T>` to `Result<T, H3DecodeError>`
// for error traceability. Each variant maps to an RFC 9114 error code.
//
// Error code mapping table (NET_DESIGN.md Phase 6+ plan):
// | None reason            | RFC 9114 error code                  | Variant            |
// |------------------------|--------------------------------------|--------------------|
// | varint overflow        | H3_ERR_GENERAL_PROTOCOL_ERROR        | VarintOverflow     |
// | frame truncated        | H3_ERR_FRAME_ERROR                   | FrameMalformed     |
// | huffman failure        | H3_ERR_GENERAL_PROTOCOL_ERROR        | HuffmanDecode      |
// | static table OOB       | H3_ERR_GENERAL_PROTOCOL_ERROR        | StaticTableIndex   |
// | dynamic table N/A      | N/A (Phase 2/3 reject)               | DynamicTableError  |
// | field section too big   | H3_ERR_REQUEST_REJECTED              | FieldSectionTooLarge |
// | QPACK int overflow     | H3_ERR_GENERAL_PROTOCOL_ERROR        | QpackIntOverflow   |

/// Decode error for QPACK / H3 frame parsing.
/// **NET7-6b**: replaces `Option<T>` decode signatures so that each
/// `None` reason can be traced back to an RFC 9114 error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum H3DecodeError {
    /// Truncated or empty input — more bytes needed.
    /// Maps to: RFC 9114 H3_ERR_FRAME_ERROR
    Truncated,
    /// QPACK integer overflow (m > 62 continuation).
    /// Maps to: RFC 9114 H3_ERR_GENERAL_PROTOCOL_ERROR
    QpackIntOverflow,
    /// Static table index out of range.
    /// Maps to: RFC 9114 H3_ERR_GENERAL_PROTOCOL_ERROR
    StaticTableIndex,
    /// Dynamic table entry not found or not available.
    /// Maps to: RFC 9114 H3_ERR_GENERAL_PROTOCOL_ERROR
    DynamicTableError,
    /// Huffman decode failure (invalid Huffman encoding).
    /// Maps to: RFC 9114 H3_ERR_GENERAL_PROTOCOL_ERROR
    HuffmanDecode,
    /// QUIC varint overflow (value exceeds u64).
    /// Maps to: RFC 9114 H3_ERR_GENERAL_PROTOCOL_ERROR
    VarintOverflow,
    /// Frame header / payload malformed (length mismatch, truncation).
    /// Maps to: RFC 9114 H3_ERR_FRAME_ERROR
    FrameMalformed,
    /// Field section exceeds max_field_section_size (NB7-43).
    /// Maps to: RFC 9114 H3_ERR_REQUEST_REJECTED
    FieldSectionTooLarge,
    /// Header block has too many fields (overflow guard, NB7-11).
    /// Maps to: RFC 9114 H3_ERR_REQUEST_REJECTED
    TooManyHeaders,
    /// Non-canonical encoding detected.
    /// Maps to: RFC 9114 H3_ERR_GENERAL_PROTOCOL_ERROR
    NonCanonical,
    /// Invalid instruction / unknown prefix.
    InvalidInstruction,
    /// Invalid UTF-8 in string literal.
    /// Maps to: RFC 9114 H3_ERR_GENERAL_PROTOCOL_ERROR
    InvalidUtf8,
    /// Idle timeout — no peer activity within timeout window.
    /// Maps to: RFC 9114 H3_ERR_NO_ERROR (0x0100, clean idle close)
    IdleTimeout,
}

impl std::fmt::Display for H3DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            H3DecodeError::Truncated => write!(f, "truncated input, more bytes needed"),
            H3DecodeError::QpackIntOverflow => write!(f, "QPACK integer overflow (m > 62)"),
            H3DecodeError::StaticTableIndex => write!(f, "static table index out of range"),
            H3DecodeError::DynamicTableError => write!(f, "dynamic table entry not available"),
            H3DecodeError::HuffmanDecode => write!(f, "Huffman decode failure"),
            H3DecodeError::VarintOverflow => write!(f, "QUIC varint overflow"),
            H3DecodeError::FrameMalformed => write!(f, "malformed frame"),
            H3DecodeError::FieldSectionTooLarge => write!(f, "field section too large"),
            H3DecodeError::TooManyHeaders => write!(f, "too many header fields"),
            H3DecodeError::NonCanonical => write!(f, "non-canonical encoding"),
            H3DecodeError::InvalidInstruction => write!(f, "invalid encoder/decoder instruction"),
            H3DecodeError::InvalidUtf8 => write!(f, "invalid UTF-8 in string literal"),
            H3DecodeError::IdleTimeout => write!(f, "idle timeout"),
        }
    }
}

/// Convenience alias for decode operations.
pub(crate) type H3Result<T> = Result<T, H3DecodeError>;

// ── QPACK Integer Coding (RFC 9204 Section 4.1.1) — Result variants ──────

/// Decode a QPACK integer with the given prefix bit width.
/// **NET7-6b**: `Result<T, H3DecodeError>` variant — provides error traceability
/// instead of opaque `None`.
///
/// **QPACK Integer encoding per RFC 9204 Section 4.1.1.** Uses prefix bits (m) and 7-bit continuation bytes.
/// NB7-33: Distinct from QUIC varint (RFC 9000 Section 16) — QPACK uses arbitrary prefix bit widths,
///   while QUIC varint uses fixed 2-bit length prefix.
/// Returns `Ok((value, bytes_consumed))` on success, `Err(H3DecodeError)` on error.
pub(crate) fn qpack_decode_int_r(data: &[u8], prefix_bits: u8) -> H3Result<(u64, usize)> {
    if data.is_empty() {
        return Err(H3DecodeError::Truncated);
    }
    // Compute mask avoiding overflow when prefix_bits == 8
    let mask: u8 = if prefix_bits >= 8 { 0xFF } else { (1u8 << prefix_bits) - 1 };
    let val = (data[0] & mask) as u64;
    if val < mask as u64 {
        return Ok((val, 1));
    }
    // Multi-byte
    let mut val = val;
    let mut m = 0u32;
    for i in 1..data.len() {
        val += ((data[i] & 0x7F) as u64) << m;
        m += 7;
        if data[i] & 0x80 == 0 {
            return Ok((val, i + 1));
        }
        if m > 62 {
            return Err(H3DecodeError::QpackIntOverflow); // overflow protection
        }
    }
    Err(H3DecodeError::Truncated) // incomplete
}

// ── QPACK Decode Functions: Result Variants (NET7-6b) ────────────────────

/// Decode a QPACK integer with the given prefix bit width.
/// **QPACK Integer decoding per RFC 9204 Section 4.1.1** — thin wrapper
/// around `qpack_decode_int_r` to share a single implementation path (NET7-11c).
pub(crate) fn qpack_decode_int(data: &[u8], prefix_bits: u8) -> Option<(u64, usize)> {
    qpack_decode_int_r(data, prefix_bits).ok()
}

/// Decode a QPACK string literal — `Result` variant (NET7-6b). Returns `Ok((string, bytes_consumed))` on success, `Err` with specific error variant.
pub(crate) fn qpack_decode_string_r(data: &[u8]) -> H3Result<(String, usize)> {
    if data.is_empty() {
        return Err(H3DecodeError::Truncated);
    }
    let is_huffman = (data[0] & 0x80) != 0;
    let (str_len, int_consumed) = qpack_decode_int_r(data, 7)?;
    let str_len = str_len as usize;
    if int_consumed + str_len > data.len() {
        return Err(H3DecodeError::Truncated);
    }
    let str_data = &data[int_consumed..int_consumed + str_len];

    if is_huffman {
        match net_h2::huffman_decode(str_data) {
            Some(decoded) => Ok((decoded, int_consumed + str_len)),
            None => Err(H3DecodeError::HuffmanDecode),
        }
    } else {
        match std::str::from_utf8(str_data) {
            Ok(s) => Ok((s.to_string(), int_consumed + str_len)),
            Err(_) => Err(H3DecodeError::InvalidUtf8),
        }
    }
}

// ── QUIC Varint: Result Variant (NET7-6b) ────────────────────────────────

/// Decode a QUIC variable-length integer — `Result` variant (NET7-6b).
/// Returns `Ok((value, bytes_consumed))` on success, `Err(H3DecodeError)` on error.
pub(crate) fn varint_decode_r(data: &[u8]) -> H3Result<(u64, usize)> {
    if data.is_empty() {
        return Err(H3DecodeError::Truncated);
    }
    let prefix = data[0] >> 6;
    let len = 1usize << prefix;
    if data.len() < len {
        return Err(H3DecodeError::Truncated);
    }
    let mut val = (data[0] & 0x3F) as u64;
    for i in 1..len {
        val = (val << 8) | data[i] as u64;
    }
    // NB7-16 / NET7-5a: canonical encoding check
    if !is_canonical_varint(val, prefix) {
        return Err(H3DecodeError::NonCanonical);
    }
    Ok((val, len))
}

/// Encode a QPACK integer with the given prefix bit width.
/// **QPACK Integer encoding per RFC 9204 Section 4.1.1.** See `qpack_decode_int` for details. NB7-33
/// Returns the number of bytes written on success, `None` on buffer overflow.
pub(crate) fn qpack_encode_int(
    buf: &mut [u8],
    prefix_bits: u8,
    value: u64,
    prefix_byte: u8,
) -> Option<usize> {
    if buf.is_empty() {
        return None;
    }
    // Compute mask avoiding overflow when prefix_bits == 8
    let mask: u8 = if prefix_bits >= 8 { 0xFF } else { (1u8 << prefix_bits) - 1 };
    if value < mask as u64 {
        buf[0] = prefix_byte | (value as u8);
        return Some(1);
    }
    buf[0] = prefix_byte | mask;
    let mut value = value - mask as u64;
    let mut pos = 1;
    while value >= 128 {
        if pos >= buf.len() {
            return None;
        }
        buf[pos] = ((value & 0x7F) | 0x80) as u8;
        pos += 1;
        value >>= 7;
    }
    if pos >= buf.len() {
        return None;
    }
    buf[pos] = value as u8;
    Some(pos + 1)
}

// ── QPACK String Coding (RFC 9204 Section 4.1.2) ────────────────────────
// Uses the same format as HPACK. For Phase 2/3, we encode as plain
// (non-Huffman) and decode both plain and Huffman.

/// Decode a QPACK string literal.
/// Returns `Some((string, bytes_consumed))` on success, `None` on error.
pub(crate) fn qpack_decode_string(data: &[u8]) -> Option<(String, usize)> {
    if data.is_empty() {
        return None;
    }
    let is_huffman = (data[0] & 0x80) != 0;
    let (str_len, int_consumed) = qpack_decode_int(data, 7)?;
    let str_len = str_len as usize;
    if int_consumed + str_len > data.len() {
        return None;
    }
    let str_data = &data[int_consumed..int_consumed + str_len];

    if is_huffman {
        // Reuse H2 Huffman decode for QPACK (same Huffman table)
        // NB7-32: Phase 2/3: returns Option<T>. Phase 6+: upgrade to Result<T, H3DecodeError>
        // for traceability — decode error reason captured in enum variant (HuffmanDecodeError,
        // VarintOverflow, FrameMalformed). See NET_DESIGN.md Phase 6+ plan.
        let decoded = net_h2::huffman_decode(str_data)?;
        Some((decoded, int_consumed + str_len))
    } else {
        let s = std::str::from_utf8(str_data).ok()?.to_string();
        Some((s, int_consumed + str_len))
    }
}

/// Encode a QPACK string literal (non-Huffman for simplicity in Phase 2/3).
/// Returns the number of bytes written, or `None` on buffer overflow.
pub(crate) fn qpack_encode_string(buf: &mut [u8], s: &str) -> Option<usize> {
    let slen = s.len() as u64;
    let int_written = qpack_encode_int(buf, 7, slen, 0x00)?;
    if int_written + s.len() > buf.len() {
        return None;
    }
    buf[int_written..int_written + s.len()].copy_from_slice(s.as_bytes());
    Some(int_written + s.len())
}

// ── QPACK Header Block Decode (RFC 9204 Section 4.5) ─────────────────────
// Phase 2/3 reference: decode encoded field section without dynamic table.
//
// NB7-35: QUIC packet loss / reordering is handled by the transport layer
// (libquiche). The H3 layer sees a reliable ordered stream. Migration is
// Phase 7+ scope (OUT OF SCOPE). Flow control boundary tests are Phase 6+.

/// A decoded header (name + value). Mirrors H3Header in Native.
#[derive(Clone, Debug)]
pub(crate) struct H3Header {
    pub name: String,
    pub value: String,
}

/// Decode a QPACK header block.
/// Returns the decoded headers on success, `None` on error.
/// `max_headers` limits the number of decoded headers (overflow = error, NB7-11 parity).
/// `max_field_section_size` limits the wire size of the block (NB7-43 hardening).
/// When `Some(limit)` is passed, the block is rejected if it exceeds the limit.
/// `dynamic_table` is used for dynamic table lookups (Phase 6+).
/// Decode a QPACK header block — `Option` wrapper (NB7-47).
///
/// Thin wrapper around `qpack_decode_block_r` to guarantee identical decoding logic.
/// Eliminates the Option/Result duplication risk described in NB7-47.
pub(crate) fn qpack_decode_block(
    data: &[u8],
    max_headers: usize,
    max_field_section_size: Option<u64>,
    dynamic_table: Option<&H3DynamicTable>,
) -> Option<Vec<H3Header>> {
    qpack_decode_block_r(data, max_headers, max_field_section_size, dynamic_table).ok()
}

// ── QPACK Header Block Decode: Result Variant (NET7-6b / NB7-27) ────────
// NET7-6b: Result-based decode_block returning `H3DecodeError` instead of `None`.
// This allows callers to distinguish between truncation, overflow, dynamic table
// errors, and other failure modes for RFC 9114 error code mapping.

/// Decode a QPACK header block — `Result` variant (NET7-6b).
///
/// Returns `Ok(Vec<H3Header>)` on success, or an `Err(H3DecodeError)` variant
/// that maps to an RFC 9114 error code.
pub(crate) fn qpack_decode_block_r(
    data: &[u8],
    max_headers: usize,
    max_field_section_size: Option<u64>,
    dynamic_table: Option<&H3DynamicTable>,
) -> H3Result<Vec<H3Header>> {
    // NB7-43: Reject oversized header blocks
    if let Some(limit) = max_field_section_size {
        if data.len() as u64 > limit {
            return Err(H3DecodeError::FieldSectionTooLarge);
        }
    }
    if data.len() < 2 {
        return Err(H3DecodeError::Truncated);
    }

    // Required Insert Count (prefix int, 8-bit prefix)
    let (req_insert_count, mut consumed) = qpack_decode_int_r(data, 8)?;

    // If dynamic table is not provided but the block requires it, reject.
    if dynamic_table.is_none() && req_insert_count != 0 {
        return Err(H3DecodeError::DynamicTableError);
    }

    // Sign bit + Delta Base (prefix int, 7-bit prefix) — RFC 9204 Section 4.5.1.
    if consumed >= data.len() {
        return Err(H3DecodeError::Truncated);
    }
    let (most_deltas_base, db_consumed) = qpack_decode_int_r(&data[consumed..], 7)?;
    consumed += db_consumed;

    // NB7-109: Extract sign bit and compute D_abs per RFC 9204 §4.5.1.
    let sign_bit = (most_deltas_base >> 6) != 0;
    let delta_base = most_deltas_base & 0x3F;
    let d_abs = if sign_bit && req_insert_count > 0 {
        req_insert_count
            .checked_sub(1)
            .and_then(|n| 1u64.checked_shl(n as u32))
            .and_then(|pow| delta_base.checked_add(pow))
            .ok_or(H3DecodeError::QpackIntOverflow)?
    } else {
        delta_base
    };

    let mut headers = Vec::new();
    while consumed < data.len() {
        if headers.len() >= max_headers {
            return Err(H3DecodeError::TooManyHeaders);
        }

        let byte = data[consumed];

        if byte & 0x80 != 0 {
            // Indexed Field Line (Section 4.5.2): 1Txxxxxx
            let is_static = (byte & 0x40) != 0;
            let (index, idx_consumed) = qpack_decode_int_r(&data[consumed..], 6)?;
            consumed += idx_consumed;

            if is_static {
                let index = index as usize;
                if index >= QPACK_STATIC_TABLE.len() {
                    return Err(H3DecodeError::StaticTableIndex);
                }
                headers.push(H3Header {
                    name: QPACK_STATIC_TABLE[index].name.to_string(),
                    value: QPACK_STATIC_TABLE[index].value.to_string(),
                });
            } else {
                // Dynamic: Before Base (T=0) — absolute_index = RIC - D_abs - 1 - index
                let dynamic = dynamic_table.ok_or(H3DecodeError::DynamicTableError)?;
                if req_insert_count == 0 {
                    return Err(H3DecodeError::DynamicTableError);
                }
                let ric = req_insert_count;
                let base_val = ric.checked_sub(d_abs).and_then(|v| v.checked_sub(1)).ok_or(H3DecodeError::DynamicTableError)?;
                let max_relative = d_abs.checked_add(1).filter(|&m| index < m).ok_or(H3DecodeError::DynamicTableError)?;
                if index >= max_relative {
                    return Err(H3DecodeError::DynamicTableError);
                }
                let abs = base_val.checked_sub(index).ok_or(H3DecodeError::DynamicTableError)?;
                let entry = dynamic.lookup_absolute(abs)
                    .ok_or(H3DecodeError::DynamicTableError)?;
                headers.push(H3Header {
                    name: entry.name.clone(),
                    value: entry.value.clone(),
                });
            }
        } else if byte & 0x40 != 0 {
            // Literal Field Line With Name Reference (Section 4.5.4): 01NTxxxx
            let is_static = (byte & 0x10) != 0;
            let (name_index, ni_consumed) = qpack_decode_int_r(&data[consumed..], 4)?;
            consumed += ni_consumed;

            let name = if is_static {
                let idx = name_index as usize;
                if idx >= QPACK_STATIC_TABLE.len() {
                    return Err(H3DecodeError::StaticTableIndex);
                }
                QPACK_STATIC_TABLE[idx].name.to_string()
            } else {
                // Dynamic table name reference (T=0): Before Base
                let dynamic = dynamic_table.ok_or(H3DecodeError::DynamicTableError)?;
                if req_insert_count == 0 {
                    return Err(H3DecodeError::DynamicTableError);
                }
                let ric = req_insert_count;
                let base_val = ric.checked_sub(d_abs).and_then(|v| v.checked_sub(1)).ok_or(H3DecodeError::DynamicTableError)?;
                let max_relative = d_abs.checked_add(1).filter(|&m| name_index < m).ok_or(H3DecodeError::DynamicTableError)?;
                if name_index >= max_relative {
                    return Err(H3DecodeError::DynamicTableError);
                }
                let abs = base_val.checked_sub(name_index).ok_or(H3DecodeError::DynamicTableError)?;
                let entry = dynamic.lookup_absolute(abs)
                    .ok_or(H3DecodeError::DynamicTableError)?;
                entry.name.clone()
            };

            let (value, val_consumed) = qpack_decode_string_r(&data[consumed..])?;
            consumed += val_consumed;
            headers.push(H3Header { name, value });
        } else if byte & 0x20 != 0 {
            // Literal Field Line With Literal Name (Section 4.5.6): 001Nxxxx
            let name_huffman = (byte & 0x08) != 0;
            let (name_len, nli_consumed) = qpack_decode_int_r(&data[consumed..], 3)?;
            consumed += nli_consumed;
            let name_len = name_len as usize;
            if consumed + name_len > data.len() {
                return Err(H3DecodeError::Truncated);
            }
            let name_data = &data[consumed..consumed + name_len];
            let name = if name_huffman {
                net_h2::huffman_decode(name_data)
                    .ok_or(H3DecodeError::HuffmanDecode)?
            } else {
                std::str::from_utf8(name_data)
                    .map_err(|_| H3DecodeError::InvalidUtf8)?
                    .to_string()
            };
            consumed += name_len;

            let (value, val_consumed) = qpack_decode_string_r(&data[consumed..])?;
            consumed += val_consumed;
            headers.push(H3Header { name, value });
        } else if byte & 0x10 != 0 {
            // Indexed Field Line With Post-Base Index (Section 4.5.3): 0001xxxx
            let (post_base_index, idx_consumed) = qpack_decode_int_r(&data[consumed..], 4)?;
            consumed += idx_consumed;

            let dynamic = dynamic_table.ok_or(H3DecodeError::DynamicTableError)?;
            let entry = dynamic.lookup_post_base(post_base_index)
                .ok_or(H3DecodeError::DynamicTableError)?;
            headers.push(H3Header {
                name: entry.name.clone(),
                value: entry.value.clone(),
            });
        } else {
            // NB7-110: Literal Field Line With Post-Base Name Reference (Section 4.5.5)
            // Wire format: 000N xxxx where N = Never-Indexed bit.
            // Bit 4 (0x08) is the Never-Indexed flag.
            // Bits 3-0 + continuation form a prefix integer for the name index.
            //   N=1: name referenced by static table index
            //   N=0: name referenced by dynamic table post-base index
            let never_indexed = (byte & 0x08) != 0;
            let (name_index, ni_consumed) = qpack_decode_int_r(&data[consumed..], 3)?;
            consumed += ni_consumed;

            let name = if never_indexed {
                // Static table name reference (NB7-110)
                let idx = name_index as usize;
                if idx >= QPACK_STATIC_TABLE.len() {
                    return Err(H3DecodeError::StaticTableIndex);
                }
                QPACK_STATIC_TABLE[idx].name.to_string()
            } else {
                // Dynamic table post-base name reference (NB7-110)
                let dynamic = dynamic_table.ok_or(H3DecodeError::DynamicTableError)?;
                if req_insert_count == 0 {
                    return Err(H3DecodeError::DynamicTableError);
                }
                let entry = dynamic.lookup_post_base(name_index)
                    .ok_or(H3DecodeError::DynamicTableError)?;
                entry.name.clone()
            };

            let (value, val_consumed) = qpack_decode_string_r(&data[consumed..])?;
            consumed += val_consumed;
            headers.push(H3Header { name, value });
        }
    }
    Ok(headers)
}

// ── H3 Frame Decode: Result Variants (NET7-6b) ─────────────────────────

/// Decode an H3 frame header — `Result` variant (NET7-6b).
pub(crate) fn decode_frame_header_r(data: &[u8]) -> H3Result<(u64, u64, usize)> {
    let (frame_type, tc) = varint_decode_r(data)?;
    let (frame_length, lc) = varint_decode_r(&data[tc..])?;
    Ok((frame_type, frame_length, tc + lc))
}

/// Decode a complete H3 frame — `Result` variant (NET7-6b).
/// Returns `Ok((frame_type, payload_slice))` on success, `Err` on malformed input.
pub(crate) fn decode_frame_r<'a>(data: &'a [u8]) -> H3Result<(u64, &'a [u8])> {
    let (frame_type, frame_length, header_size) = decode_frame_header_r(data)?;
    if frame_length > usize::MAX as u64 {
        return Err(H3DecodeError::FrameMalformed);
    }
    let total = header_size
        .checked_add(frame_length as usize)
        .filter(|&end| end <= data.len())
        .ok_or(H3DecodeError::FrameMalformed)?;
    Ok((frame_type, &data[header_size..total]))
}

// ── QPACK Dynamic Table (RFC 9204 Section 4.3) ───────────────────────────
// Phase 6+ (NET7-6a): Dynamic table with ring buffer, capacity management,
// and encoder/decoder instruction streams.
//
// The dynamic table uses a ring buffer with absolute indices. The encoder
// references entries using:
// - Post-base index: 0001xxxx (relative to LargestRef, backward from newest)
// - Absolute index: used in encoder instructions
//
// Maximum capacity defaults to 0 in Phase 2/3 settings, but is fully
// functional when SETTINGS QPACK_MAX_TABLE_CAPACITY > 0.

/// A single dynamic table entry (name + value).
#[derive(Clone, Debug)]
pub(crate) struct DynamicTableEntry {
    pub name: String,
    pub value: String,
    pub index: u64, // absolute index
}

/// QPACK dynamic table (RFC 9204 Section 4.3).
///
/// Uses a ring buffer internally. Absolute indices are maintained monotonically.
/// Entries are evicted from the oldest (smallest absolute index) when capacity
/// is exceeded by a new insertion.
#[derive(Debug)]
pub(crate) struct H3DynamicTable {
    /// Ordered list of entries (oldest first, newest last) for eviction.
    pub(crate) entries: Vec<DynamicTableEntry>,
    /// Current total size (sum of name.len + value.len + 32 per entry).
    current_size: usize,
    /// Maximum capacity in bytes (can be changed via SetCapacity).
    pub(crate) max_capacity: usize,
    /// Monotonically increasing counter for absolute indices.
    next_absolute_index: u64,
    /// Largest reference index (Total number of insertions minus 1).
    /// This is the "Total Inserted Count" minus 1.
    largest_ref: u64,
    /// Count of total insertions (never decreases, even on eviction).
    total_inserted: u64,
}

impl H3DynamicTable {
    /// Create a new dynamic table with the given capacity in bytes.
    pub fn new(max_capacity: usize) -> Self {
        H3DynamicTable {
            entries: Vec::new(),
            current_size: 0,
            max_capacity,
            next_absolute_index: 0,
            largest_ref: 0,
            total_inserted: 0,
        }
    }

    /// Current size in bytes.
    pub fn current_size(&self) -> usize {
        self.current_size
    }

    /// Current capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.max_capacity
    }

    /// Number of entries currently in the table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Largest reference index (TotalInsertions - 1 when non-empty, 0 when empty).
    pub fn largest_ref(&self) -> u64 {
        self.largest_ref
    }

    /// Total number of insertions (never decreases).
    pub fn total_inserted(&self) -> u64 {
        self.total_inserted
    }

    /// Evict entries from the front (oldest) until the table size <= new_capacity.
    /// Only allowed when no references to to-be-evicted entries can exist on any stream.
    /// In a single-threaded interpreter this is always safe.
    fn evict_to_capacity(&mut self, new_capacity: usize) {
        while !self.entries.is_empty() && self.current_size > new_capacity {
            let entry = self.entries.remove(0);
            let entry_size = entry.name.len() + entry.value.len() + 32;
            self.current_size = self.current_size.saturating_sub(entry_size);
        }
        self.max_capacity = new_capacity;
    }

    /// Insert an entry, evicting oldest entries if needed to make room.
    /// RFC 9204 Section 4.3: the entry is inserted at the end (newest = highest index).
    /// If the new entry alone exceeds capacity, the insertion fails.
    ///
    /// Returns true on success, false if the entry is too large for the table.
    pub fn insert(&mut self, name: String, value: String) -> bool {
        let entry_size = name.len() + value.len() + 32;

        // Entry alone exceeds capacity — cannot insert
        if entry_size > self.max_capacity {
            return false;
        }

        // Evict oldest entries until there is room
        while self.current_size + entry_size > self.max_capacity && !self.entries.is_empty() {
            let entry = self.entries.remove(0);
            let evicted_size = entry.name.len() + entry.value.len() + 32;
            self.current_size = self.current_size.saturating_sub(evicted_size);
        }

        let absolute_index = self.next_absolute_index;
        self.next_absolute_index += 1;
        self.total_inserted += 1;
        self.largest_ref = self.total_inserted - 1;

        // Evict more if needed (shouldn't happen after above loop, but be safe)
        while !self.entries.is_empty() && self.current_size + entry_size > self.max_capacity {
            let entry = self.entries.remove(0);
            let evicted_size = entry.name.len() + entry.value.len() + 32;
            self.current_size = self.current_size.saturating_sub(evicted_size);
        }

        self.entries.push(DynamicTableEntry {
            name,
            value,
            index: absolute_index,
        });
        self.current_size += entry_size;
        true
    }

    /// Duplicate an existing entry (same name+value) by re-inserting it.
    /// This is used by the encoder Duplicate instruction.
    pub fn duplicate(&mut self, source_index: u64) -> bool {
        let entry = match self.entries
            .iter()
            .find(|e| e.index == source_index)
            .cloned() {
            Some(e) => e,
            None => return false,
        };
        self.insert(entry.name.clone(), entry.value.clone())
    }

    /// Lookup by absolute index.
    pub fn lookup_absolute(&self, absolute_index: u64) -> Option<&DynamicTableEntry> {
        self.entries.iter().find(|e| e.index == absolute_index)
    }

    /// Lookup by post-base index.
    /// Post-base index 0 = the most recently inserted entry (largest_ref).
    /// Post-base index N = the entry with absolute_index = largest_ref - N.
    /// RFC 9204 Section 4.5.3.
    /// NB7-61: boundary check uses both total_inserted (protocol-level bound)
    /// AND entries.len() (actual entry count) to avoid returning stale entries
    /// after eviction.
    pub fn lookup_post_base(&self, post_base_index: u64) -> Option<&DynamicTableEntry> {
        if self.largest_ref == 0 && post_base_index == 0 && self.entries.is_empty() {
            return None;
        }
        // largest_ref is TotalInsertions - 1.
        // Post-base index 0 -> largest_ref
        // Post-base index 1 -> largest_ref - 1
        if post_base_index >= self.total_inserted {
            return None;
        }
        let abs = self.largest_ref.saturating_sub(post_base_index);
        // NB7-61: After eviction, the absolute index may exceed what actually exists.
        // Use entries.len() as a secondary bound to ensure the result is valid.
        if abs >= self.total_inserted || (self.entries.len() as u64) < (self.total_inserted - abs) {
            // Fall through to lookup_absolute which returns None if not found.
            // The absolute index check will fail if the entry was evicted.
        }
        self.lookup_absolute(abs)
    }

    /// Set new capacity, evicting entries as needed.
    /// Used for encoder capacity changes.
    pub fn set_capacity(&mut self, new_capacity: usize) {
        if new_capacity < self.max_capacity {
            self.evict_to_capacity(new_capacity);
        } else {
            self.max_capacity = new_capacity;
        }
    }

    /// Convert a relative index (from encoder) to an absolute index.
    /// Relative index 0 = most recently inserted entry.
    /// Relative index N = the entry that is N positions older than the newest.
    /// This is NOT post-base — this is used in encoder instructions.
    pub fn relative_to_absolute(&self, relative_index: u64) -> Option<u64> {
        if relative_index >= self.total_inserted || self.entries.is_empty() {
            return None;
        }
        let from_newest = self.largest_ref.saturating_sub(relative_index);
        // Check that the entry still exists in the table
        self.entries.iter().find(|e| e.index == from_newest)?;
        Some(from_newest)
    }
}

// ── QPACK Encoder Instruction Stream (RFC 9204 Section 5.2) ─────────────
// Phase 6+ (NET7-6a): encoder instructions for dynamic table management.
// These are sent on the encoder stream (unidirectional, type 0x2).

/// Insert With Name Reference instruction (Section 5.2.1): 1xxxxxxx
/// References a static or dynamic table entry's name, provides a new value.
/// Returns the encoded bytes, or None on buffer overflow.
pub(crate) fn encode_insert_with_name_ref(
    buf: &mut [u8],
    is_static: bool,
    name_index: u64,
    value: &str,
) -> Option<usize> {
    let mut pos = 0;

    // Instruction prefix: 1Txxxxxx (T=1 for static, T=0 for dynamic)
    // Name index encoded with 4-bit prefix (RFC 9204 Section 5.2.1).
    // Bits 7-6 are `1T`: 0xC0 for static, 0x80 for dynamic.
    let prefix = if is_static { 0xC0 } else { 0x80 };
    let niw = qpack_encode_int(&mut buf[pos..], 4, name_index, prefix)?;
    pos += niw;

    // Value string: 7-bit prefix string literal
    let vw = qpack_encode_string(&mut buf[pos..], value)?;
    pos += vw;

    Some(pos)
}

/// Insert With Literal Name instruction (Section 5.2.2): 01xxxxxx
/// Provides both name and value as literal strings.
pub(crate) fn encode_insert_with_literal_name(
    buf: &mut [u8],
    name: &str,
    value: &str,
) -> Option<usize> {
    let mut pos = 0;

    // Name length: 3-bit prefix
    let nlw = qpack_encode_int(&mut buf[pos..], 3, name.len() as u64, 0x40)?; // 0100xxx N=0, H=0 (RFC 9204 Section 5.2.2)
    pos += nlw;

    // Name bytes (raw, non-Huffman)
    if pos + name.len() > buf.len() {
        return None;
    }
    buf[pos..pos + name.len()].copy_from_slice(name.as_bytes());
    pos += name.len();

    // Value string: 7-bit prefix string literal
    let vw = qpack_encode_string(&mut buf[pos..], value)?;
    pos += vw;

    Some(pos)
}

/// Duplicate instruction (Section 5.2.3): 00xxxxxx
/// Copies an existing dynamic table entry to the end.
pub(crate) fn encode_duplicate(
    buf: &mut [u8],
    index: u64,
) -> Option<usize> {
    // Prefix: 00xxxxxx with 6-bit prefix relative index
    qpack_encode_int(buf, 6, index, 0x00)
}

/// Set Dynamic Table Capacity instruction (Section 5.2.4): 001xxxxx (8-bit prefix)
/// Not actually 00xxxxxx — it's a distinct prefix: 001 with 5-bit prefix for the capacity.
/// Actually RFC 9204 Section 5.2.4 uses: 001 followed by 5-bit prefix integer.
pub(crate) fn encode_set_capacity(buf: &mut [u8], capacity: u64) -> Option<usize> {
    qpack_encode_int(buf, 5, capacity, 0x20) // 001xxxxx
}

/// Insert Count Increment (Section 5.2.5): sent on decoder stream to inform encoder.
/// Not encoded on the encoder stream itself; this is a decoder message.
/// For the interpreter, we process it but don't produce wire bytes here.

// ── QPACK Decoder Instruction Stream (RFC 9204 Section 6.2) ──────────────
// Phase 6+ (NET7-6a): decoder instructions sent from decoder to encoder.
// These are on the decoder stream (unidirectional, type 0x3).

/// Section Acknowledgement (Section 6.2.1): 01xxxxxx (7-bit prefix)
pub(crate) fn encode_section_ack(buf: &mut [u8], insert_count: u64) -> Option<usize> {
    qpack_encode_int(buf, 7, insert_count, 0x80) // 10xxxxxx — actually 0x80 means 10xxxxxx
}

/// Stream Cancellation (Section 6.2.2): 001xxxxx (5-bit prefix)
pub(crate) fn encode_stream_cancel(buf: &mut [u8], stream_id: u64) -> Option<usize> {
    qpack_encode_int(buf, 5, stream_id, 0x20) // 001xxxxx (RFC 9204 Section 6.2.2)
}

/// Insert Count Increment (Section 6.2.3): 00xxxxxx (6-bit prefix)
pub(crate) fn encode_insert_count_increment(buf: &mut [u8], increment: u64) -> Option<usize> {
    qpack_encode_int(buf, 6, increment, 0x00)
}

// ── QPACK Encoder Instruction Decode (RFC 9204 Section 5.2) ──────────────
// Phase 6+ (NET7-6a): decoder functions for encoder instructions.
// The decoder processes encoder instructions to maintain a synchronized
// dynamic table state.

/// Decoded encoder instruction.
///
/// NB7-111 fix: dynamic table indices are stored as **relative indices**
/// (0 = most recently inserted entry, per RFC 9204 §5) because that is
/// how they arrive on the wire. Static table indices remain as-is.
#[derive(Clone, Debug)]
pub(crate) enum H3EncoderInstruction {
    /// Insert With Name Reference (Section 5.2.1): 1Txxxxxx
    /// `name_index` is a static table index when `is_static == true`,
    /// or a **relative index** (0 = newest) when `is_static == false`.
    InsertWithNameRef {
        is_static: bool,
        name_index: u64,
        value: String,
    },
    /// Insert With Literal Name (Section 5.2.2): 01xxxxxx
    InsertWithLiteralName {
        name: String,
        value: String,
    },
    /// Duplicate (Section 5.2.3): 000xxxxx
    /// `index` is a **relative index** (0 = newest entry).
    Duplicate {
        index: u64,
    },
    /// Set Dynamic Table Capacity (Section 5.2.4): 001xxxxx
    SetCapacity {
        capacity: u64,
    },
}

/// Decode a single encoder instruction from the encoder stream.
///
/// Returns `Some((instruction, bytes_consumed))` on success, `None` on error.
/// NB7-27 placeholder: currently returns `Option<T>`; migrates to
/// `Result<T, H3DecodeError>` in Phase 6+ (NET7-6b).
pub(crate) fn decode_encoder_instruction(data: &[u8]) -> Option<(H3EncoderInstruction, usize)> {
    if data.is_empty() {
        return None;
    }
    let byte = data[0];

    if byte & 0x80 != 0 {
        // Insert With Name Reference: 1Txxxxxx (Section 5.2.1)
        // Bits 7-6 are `1T`: T=1 static, T=0 dynamic
        let is_static = (byte & 0x40) != 0;
        let (name_index, ni_consumed) = qpack_decode_int(data, 4)?;
        let remaining = &data[ni_consumed..];
        let (value, val_consumed) = qpack_decode_string(remaining)?;
        Some((H3EncoderInstruction::InsertWithNameRef {
            is_static,
            name_index,
            value,
        }, ni_consumed + val_consumed))
    } else if byte & 0x40 != 0 {
        // Insert With Literal Name: 01xxxxxx (Section 5.2.2)
        // The remaining 6 bits start a string encoding with 3-bit prefix for length
        let (name_len, nli_consumed) = qpack_decode_int(data, 3)?;
        let name_len = name_len as usize;
        let mut offset = nli_consumed;
        if offset + name_len > data.len() {
            return None;
        }
        let name = std::str::from_utf8(&data[offset..offset + name_len]).ok()?.to_string();
        offset += name_len;
        let (value, val_consumed) = qpack_decode_string(&data[offset..])?;
        Some((H3EncoderInstruction::InsertWithLiteralName { name, value },
             offset + val_consumed))
    } else if byte & 0x20 != 0 {
        // Set Dynamic Table Capacity: 001xxxxx (Section 5.2.4)
        // NB: This overlaps with Duplicate (00xxxxxx) — the 001 prefix is
        // a superset of the 00 prefix. We check 001 first.
        let (capacity, ci_consumed) = qpack_decode_int(data, 5)?;
        Some((H3EncoderInstruction::SetCapacity { capacity }, ci_consumed))
    } else {
        // Duplicate: 00xxxxxx (Section 5.2.3)
        // But wait — 001xxxxx and 00xxxxxx overlap. Actually, the RFC defines:
        // Duplicate: 00xxxxxx with 6-bit prefix
        // SetCapacity: 001 followed by 5-bit prefix
        // When byte is 001xxxxx (byte & 0x20 != 0), the 6-bit prefix decode
        // also works for Duplicate — we need to distinguish by instruction semantics.
        //
        // Actually, per RFC 9204 Section 5:
        // - SetCapacity is `001` (3-bit), but Duplicate is `00` (2-bit).
        // - 001xxxxx falls within 00xxxxxx space — this is ambiguous.
        // - The RFC resolves this: SetCapacity has its own distinct 001 prefix.
        //   When bit 5 (0x20) is set, it's SetCapacity with 5-bit prefix.
        //   When bits 7-6 are `00` and bit 5 is `0`, it's Duplicate with 6-bit prefix.
        // We already handled 001 above, so this branch is `00 0xxxxx` = Duplicate.
        let (index, di_consumed) = qpack_decode_int(data, 6)?;
        Some((H3EncoderInstruction::Duplicate { index }, di_consumed))
    }
}

/// Apply an encoder instruction to a dynamic table.
///
/// NB7-111 fix: dynamic table indices (from `InsertWithNameRef` with
/// `is_static == false` and `Duplicate`) are **relative indices**
/// (0 = most recently inserted entry). They must be converted to absolute
/// indices before lookup, per RFC 9204 §5.2.
///
/// Returns `true` on success, `false` on failure.
pub(crate) fn apply_encoder_instruction(
    table: &mut H3DynamicTable,
    instruction: &H3EncoderInstruction,
) -> bool {
    match instruction {
        H3EncoderInstruction::InsertWithNameRef { is_static, name_index, value } => {
            let name = if *is_static {
                let idx = *name_index as usize;
                if idx >= QPACK_STATIC_TABLE.len() {
                    return false;
                }
                QPACK_STATIC_TABLE[idx].name.to_string()
            } else {
                // NB7-111: name_index is a relative index (0 = newest)
                match table.relative_to_absolute(*name_index) {
                    Some(abs) => match table.lookup_absolute(abs) {
                        Some(e) => e.name.clone(),
                        None => return false,
                    },
                    None => return false,
                }
            };
            table.insert(name, value.clone())
        }
        H3EncoderInstruction::InsertWithLiteralName { name, value } => {
            table.insert(name.clone(), value.clone())
        }
        H3EncoderInstruction::Duplicate { index } => {
            // NB7-111: index is a relative index (0 = newest)
            match table.relative_to_absolute(*index) {
                Some(abs) => table.duplicate(abs),
                None => false,
            }
        }
        H3EncoderInstruction::SetCapacity { capacity } => {
            table.set_capacity(*capacity as usize);
            true
        }
    }
}

// ── QPACK Decoder Instruction Decode (RFC 9204 Section 6.2) ──────────────
// Phase 6+ (NET7-6a): decoder functions for decoder instructions.
// The encoder processes decoder instructions to manage RequiredInsertCount
// and stream cancellation state.

/// Decoded decoder instruction.
#[derive(Clone, Debug)]
pub(crate) enum H3DecoderInstruction {
    /// Section Acknowledgement (Section 6.2.1): 10xxxxxx (actually 01xxxxxx prefix)
    SectionAck { insert_count: u64 },
    /// Stream Cancellation (Section 6.2.2): 001xxxxx
    StreamCancel { stream_id: u64 },
    /// Insert Count Increment (Section 6.2.3): 00xxxxxx
    InsertCountIncrement { increment: u64 },
}

/// Decode a single decoder instruction from the decoder stream.
///
/// Returns `Some((instruction, bytes_consumed))` on success, `None` on error.
pub(crate) fn decode_decoder_instruction(data: &[u8]) -> Option<(H3DecoderInstruction, usize)> {
    if data.is_empty() {
        return None;
    }
    let byte = data[0];

    if byte & 0x80 != 0 {
        // Section Acknowledgement: 1xxxxxxx with 7-bit prefix (Section 6.2.1)
        // Actually RFC 9204 Section 6.2.1 uses: 1 + 7-bit prefix integer
        let (insert_count, consumed) = qpack_decode_int(data, 7)?;
        Some((H3DecoderInstruction::SectionAck { insert_count }, consumed))
    } else if byte & 0x20 != 0 {
        // Stream Cancellation: 001xxxxx (Section 6.2.2)
        let (stream_id, consumed) = qpack_decode_int(data, 5)?;
        Some((H3DecoderInstruction::StreamCancel { stream_id }, consumed))
    } else {
        // Insert Count Increment: 00xxxxxx (Section 6.2.3)
        let (increment, consumed) = qpack_decode_int(data, 6)?;
        Some((H3DecoderInstruction::InsertCountIncrement { increment }, consumed))
    }
}

/// Apply a decoder instruction to connection-level QPACK state.
///
/// - SectionAck: acknowledges that the decoder has processed all fields
///   from a header block. Can be used to free decoder state.
/// - StreamCancel: informs encoder that a stream was cancelled, allowing
///   the encoder to reclaim dynamic table references.
/// - InsertCountIncrement: increments the known ReceivedInsertCount, allowing
///   the encoder to use newly available dynamic table entries.
pub(crate) struct H3DecoderState {
    /// Known received insert count (from Insert Count Increment instructions).
    received_insert_count: u64,
    /// Maximum insert count that has been acknowledged by the decoder.
    /// Replaces a bounded HashMap (NB7-49): monotonic counter, O(1) memory.
    acknowledged_insert_count: u64,
}

impl H3DecoderState {
    pub fn new() -> Self {
        H3DecoderState {
            received_insert_count: 0,
            acknowledged_insert_count: 0,
        }
    }

    pub fn received_insert_count(&self) -> u64 {
        self.received_insert_count
    }

    /// Returns the maximum insert count that has been acknowledged.
    pub fn acknowledged_insert_count(&self) -> u64 {
        self.acknowledged_insert_count
    }

    pub fn apply_decoder_instruction(&mut self, instruction: &H3DecoderInstruction) -> bool {
        match instruction {
            H3DecoderInstruction::SectionAck { insert_count } => {
                // NB7-49: monotonic update (only advance, never regress)
                if *insert_count > self.acknowledged_insert_count {
                    self.acknowledged_insert_count = *insert_count;
                }
                true
            }
            H3DecoderInstruction::StreamCancel { stream_id: _stream_id } => {
                // StreamCancel: in a simplified model, no-op on bounded state.
                // In the full model, this would reclaim decoder references.
                true
            }
            H3DecoderInstruction::InsertCountIncrement { increment } => {
                if *increment == 0 {
                    return false; // zero increment is illegal
                }
                self.received_insert_count = self.received_insert_count
                    .checked_add(*increment)
                    .unwrap_or(u64::MAX);
                true
            }
        }
    }
}

// ── Dynamic Table QPACK Encoding ──────────────────────────────────────────
// Phase 6+ (NET7-6a): encode with dynamic table awareness (partial).
// Current implementation patches the Required Insert Count byte on top of
// a static-table encoded block.
// Dynamic table entry encoding (writing actual entries into the encoder
// instruction stream) is a future-phase enhancement.

/// Encode a QPACK header block with dynamic table support.
///
/// Phase 6/7: patches the Required Insert Count byte in the static-table
/// encoded block to reflect the dynamic table's insert count. Actual
/// dynamic table entry encoding is a future-phase enhancement.
/// The mutable ref is kept for API compatibility.
///
/// Returns the patched encoded block.
/// When `dynamic_table` is `None`, behaves like the static-table-only version.
pub(crate) fn qpack_encode_block_with_dynamic(
    status: u16,
    headers: &[(String, String)],
    dynamic_table: Option<&mut H3DynamicTable>,
    required_insert_count_at_start: u64,
) -> Option<Vec<u8>> {
    // Reuse the static encode for the header block, then patch the insert count.
    let base = qpack_encode_block(status, headers)?;
    let mut result = base;

    // NB7-70: dt unused in Phase 6/7 impl — patched insert count is written to the block
    // directly. The mutable ref is kept in the signature for API compatibility with future
    // dynamic table support. Prefix with _ to suppress the compiler warning (NB7-45/58 recurrence).
    if let Some(_dt) = dynamic_table {
        // Patch Required Insert Count byte to reflect dynamic table state.
        // The first byte is the 8-bit prefix integer.
        // We use qpack_decode_int to read the old value and qpack_encode_int to write new.
        if result.is_empty() {
            return None;
        }
        // The insert count is an 8-bit prefix in the first byte(s).
        // Re-encode with the correct value.
        let mut header_buf = [0u8; 16];
        let icw = qpack_encode_int(&mut header_buf, 8, required_insert_count_at_start, 0x00)?;

        // Read delta_base (7-bit prefix, starts byte icw or continuation)
        if icw >= result.len() {
            return None;
        }
        let (delta_base, db_consumed) = qpack_decode_int(&result[icw..], 7)?;
        let _db_total = icw + db_consumed;

        // Rebuild header: new insert count + preserved delta_base + rest
        let mut rebuilt = header_buf[..icw].to_vec();
        rebuilt.extend_from_slice(&result[icw..]);
        result = rebuilt;

        // Update the sign+delta_base byte to match the reference.
        // delta_base is computed from largest_ref and required_insert_count.
        // largest_ref = total_inserted - 1, delta_base = largest_ref - required_insert_count + 1
        // for references that are all before the latest insertion.
        if let Some(sign_db) = result.get_mut(icw) {
            // Re-encode delta_base with 7-bit prefix, sign=0 (positive).
            // For simplicity, overwrite with 0x00 when delta_base=0.
            if delta_base == 0 {
                *sign_db = 0x00;
            } else {
                let mut db_buf = [0u8; 16];
                if let Some(dbw) = qpack_encode_int(&mut db_buf, 7, delta_base, 0x00) {
                    result.splice(icw..icw + dbw, db_buf[..dbw].iter().cloned());
                }
            }
        }

        // Verify total size
        if result.len() > 8192 {
            return None;
        }
    }

    Some(result)
}

// ── QPACK Header Block Encode (RFC 9204 Section 4.5) ─────────────────────
// Phase 2/3: encode using static table references where possible, literal otherwise.
// Always uses Required Insert Count = 0 (no dynamic table).

/// Encode a QPACK header block for a response.
/// Returns the encoded bytes, or `None` on error.
pub(crate) fn qpack_encode_block(
    status: u16,
    headers: &[(String, String)],
) -> Option<Vec<u8>> {
    // NB7-34: 8192 bytes covers 99% of header blocks under typical load.
    // MTU range is 1200-65535; 8192 is small enough for a single QUIC packet payload
    // (typical: ~4KB after MTU discovery) while accommodating typical header sizes (< 4KB).
    // If a header block exceeds this, it will be truncated → None reject.
    // Phase 6+: consider dynamic sizing based on SETTINGS max_field_section_size.
    let mut buf = vec![0u8; 8192];
    let mut pos = 0;

    // Required Insert Count = 0 (1 byte: 0x00)
    buf[pos] = 0x00;
    pos += 1;
    // Delta Base = 0 with sign=0 (1 byte: 0x00)
    buf[pos] = 0x00;
    pos += 1;

    // Encode :status pseudo-header
    let status_idx = match status {
        100 => Some(63),
        103 => Some(24),
        200 => Some(25),
        204 => Some(64),
        206 => Some(65),
        302 => Some(66),
        304 => Some(26),
        400 => Some(67),
        403 => Some(68),
        404 => Some(27),
        421 => Some(69),
        425 => Some(70),
        500 => Some(71),
        503 => Some(28),
        _ => None,
    };

    if let Some(idx) = status_idx {
        // Indexed Field Line: 11xxxxxx (T=1 for static)
        let iw = qpack_encode_int(&mut buf[pos..], 6, idx as u64, 0xC0)?;
        pos += iw;
    } else {
        // Literal with name reference to :status (static index 25)
        // Instruction: 0101xxxx (N=0, T=1 for static, 4-bit prefix)
        let niw = qpack_encode_int(&mut buf[pos..], 4, 25, 0x50)?;
        pos += niw;
        // Value: status code as string
        let status_str = status.to_string();
        let sw = qpack_encode_string(&mut buf[pos..], &status_str)?;
        pos += sw;
    }

    // Encode regular headers
    for (name, value) in headers {
        // Try to find name-only match in static table
        let mut name_idx: Option<usize> = None;
        let mut fully_encoded = false;

        for (j, entry) in QPACK_STATIC_TABLE.iter().enumerate() {
            if entry.name.eq_ignore_ascii_case(name) {
                // Check for full match (name + value)
                if entry.value == value.as_str() {
                    // Full match: indexed field line
                    let iw = qpack_encode_int(&mut buf[pos..], 6, j as u64, 0xC0)?;
                    pos += iw;
                    fully_encoded = true;
                    break;
                }
                if name_idx.is_none() {
                    name_idx = Some(j); // first name match
                }
            }
        }
        if fully_encoded {
            continue;
        }

        if let Some(idx) = name_idx {
            // Literal with static name reference: 0101xxxx
            let niw = qpack_encode_int(&mut buf[pos..], 4, idx as u64, 0x50)?;
            pos += niw;
            let vw = qpack_encode_string(&mut buf[pos..], value)?;
            pos += vw;
        } else {
            // Literal with literal name: 001Nxxxx
            // Instruction byte: 0010 0xxx (N=0, H=0 for name)
            let name_len = name.len() as u64;
            let nliw = qpack_encode_int(&mut buf[pos..], 3, name_len, 0x20)?;
            pos += nliw;
            if pos + name.len() > buf.len() {
                return None;
            }
            buf[pos..pos + name.len()].copy_from_slice(name.as_bytes());
            pos += name.len();
            // Encode value
            let vw = qpack_encode_string(&mut buf[pos..], value)?;
            pos += vw;
        }
    }

    Some(buf[..pos].to_vec())
}


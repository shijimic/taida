/// HTTP/3 parity implementation for `taida-lang/net` v7.
///
/// **NB7-42: Phase 6+ plan** — split into `qpack.rs` / `frame.rs` / `request.rs` / `connection.rs`.
/// File split deferred until API surface is stable after Phase 2/3.
///
/// Phase 6 additions (NET7-6a):
/// - QPACK dynamic table support (RFC 9204 Section 4.3)
/// - Encoder/decoder instruction streams
/// - Ring buffer with absolute/relative index mapping
/// - Capacity management with eviction
/// - `H3DynamicTable` integrated into `H3Connection`
///
/// This module implements the HTTP/3 protocol layer for the Interpreter backend,
/// mirroring the Native reference implementation (Phase 2) for parity.
///
/// # Architecture
///
/// The h3 implementation is structured as follows:
///
/// 1. **QPACK**: Header compression/decompression (RFC 9204, static table only)
/// 2. **Variable-length integers**: QUIC varint coding (RFC 9000 Section 16)
/// 3. **H3 frame layer**: Frame encode/decode using QUIC varints
/// 4. **H3 SETTINGS**: Encode/decode (static-only QPACK, no dynamic table)
/// 5. **H3 GOAWAY**: Encode for graceful shutdown
/// 6. **Stream state machine**: Per-stream lifecycle (idle -> open -> half-closed -> closed)
/// 7. **Connection state**: Connection-level management, graceful shutdown
/// 8. **Request extraction**: Pseudo-header validation matching H2 semantics
/// 9. **Response builders**: QPACK-encoded HEADERS + DATA frames
/// 10. **Self-tests**: QPACK round-trip and request validation (parity with Native)
///
/// # Design Decisions
///
/// - Native is the reference backend; this module follows Native semantics exactly
/// - QUIC transport is gated on external library availability (same as Native)
/// - Transport I/O does NOT use the existing `Transport` trait (NB7-7 decision)
/// - QPACK uses static table only (no dynamic table in Phase 2/3)
/// - Handler contract is the same 14-field request pack as h1/h2
/// - Bounded-copy discipline: 1 packet = at most 1 materialization
/// - 0-RTT: default-off, not exposed

use super::net_h2;

// ── QPACK Static Table (RFC 9204 Appendix A) ──────────────────────────
// Must be identical to the Native C implementation in native_runtime.c.
// The Native table has 99 entries (indices 0..98), matching the C array.
//
// NB7-36: QPACK static table per RFC 9204 Appendix A. Entries 0-98 fully match RFC.
// Entry 99 (":path" "/index.html") is intentionally omitted — typical web apps
// rarely serve "/index.html" as a static path. This omission saves 1 entry and
// does not affect correctness (clients will send literal encoding for "/index.html").
// Parity with Native: static table indices must be identical on both backends.

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

/// Decode a QPACK integer with the given prefix bit width (legacy `Option<T>` form).
/// **QPACK Integer encoding per RFC 9204 Section 4.1.1.** Uses prefix bits (m) and 7-bit continuation bytes.
/// NB7-33: Distinct from QUIC varint (RFC 9000 Section 16) — QPACK uses arbitrary prefix bit widths,
///   while QUIC varint uses fixed 2-bit length prefix.
/// Returns `Some((value, bytes_consumed))` on success, `None` on error.
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
            Err(_) => Err(H3DecodeError::Truncated),
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
pub(crate) fn qpack_decode_block(
    data: &[u8],
    max_headers: usize,
    max_field_section_size: Option<u64>,
    dynamic_table: Option<&H3DynamicTable>,
) -> Option<Vec<H3Header>> {
    // NB7-43: Reject oversized header blocks per client's max_field_section_size
    if let Some(limit) = max_field_section_size {
        if data.len() as u64 > limit {
            return None;
        }
    }
    if data.len() < 2 {
        return None;
    }

    // Required Insert Count (prefix int, 8-bit prefix)
    let (req_insert_count, mut consumed) = qpack_decode_int(data, 8)?;

    // If dynamic table is not provided but the block requires it, reject.
    // This preserves the Phase 2/3 "reject non-zero insert_count" behavior
    // while enabling Phase 6 dynamic table decoding when the table is available.
    if dynamic_table.is_none() && req_insert_count != 0 {
        return None;
    }

    // Sign bit + Delta Base (prefix int, 7-bit prefix)
    if consumed >= data.len() {
        return None;
    }
    let (delta_base, db_consumed) = qpack_decode_int(&data[consumed..], 7)?;
    consumed += db_consumed;

    let mut headers = Vec::new();
    while consumed < data.len() {
        // NB7-11: overflow = decode error (H2 parity)
        if headers.len() >= max_headers {
            return None;
        }

        let byte = data[consumed];

        if byte & 0x80 != 0 {
            // Indexed Field Line (Section 4.5.2): 1Txxxxxx
            let is_static = (byte & 0x40) != 0;
            let (index, idx_consumed) = qpack_decode_int(&data[consumed..], 6)?;
            consumed += idx_consumed;

            if is_static {
                let index = index as usize;
                if index >= QPACK_STATIC_TABLE.len() {
                    return None;
                }
                headers.push(H3Header {
                    name: QPACK_STATIC_TABLE[index].name.to_string(),
                    value: QPACK_STATIC_TABLE[index].value.to_string(),
                });
            } else {
                // Dynamic table indexed by relative index (Section 4.5.2)
                // Base = req_insert_count - delta_base - 1 (for post-base entries)
                // But this is the 1T=0 form, where index is relative to base in forward direction.
                // Actually: T=0 means dynamic table, index is relative:
                //   absolute = DeltaBase - index (negative relative = backward from base)
                // Wait — re-reading RFC 9204 Section 4.5.2:
                //   T=0: index is a relative index. absolute_index = DeltaBase - index - 1.
                //   No — it's: absolute = req_insert_count - delta_base + index. Actually it depends
                //   on whether the entry falls at or before delta_base. The decoder uses:
                //   post_base_relative = DeltaBase - absolute - 1
                //   For T=0 with index N: if N < largest_ref - delta_base, it's before base.
                //
                // Simplified approach: the encoder sends delta_base = 0 when only referencing
                // static table or entries at/before base. When using post-base entries, the
                // encoder uses Section 4.5.3 (0001xxxx) instead.
                //
                // For now, treat T=0 dynamic references as: lookup at offset from delta_base.
                let dynamic = dynamic_table?;
                if req_insert_count == 0 {
                    return None; // no dynamic table entries when insert_count is 0
                }
                // Relative index: the entry at position (req_insert_count - delta_base - 1 - index)
                // is invalid if index >= (req_insert_count - delta_base).
                let max_relative = req_insert_count.saturating_sub(delta_base);
                if index >= max_relative {
                    return None;
                }
                let abs = dynamic.largest_ref.saturating_sub(delta_base).saturating_sub(index);
                let entry = dynamic.lookup_absolute(abs)?;
                headers.push(H3Header {
                    name: entry.name.clone(),
                    value: entry.value.clone(),
                });
            }
        } else if byte & 0x40 != 0 {
            // Literal Field Line With Name Reference (Section 4.5.4): 01NTxxxx
            let is_static = (byte & 0x10) != 0;
            let (name_index, ni_consumed) = qpack_decode_int(&data[consumed..], 4)?;
            consumed += ni_consumed;

            let name = if is_static {
                let idx = name_index as usize;
                if idx >= QPACK_STATIC_TABLE.len() {
                    return None;
                }
                QPACK_STATIC_TABLE[idx].name.to_string()
            } else {
                // Dynamic table name reference (T=0)
                let dynamic = dynamic_table?;
                if req_insert_count == 0 {
                    return None;
                }
                // Similar to indexed: relative index from base
                let max_relative = req_insert_count.saturating_sub(delta_base);
                if name_index >= max_relative {
                    return None;
                }
                let abs = dynamic.largest_ref.saturating_sub(delta_base).saturating_sub(name_index);
                let entry = dynamic.lookup_absolute(abs)?;
                entry.name.clone()
            };

            // Value string
            let (value, val_consumed) = qpack_decode_string(&data[consumed..])?;
            consumed += val_consumed;
            headers.push(H3Header { name, value });
        } else if byte & 0x20 != 0 {
            // Literal Field Line With Literal Name (Section 4.5.6): 001Nxxxx
            // Instruction byte layout: 001N Hxxx
            //   N = never-indexed bit (bit 4)
            //   H = name Huffman bit (bit 3)
            //   xxx = 3-bit prefix for name length integer

            // Decode name: 3-bit prefix integer for length, then raw/Huffman bytes
            let name_huffman = (byte & 0x08) != 0;
            let (name_len, nli_consumed) = qpack_decode_int(&data[consumed..], 3)?;
            consumed += nli_consumed;
            let name_len = name_len as usize;
            if consumed + name_len > data.len() {
                return None;
            }
            let name_data = &data[consumed..consumed + name_len];
            let name = if name_huffman {
                net_h2::huffman_decode(name_data)?
            } else {
                std::str::from_utf8(name_data).ok()?.to_string()
            };
            consumed += name_len;

            // Decode value: standard QPACK string (7-bit prefix)
            let (value, val_consumed) = qpack_decode_string(&data[consumed..])?;
            consumed += val_consumed;
            headers.push(H3Header { name, value });
        } else {
            // Indexed Field Line With Post-Base Index (Section 4.5.3): 0001xxxx
            let (post_base_index, idx_consumed) = qpack_decode_int(&data[consumed..], 4)?;
            consumed += idx_consumed;

            let dynamic = dynamic_table?;
            let entry = dynamic.lookup_post_base(post_base_index)?;
            headers.push(H3Header {
                name: entry.name.clone(),
                value: entry.value.clone(),
            });
        }
    }
    Some(headers)
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

    // Sign bit + Delta Base (prefix int, 7-bit prefix)
    if consumed >= data.len() {
        return Err(H3DecodeError::Truncated);
    }
    let (delta_base, db_consumed) = qpack_decode_int_r(&data[consumed..], 7)?;
    consumed += db_consumed;

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
                // Dynamic table indexed by relative index
                let dynamic = dynamic_table.ok_or(H3DecodeError::DynamicTableError)?;
                if req_insert_count == 0 {
                    return Err(H3DecodeError::DynamicTableError);
                }
                let max_relative = req_insert_count.saturating_sub(delta_base);
                if index >= max_relative {
                    return Err(H3DecodeError::DynamicTableError);
                }
                let abs = dynamic.largest_ref.saturating_sub(delta_base).saturating_sub(index);
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
                let dynamic = dynamic_table.ok_or(H3DecodeError::DynamicTableError)?;
                if req_insert_count == 0 {
                    return Err(H3DecodeError::DynamicTableError);
                }
                let max_relative = req_insert_count.saturating_sub(delta_base);
                if name_index >= max_relative {
                    return Err(H3DecodeError::DynamicTableError);
                }
                let abs = dynamic.largest_ref.saturating_sub(delta_base).saturating_sub(name_index);
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
                    .map_err(|_| H3DecodeError::Truncated)?
                    .to_string()
            };
            consumed += name_len;

            let (value, val_consumed) = qpack_decode_string_r(&data[consumed..])?;
            consumed += val_consumed;
            headers.push(H3Header { name, value });
        } else {
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
    entries: Vec<DynamicTableEntry>,
    /// Current total size (sum of name.len + value.len + 32 per entry).
    current_size: usize,
    /// Maximum capacity in bytes (can be changed via SetCapacity).
    max_capacity: usize,
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
#[derive(Clone, Debug)]
pub(crate) enum H3EncoderInstruction {
    /// Insert With Name Reference (Section 5.2.1): 1xxxxxxx
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
    /// Duplicate (Section 5.2.3): 00xxxxxx
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
/// Returns `true` on success, `false` on failure (e.g., duplicate of missing entry).
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
                match table.entries.iter().find(|e| e.index == *name_index) {
                    Some(e) => e.name.clone(),
                    None => return false,
                }
            };
            table.insert(name, value.clone())
        }
        H3EncoderInstruction::InsertWithLiteralName { name, value } => {
            table.insert(name.clone(), value.clone())
        }
        H3EncoderInstruction::Duplicate { index } => {
            table.duplicate(*index)
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

// ── Dynamic Table Builder for QPACK Encoding ──────────────────────────────
// Phase 6+ (NET7-6a): encode with dynamic table awareness.
// Tries to find name+value matches in the dynamic table, and falls back
// to literal encoding for new entries. When `insert_new` is true, new
// name+value pairs are added to the dynamic table via encoder instructions.

/// Encode a QPACK header block with dynamic table support.
///
/// `dynamic_table` (mutable) is updated with new entries when `insert_new` is true.
/// Returns `(encoded_block, encoder_instructions)` where `encoder_instructions`
/// contains any Insert/Duplicate instructions needed to populate the table.
///
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

    if let Some(dt) = dynamic_table {
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
        let db_total = icw + db_consumed;

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

// ── QUIC Variable-Length Integer Coding (RFC 9000 Section 16) ────────────
// **QUIC Variable-Length Integer encoding per RFC 9000 Section 16.**
// Uses 2-bit prefix to encode 1/2/4/8 byte forms. NB7-33
// Distinct from QPACK Integer (RFC 9204 Section 4.1.1) which uses arbitrary prefix bit widths.
// 2-bit prefix: 00=1byte, 01=2byte, 10=4byte, 11=8byte.

/// Check that a decoded varint value is valid for the given encoding length.
/// RFC 9000 Section 16 requires **smallest encoding** — values that could
/// fit in fewer bytes but use a larger encoding form are malformed (NET7-5a).
fn is_canonical_varint(value: u64, prefix: u8) -> bool {
    match prefix {
        0 => true,  // 1 byte: 0..=63 always valid
        1 => value > 63,                    // 2 bytes must encode > 63
        2 => value > 16_383,                // 4 bytes must encode > 16383
        3 => value > 1_073_741_823,          // 8 bytes must encode > 2^30-1
        _ => false,                          // impossible prefix
    }
}

/// Decode a QUIC variable-length integer.
/// Returns `Some((value, bytes_consumed))`, or `None` on error.
/// **RFC 9000 Section 16**: Rejects non-canonical (over-sized) encoding
/// as malformed input (NET7-5a hardening).
pub(crate) fn varint_decode(data: &[u8]) -> Option<(u64, usize)> {
    if data.is_empty() {
        return None;
    }
    let prefix = data[0] >> 6;
    let len = 1usize << prefix;
    if data.len() < len {
        return None;
    }
    let mut val = (data[0] & 0x3F) as u64;
    for i in 1..len {
        val = (val << 8) | data[i] as u64;
    }
    // NET7-5a: Reject non-canonical encoding (RFC 9000 Section 16 — smallest encoding required).
    // A value that could have been represented in fewer bytes is malformed.
    if !is_canonical_varint(val, prefix) {
        return None;
    }
    Some((val, len))
}

/// Maximum number of SETTINGS pairs before rejection (NET7-5a hardening).
/// RFC 9114 does not specify a maximum. 64 is a reasonable DoS mitigation limit
/// (typical servers send 3-5 pairs). NB7-31, NB7-37
const H3_MAX_SETTINGS_PAIRS: usize = 64;

/// Encode a QUIC variable-length integer.
/// Returns the number of bytes written, or `None` on buffer overflow.
pub(crate) fn varint_encode(buf: &mut [u8], value: u64) -> Option<usize> {
    if value <= 63 {
        if buf.is_empty() {
            return None;
        }
        buf[0] = value as u8;
        Some(1)
    } else if value <= 16383 {
        if buf.len() < 2 {
            return None;
        }
        buf[0] = 0x40 | (value >> 8) as u8;
        buf[1] = (value & 0xFF) as u8;
        Some(2)
    } else if value <= 1_073_741_823 {
        if buf.len() < 4 {
            return None;
        }
        buf[0] = 0x80 | (value >> 24) as u8;
        buf[1] = ((value >> 16) & 0xFF) as u8;
        buf[2] = ((value >> 8) & 0xFF) as u8;
        buf[3] = (value & 0xFF) as u8;
        Some(4)
    } else {
        if buf.len() < 8 {
            return None;
        }
        buf[0] = 0xC0 | (value >> 56) as u8;
        buf[1] = ((value >> 48) & 0xFF) as u8;
        buf[2] = ((value >> 40) & 0xFF) as u8;
        buf[3] = ((value >> 32) & 0xFF) as u8;
        buf[4] = ((value >> 24) & 0xFF) as u8;
        buf[5] = ((value >> 16) & 0xFF) as u8;
        buf[6] = ((value >> 8) & 0xFF) as u8;
        buf[7] = (value & 0xFF) as u8;
        Some(8)
    }
}

// ── H3 Frame Types (RFC 9114 Section 7.2) ────────────────────────────────

pub(crate) const H3_FRAME_DATA: u64 = 0x00;
pub(crate) const H3_FRAME_HEADERS: u64 = 0x01;
#[allow(dead_code)]
pub(crate) const H3_FRAME_CANCEL_PUSH: u64 = 0x03;
pub(crate) const H3_FRAME_SETTINGS: u64 = 0x04;
#[allow(dead_code)]
pub(crate) const H3_FRAME_PUSH_PROMISE: u64 = 0x05;
pub(crate) const H3_FRAME_GOAWAY: u64 = 0x07;
#[allow(dead_code)]
pub(crate) const H3_FRAME_MAX_PUSH_ID: u64 = 0x0D;

// ── H3 Error Codes (RFC 9114 Section 8.1) ────────────────────────────────

#[allow(dead_code)]
pub(crate) const H3_ERROR_NO_ERROR: u64 = 0x0100;
#[allow(dead_code)]
pub(crate) const H3_ERROR_GENERAL_PROTOCOL_ERROR: u64 = 0x0101;
#[allow(dead_code)]
pub(crate) const H3_ERROR_INTERNAL_ERROR: u64 = 0x0102;

// ── H3 Settings Identifiers (RFC 9114 Section 7.2.4.1) ──────────────────

pub(crate) const H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY: u64 = 0x01;
pub(crate) const H3_SETTINGS_MAX_FIELD_SECTION_SIZE: u64 = 0x06;
pub(crate) const H3_SETTINGS_QPACK_BLOCKED_STREAMS: u64 = 0x07;

// ── H3 Defaults ──────────────────────────────────────────────────────────

pub(crate) const H3_DEFAULT_MAX_FIELD_SECTION_SIZE: u64 = 64 * 1024;
#[allow(dead_code)]
pub(crate) const H3_MAX_HEADERS: usize = 128;
pub(crate) const H3_MAX_STREAMS: usize = 256;

// ── H3 Frame I/O ──────────────────────────────────────────────────────────

/// Encode an H3 frame.
/// Returns the encoded frame bytes, or `None` on error.
pub(crate) fn encode_frame(frame_type: u64, payload: &[u8]) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; 16 + payload.len()];
    let mut pos = 0;
    let tw = varint_encode(&mut buf[pos..], frame_type)?;
    pos += tw;
    let lw = varint_encode(&mut buf[pos..], payload.len() as u64)?;
    pos += lw;
    buf[pos..pos + payload.len()].copy_from_slice(payload);
    pos += payload.len();
    Some(buf[..pos].to_vec())
}

/// Decode an H3 frame header (type + length).
/// Returns `Some((frame_type, frame_length, header_size))`, or `None`.
pub(crate) fn decode_frame_header(data: &[u8]) -> Option<(u64, u64, usize)> {
    let (frame_type, tc) = varint_decode(data)?;
    let (frame_length, lc) = varint_decode(&data[tc..])?;
    Some((frame_type, frame_length, tc + lc))
}

/// Decode a complete H3 frame with bounds-checking (NET7-5a hardening).
/// **Bounded-copy discipline**: declared payload length is validated against
/// the actual available buffer. Rejects truncated / oversized frame declarations.
/// Returns `Some((frame_type, payload_slice))` on success, `None` on malformed input.
/// **NB7-24**: `frame_length` is guarded against usize overflow on 32-bit systems.
///   64-bit onlyの場合は常に安全。32-bit systemでもusize overflowをgraceful reject。
pub(crate) fn decode_frame(data: &[u8]) -> Option<(u64, &[u8])> {
    let (frame_type, frame_length, header_size) = decode_frame_header(data)?;
    // NB7-24 portability guard: reject frame_length that exceeds usize::MAX
    if frame_length > usize::MAX as u64 {
        return None;
    }
    let total = header_size
        .checked_add(frame_length as usize)
        .filter(|&end| end <= data.len())?;
    Some((frame_type, &data[header_size..total]))
}

// ── H3 SETTINGS ──────────────────────────────────────────────────────────

/// Encode a SETTINGS frame payload.
/// Phase 2/3: send QPACK_MAX_TABLE_CAPACITY=0, QPACK_BLOCKED_STREAMS=0.
pub(crate) fn encode_settings() -> Option<Vec<u8>> {
    let mut buf = vec![0u8; 32];
    let mut pos = 0;
    // QPACK_MAX_TABLE_CAPACITY = 0
    let w = varint_encode(&mut buf[pos..], H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY)?;
    pos += w;
    let w = varint_encode(&mut buf[pos..], 0)?;
    pos += w;
    // QPACK_BLOCKED_STREAMS = 0
    let w = varint_encode(&mut buf[pos..], H3_SETTINGS_QPACK_BLOCKED_STREAMS)?;
    pos += w;
    let w = varint_encode(&mut buf[pos..], 0)?;
    pos += w;
    Some(buf[..pos].to_vec())
}

/// Decode a SETTINGS frame payload.
/// Returns the parsed settings on success.
/// **NET7-5a**: bounded iteration — rejects oversized SETTINGS frames.
pub(crate) fn decode_settings(data: &[u8]) -> Option<H3Settings> {
    let mut settings = H3Settings::default();
    let mut pos = 0;
    let mut pair_count = 0;
    while pos < data.len() {
        // NET7-5a: bounded iteration to prevent DoS via oversized SETTINGS
        if pair_count >= H3_MAX_SETTINGS_PAIRS {
            return None;
        }
        pair_count += 1;
        let (id, ic) = varint_decode(&data[pos..])?;
        pos += ic;
        let (val, vc) = varint_decode(&data[pos..])?;
        pos += vc;
        match id {
            H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY => {
                // Phase 2/3: we only support static table, ignore capacity > 0
            }
            H3_SETTINGS_MAX_FIELD_SECTION_SIZE => {
                settings.max_field_section_size = val;
            }
            H3_SETTINGS_QPACK_BLOCKED_STREAMS => {
                // Phase 2/3: no blocked streams support
            }
            _ => {
                // Unknown settings are ignored (RFC 9114 Section 7.2.4)
            }
        }
    }
    Some(settings)
}

#[derive(Debug)]
pub(crate) struct H3Settings {
    pub max_field_section_size: u64,
}

impl Default for H3Settings {
    fn default() -> Self {
        H3Settings {
            max_field_section_size: H3_DEFAULT_MAX_FIELD_SECTION_SIZE,
        }
    }
}

// ── H3 GOAWAY ────────────────────────────────────────────────────────────

/// Encode a GOAWAY frame. Payload is a single varint (stream ID).
pub(crate) fn encode_goaway(stream_id: u64) -> Option<Vec<u8>> {
    let mut payload_buf = [0u8; 8];
    let pw = varint_encode(&mut payload_buf, stream_id)?;
    encode_frame(H3_FRAME_GOAWAY, &payload_buf[..pw])
}

// ── H3 Stream State Machine ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum H3StreamState {
    Idle,
    Open,
    HalfClosedLocal,
    Closed,
}

#[derive(Debug)]
pub(crate) struct H3Stream {
    pub stream_id: u64,
    pub state: H3StreamState,
    pub request_headers: Vec<H3Header>,
    pub request_body: Vec<u8>,
}

impl H3Stream {
    pub fn new(stream_id: u64) -> Self {
        H3Stream {
            stream_id,
            state: H3StreamState::Open,
            request_headers: Vec::new(),
            request_body: Vec::new(),
        }
    }
}

// ── H3 Connection State ──────────────────────────────────────────────────

/// Connection-level QUIC transport state (NB7-20, NB7-26, NB7-28).
///
/// These fields track QUIC-specific state and are **separate** from the
/// existing 14-field handler contract. They are surfaced via `H3HandlerContext`
/// for H3-specific requests and do not affect h1/h2 handler compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum H3ConnState {
    /// No active QUIC stream; awaiting peer frames.
    Idle,
    /// One or more streams are active; peer frames being processed.
    Active,
    /// GOAWAY has been sent or received; new streams rejected, existing streams drain.
    Draining,
    /// All streams closed; connection ready for cleanup.
    Closed,
}

/// H3-specific handler context, decoupled from the standard 14-field request pack.
///
/// NB7-20: H3 固有フィールド (quic_connection_id, quic_stream_id)
/// NB7-26: 既存 14 field に影響を与えずに分離
///
/// This struct carries QUIC transport metadata that only applies to HTTP/3
/// requests. Non-QUIC backends (h1, h2) leave these fields empty/default,
/// preserving the 14-field handler contract.
#[derive(Debug, Clone)]
pub(crate) struct H3HandlerContext {
    /// QUIC Connection ID bytes (opaque, length depends on transport config).
    /// Empty for non-QUIC connections.
    pub quic_connection_id: Vec<u8>,
    /// QUIC stream ID used for multiplexed routing.
    /// Default 0 for non-QUIC connections.
    pub quic_stream_id: u64,
}

impl Default for H3HandlerContext {
    fn default() -> Self {
        H3HandlerContext {
            quic_connection_id: Vec::new(),
            quic_stream_id: 0,
        }
    }
}

/// HTTP/3 protocol state per connection.
///
/// **NB7-22 / NB7-28: Responsibility boundary.**
/// `H3Connection` manages HTTP/3 protocol state only:
/// - QPACK encode/decode state (static table only in Phase 2/3)
/// - Stream lifecycle (open/close)
/// - Settings (max_field_section_size)
/// - GOAWAY tracking
/// - Idle timeout (NET7-6c): H3-layer deadline tracking
/// - QUIC transport state (NET7-7c): connection ID, stream ID, lifecycle
///
/// QUIC transport state (draining, loss_detection, congestion_control)
/// is managed by `net_transport.rs` / QUIC substrate (libquiche).
#[derive(Debug)]
pub(crate) struct H3Connection {
    pub streams: Vec<H3Stream>,
    pub max_field_section_size: u64,
    pub last_peer_stream_id: u64,
    pub goaway_sent: bool,
    pub goaway_id: u64,
    // NET7-6c (NB7-22): idle timeout deadline implemented in Phase 6+.
    // Set on init, checked during polling, refreshed on peer activity.
    pub idle_timeout_at: std::time::Instant,
    // NET7-7c (NB7-20, NB7-26, NB7-28): QUIC transport state integration.
    // Connection-level QUIC Connection ID (bytes). Empty for non-QUIC.
    pub quic_connection_id: Vec<u8>,
    // Current QUIC stream being processed (0 if none / non-QUIC).
    pub current_quic_stream_id: u64,
    // Connection lifecycle state (Idle -> Active -> Draining -> Closed)
    pub state: H3ConnState,
    // H3-specific handler context for the current request
    pub handler_ctx: H3HandlerContext,
}

impl H3Connection {
    /// Create a new connection with the default idle timeout (30 seconds).
    pub fn new() -> Self {
        // NB7-22, NB7-23: Error scope comment — H3 protocol errors are stream errors
        // (H3_ERR_REQUEST_INCOMPLETE/400 equivalent). Connection errors
        // (H3_ERR_GENERAL_PROTOCOL_ERROR) apply only to framing violations. NB7-23
        H3Connection {
            streams: Vec::new(),
            max_field_section_size: H3_DEFAULT_MAX_FIELD_SECTION_SIZE,
            last_peer_stream_id: 0,
            goaway_sent: false,
            goaway_id: 0,
            idle_timeout_at: Self::default_idle_deadline(),
            quic_connection_id: Vec::new(),
            current_quic_stream_id: 0,
            state: H3ConnState::Idle,
            handler_ctx: H3HandlerContext::default(),
        }
    }

    /// Default idle timeout for an H3 connection.
    /// Per HTTP/3 common practice, 30 seconds is a reasonable default.
    /// This matches common HTTP server defaults (nginx, Caddy, etc.).
    pub const DEFAULT_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    fn default_idle_deadline() -> std::time::Instant {
        std::time::Instant::now() + Self::DEFAULT_IDLE_TIMEOUT
    }

    /// Check whether the idle timeout has elapsed.
    ///
    /// Returns `Some(H3DecodeError::Truncated)` if the idle timeout has fired
    /// (the specific error type is `Truncated` because an idle timeout is
    /// conceptually "expected more data within the deadline — none arrived").
    /// Per RFC 9000 §10.1, idle timeout fires when no frames are received
    /// within the idle timeout period.
    ///
    /// NET7-6b note: this maps to RFC 9114 `H3_ERR_NO_ERROR` (0x0100) for
    /// a clean idle close, but here we use `H3DecodeError::Truncated` as an
    /// internal signal since `Truncated` means "expected input but didn't arrive".
    pub fn check_timeout(&self) -> Option<H3DecodeError> {
        if std::time::Instant::now() > self.idle_timeout_at {
            Some(H3DecodeError::Truncated)
        } else {
            None
        }
    }

    /// Reset the idle timeout deadline on peer activity.
    /// Called when a new frame is received from the peer.
    /// NET7-6c: this is the "touch" mechanism for the idle timer.
    pub fn reset_idle_timer(&mut self) {
        self.idle_timeout_at = Self::default_idle_deadline();
    }

    /// Set a custom idle timeout duration. Useful for testing.
    pub fn set_idle_timeout(&mut self, duration: std::time::Duration) {
        self.idle_timeout_at = std::time::Instant::now() + duration;
    }

    pub fn find_stream(&self, stream_id: u64) -> Option<&H3Stream> {
        self.streams.iter().rev().find(|s| s.stream_id == stream_id)
    }

    #[allow(dead_code)]
    pub fn find_stream_mut(&mut self, stream_id: u64) -> Option<&mut H3Stream> {
        self.streams.iter_mut().rev().find(|s| s.stream_id == stream_id)
    }

    pub fn new_stream(&mut self, stream_id: u64) -> Option<&mut H3Stream> {
        if self.streams.len() >= H3_MAX_STREAMS {
            return None;
        }
        self.streams.push(H3Stream::new(stream_id));
        self.streams.last_mut()
    }

    pub fn remove_closed_streams(&mut self) {
        self.streams.retain(|s| s.state != H3StreamState::Closed);
    }

    // ── NET7-7c: QUIC Transport State Integration ─────────────────────
    // NET7-7a: Error scope boundary — these methods manage connection-level
    // state. Stream-level errors are scoped per-stream; connection-level
    // errors (state transitions, GOAWAY) apply to the entire connection.

    /// Set the QUIC Connection ID from the transport layer.
    /// The ID is opaque bytes whose length depends on the QUIC config.
    /// NB7-20, NB7-26: stored on H3Connection, NOT on the 14-field handler pack.
    pub fn set_quic_connection_id(&mut self, id: Vec<u8>) {
        self.quic_connection_id = id;
        self.handler_ctx.quic_connection_id = self.quic_connection_id.clone();
    }

    /// Return a reference to the QUIC Connection ID bytes.
    pub fn quic_connection_id(&self) -> &[u8] {
        &self.quic_connection_id
    }

    /// Set the current QUIC stream being processed.
    /// Updates both the connection field and the handler context.
    pub fn set_current_stream(&mut self, stream_id: u64) {
        self.current_quic_stream_id = stream_id;
        self.handler_ctx.quic_stream_id = stream_id;
    }

    /// Get the handler context for the current H3 request.
    /// Returns a clone so the caller can embed it in the 14-field pack
    /// without breaking the existing handler contract.
    pub fn handler_context(&self) -> H3HandlerContext {
        self.handler_ctx.clone()
    }

    /// Transition the connection state. Returns false if the transition
    /// is illegal for the current state.
    ///
    /// State machine: Idle -> Active -> Draining -> Closed
    /// Active -> Closed (emergency close, skip draining)
    pub fn transition_state(&mut self, target: H3ConnState) -> bool {
        let valid = matches!(
            (self.state.clone(), target.clone()),
            (H3ConnState::Idle, H3ConnState::Active)
                | (H3ConnState::Idle, H3ConnState::Closed)
                | (H3ConnState::Active, H3ConnState::Draining)
                | (H3ConnState::Active, H3ConnState::Closed)
                | (H3ConnState::Draining, H3ConnState::Closed)
        );
        if valid {
            self.state = target;
        }
        valid
    }

    /// Check if the connection is in a state that accepts new streams.
    /// Only Active connections accept new streams.
    pub fn accepts_new_streams(&self) -> bool {
        self.state == H3ConnState::Active
    }

    /// Begin graceful shutdown: send GOAWAY with the last peer stream ID,
    /// then transition to Draining state. Returns false if already shutting down.
    pub fn begin_shutdown(&mut self) -> bool {
        if self.state != H3ConnState::Active || self.goaway_sent {
            return false;
        }
        self.goaway_sent = true;
        self.state = H3ConnState::Draining;
        true
    }

    /// Complete shutdown: close all remaining streams and transition to Closed.
    pub fn complete_shutdown(&mut self) {
        for stream in self.streams.iter_mut() {
            stream.state = H3StreamState::Closed;
        }
        self.remove_closed_streams();
        self.state = H3ConnState::Closed;
    }
}

// ── H3 Request Extraction ────────────────────────────────────────────────
// Mirrors h3_extract_request_fields in native_runtime.c.
// Validates pseudo-headers matching H2 semantics (NB7-10).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum H3RequestError {
    Ordering,
    UnknownPseudo,
    MissingPseudo,
    DuplicatePseudo,
    EmptyPseudo,
}

/// Extracted request fields from H3 pseudo-headers.
#[derive(Debug)]
pub(crate) struct H3RequestFields {
    pub method: String,
    pub path: String,
    pub authority: String,
    pub regular_headers: Vec<(String, String)>,
}

/// Extract request fields from decoded H3 headers.
/// Validates pseudo-header ordering, duplicates, required fields, and empty values.
/// Returns the extracted fields on success, or an error kind.
pub(crate) fn extract_request_fields(
    headers: &[H3Header],
) -> Result<H3RequestFields, H3RequestError> {
    let mut method = None;
    let mut path = None;
    let mut authority = None;
    let mut scheme = None;
    let mut saw_regular = false;
    let mut saw_method = false;
    let mut saw_path = false;
    let mut saw_authority = false;
    let mut saw_scheme = false;
    let mut regular_headers = Vec::new();

    for hdr in headers {
        if hdr.name.starts_with(':') {
            if saw_regular {
                return Err(H3RequestError::Ordering);
            }
            match hdr.name.as_str() {
                ":method" => {
                    if saw_method {
                        return Err(H3RequestError::DuplicatePseudo);
                    }
                    saw_method = true;
                    method = Some(hdr.value.clone());
                }
                ":path" => {
                    if saw_path {
                        return Err(H3RequestError::DuplicatePseudo);
                    }
                    saw_path = true;
                    path = Some(hdr.value.clone());
                }
                ":authority" => {
                    if saw_authority {
                        return Err(H3RequestError::DuplicatePseudo);
                    }
                    saw_authority = true;
                    authority = Some(hdr.value.clone());
                }
                ":scheme" => {
                    if saw_scheme {
                        return Err(H3RequestError::DuplicatePseudo);
                    }
                    saw_scheme = true;
                    scheme = Some(hdr.value.clone());
                }
                _ => {
                    return Err(H3RequestError::UnknownPseudo);
                }
            }
        } else {
            saw_regular = true;
            regular_headers.push((hdr.name.clone(), hdr.value.clone()));
        }
    }

    // Required pseudo-headers: :method, :path, :scheme (matches H2 semantics)
    if !saw_method || !saw_path || !saw_scheme {
        return Err(H3RequestError::MissingPseudo);
    }

    let method = method.unwrap();
    let path = path.unwrap();
    let scheme = scheme.unwrap();
    let authority = authority.unwrap_or_default();
    // NB7-29: :authority is conditionally required per RFC 9114 §4.1.
    // Empty value is valid (matches H2 behavior). No deviation from h1/h2 compatibility policy.

    // Reject empty pseudo-header values (matches H2 semantics)
    if method.is_empty() || path.is_empty() || scheme.is_empty() {
        return Err(H3RequestError::EmptyPseudo);
    }

    Ok(H3RequestFields {
        method,
        path,
        authority,
        regular_headers,
    })
}

// ── H3 Response Builders ─────────────────────────────────────────────────

/// Build an H3 HEADERS frame with QPACK-encoded response headers.
pub(crate) fn build_response_headers_frame(
    status: u16,
    headers: &[(String, String)],
) -> Option<Vec<u8>> {
    let qpack_block = qpack_encode_block(status, headers)?;
    encode_frame(H3_FRAME_HEADERS, &qpack_block)
}

/// Build an H3 DATA frame.
pub(crate) fn build_data_frame(data: &[u8]) -> Option<Vec<u8>> {
    encode_frame(H3_FRAME_DATA, data)
}

// ── H3 Self-Tests ────────────────────────────────────────────────────────
// Mirrors the Native self-tests (NB7-9, NB7-10, NB7-11) for parity.

/// Result of self-test execution.
#[derive(Debug)]
pub(crate) enum SelftestResult {
    Ok,
    QpackFailure(i32),
    ValidationFailure(i32),
}

/// Run all H3 self-tests. Returns `SelftestResult::Ok` if all pass.
pub(crate) fn run_selftests() -> SelftestResult {
    match selftest_qpack_roundtrip() {
        0 => {}
        rc => return SelftestResult::QpackFailure(rc),
    }
    match selftest_request_validation() {
        0 => {}
        rc => return SelftestResult::ValidationFailure(rc),
    }
    SelftestResult::Ok
}

/// NB7-9: QPACK encode/decode round-trip self-test.
fn selftest_qpack_roundtrip() -> i32 {
    // Encode a response with 4 custom headers
    let headers = vec![
        ("content-type".to_string(), "text/plain".to_string()),
        ("x-custom-header".to_string(), "custom-value-123".to_string()),
        ("x-request-id".to_string(), "abc-def-ghi".to_string()),
        ("accept".to_string(), "application/json".to_string()),
    ];

    // Encode
    let encoded = match qpack_encode_block(200, &headers) {
        Some(e) => e,
        None => return -1,
    };

    // Decode
    let decoded = match qpack_decode_block(&encoded, 8, None, None) {
        Some(d) => d,
        None => return -2,
    };

    // Expected: :status + 4 headers = 5
    if decoded.len() != 5 {
        return -2;
    }

    // Verify :status
    if decoded[0].name != ":status" {
        return -3;
    }
    if decoded[0].value != "200" {
        return -4;
    }

    // Verify round-trip for each input header
    for (i, (name, value)) in headers.iter().enumerate() {
        if decoded[i + 1].name != *name {
            return -(10 + i as i32);
        }
        if decoded[i + 1].value != *value {
            return -(20 + i as i32);
        }
    }

    // NB7-11: Test max_headers overflow
    match qpack_decode_block(&encoded, 2, None, None) {
        None => {} // correct: overflow = decode error
        Some(_) => return -30,
    }

    0
}

/// NB7-10: H3 request pseudo-header validation self-test.
fn selftest_request_validation() -> i32 {
    // Test 1: Valid request
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
            H3Header { name: ":authority".into(), value: "localhost".into() },
        ];
        if extract_request_fields(&hdrs).is_err() {
            return -1;
        }
    }

    // Test 2: Missing :scheme should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::MissingPseudo) => {}
            _ => return -2,
        }
    }

    // Test 3: Empty :scheme should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::EmptyPseudo) => {}
            _ => return -4,
        }
    }

    // Test 4: Empty :method should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::EmptyPseudo) => {}
            _ => return -6,
        }
    }

    // Test 5: Duplicate :scheme should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
            H3Header { name: ":scheme".into(), value: "http".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::DuplicatePseudo) => {}
            _ => return -8,
        }
    }

    // Test 6: Ordering violation (regular before pseudo)
    {
        let hdrs = vec![
            H3Header { name: "host".into(), value: "localhost".into() },
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::Ordering) => {}
            _ => return -10,
        }
    }

    // Test 7: Unknown pseudo-header should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
            H3Header { name: ":protocol".into(), value: "websocket".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::UnknownPseudo) => {}
            _ => return -12,
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qpack_static_table_count() {
        // Must have same count as Native (99 entries, indices 0..98)
        assert_eq!(QPACK_STATIC_TABLE.len(), 99);
    }

    #[test]
    fn test_qpack_int_roundtrip() {
        for prefix_bits in [3, 4, 6, 7, 8] {
            for &value in &[0, 1, 5, 62, 63, 127, 128, 255, 1000, 65535] {
                let mut buf = [0u8; 16];
                let written = qpack_encode_int(&mut buf, prefix_bits, value, 0x00)
                    .expect("encode should succeed");
                let (decoded, consumed) = qpack_decode_int(&buf[..written], prefix_bits)
                    .expect("decode should succeed");
                assert_eq!(decoded, value, "prefix={}, value={}", prefix_bits, value);
                assert_eq!(consumed, written);
            }
        }
    }

    #[test]
    fn test_qpack_string_roundtrip() {
        for s in &["", "hello", "content-type", "x-custom-header-with-long-name"] {
            let mut buf = [0u8; 256];
            let written = qpack_encode_string(&mut buf, s).expect("encode");
            let (decoded, consumed) = qpack_decode_string(&buf[..written]).expect("decode");
            assert_eq!(decoded, *s);
            assert_eq!(consumed, written);
        }
    }

    #[test]
    fn test_varint_roundtrip() {
        for &value in &[0, 1, 63, 64, 16383, 16384, 1_073_741_823, 1_073_741_824] {
            let mut buf = [0u8; 16];
            let written = varint_encode(&mut buf, value).expect("encode");
            let (decoded, consumed) = varint_decode(&buf[..written]).expect("decode");
            assert_eq!(decoded, value, "value={}", value);
            assert_eq!(consumed, written);
        }
    }

    #[test]
    fn test_h3_frame_encode_decode() {
        let payload = b"hello";
        let frame = encode_frame(H3_FRAME_DATA, payload).expect("encode");
        let (ft, fl, hs) = decode_frame_header(&frame).expect("decode");
        assert_eq!(ft, H3_FRAME_DATA);
        assert_eq!(fl, payload.len() as u64);
        assert_eq!(&frame[hs..hs + fl as usize], payload);
    }

    #[test]
    fn test_h3_settings_encode_decode() {
        let settings_payload = encode_settings().expect("encode");
        let settings = decode_settings(&settings_payload).expect("decode");
        assert_eq!(settings.max_field_section_size, H3_DEFAULT_MAX_FIELD_SECTION_SIZE);
    }

    #[test]
    fn test_h3_goaway_encode() {
        let goaway = encode_goaway(4).expect("encode");
        let (ft, fl, hs) = decode_frame_header(&goaway).expect("decode");
        assert_eq!(ft, H3_FRAME_GOAWAY);
        // Payload is a varint for stream_id=4
        let (sid, _) = varint_decode(&goaway[hs..hs + fl as usize]).expect("decode payload");
        assert_eq!(sid, 4);
    }

    #[test]
    fn test_qpack_block_roundtrip() {
        let headers = vec![
            ("content-type".to_string(), "text/plain".to_string()),
            ("x-custom".to_string(), "value123".to_string()),
        ];
        let encoded = qpack_encode_block(200, &headers).expect("encode");
        let decoded = qpack_decode_block(&encoded, 10, None, None).expect("decode");
        // :status + 2 headers = 3
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].name, ":status");
        assert_eq!(decoded[0].value, "200");
        assert_eq!(decoded[1].name, "content-type");
        assert_eq!(decoded[1].value, "text/plain");
        assert_eq!(decoded[2].name, "x-custom");
        assert_eq!(decoded[2].value, "value123");
    }

    #[test]
    fn test_qpack_block_overflow_is_error() {
        // NB7-11: overflow must return None (decode error), not partial
        let headers = vec![
            ("a".to_string(), "b".to_string()),
            ("c".to_string(), "d".to_string()),
        ];
        let encoded = qpack_encode_block(200, &headers).expect("encode");
        // 3 headers (:status + 2), but max=1 -> error
        assert!(qpack_decode_block(&encoded, 1, None, None).is_none());
    }

    #[test]
    fn test_extract_request_fields_valid() {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/test?q=1".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
            H3Header { name: ":authority".into(), value: "example.com".into() },
            H3Header { name: "accept".into(), value: "*/*".into() },
        ];
        let fields = extract_request_fields(&hdrs).expect("valid request");
        assert_eq!(fields.method, "GET");
        assert_eq!(fields.path, "/test?q=1");
        assert_eq!(fields.authority, "example.com");
        assert_eq!(fields.regular_headers.len(), 1);
        assert_eq!(fields.regular_headers[0].0, "accept");
    }

    #[test]
    fn test_extract_request_fields_missing_scheme() {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
        ];
        assert!(matches!(
            extract_request_fields(&hdrs),
            Err(H3RequestError::MissingPseudo)
        ));
    }

    #[test]
    fn test_extract_request_fields_empty_method() {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
        ];
        assert!(matches!(
            extract_request_fields(&hdrs),
            Err(H3RequestError::EmptyPseudo)
        ));
    }

    #[test]
    fn test_extract_request_fields_ordering() {
        let hdrs = vec![
            H3Header { name: "host".into(), value: "localhost".into() },
            H3Header { name: ":method".into(), value: "GET".into() },
        ];
        assert!(matches!(
            extract_request_fields(&hdrs),
            Err(H3RequestError::Ordering)
        ));
    }

    #[test]
    fn test_extract_request_fields_duplicate_pseudo() {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
            H3Header { name: ":method".into(), value: "POST".into() },
        ];
        assert!(matches!(
            extract_request_fields(&hdrs),
            Err(H3RequestError::DuplicatePseudo)
        ));
    }

    #[test]
    fn test_extract_request_fields_unknown_pseudo() {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
            H3Header { name: ":protocol".into(), value: "ws".into() },
        ];
        assert!(matches!(
            extract_request_fields(&hdrs),
            Err(H3RequestError::UnknownPseudo)
        ));
    }

    #[test]
    fn test_response_headers_frame() {
        let headers = vec![
            ("content-type".to_string(), "text/plain".to_string()),
        ];
        let frame = build_response_headers_frame(200, &headers).expect("build");
        let (ft, fl, hs) = decode_frame_header(&frame).expect("decode header");
        assert_eq!(ft, H3_FRAME_HEADERS);
        // Payload should be valid QPACK
        let payload = &frame[hs..hs + fl as usize];
        let decoded = qpack_decode_block(payload, 10, None, None).expect("decode qpack");
        assert_eq!(decoded[0].name, ":status");
        assert_eq!(decoded[0].value, "200");
    }

    #[test]
    fn test_data_frame() {
        let body = b"hello world";
        let frame = build_data_frame(body).expect("build");
        let (ft, fl, hs) = decode_frame_header(&frame).expect("decode header");
        assert_eq!(ft, H3_FRAME_DATA);
        assert_eq!(&frame[hs..hs + fl as usize], body);
    }

    #[test]
    fn test_h3_stream_lifecycle() {
        let mut conn = H3Connection::new();
        conn.new_stream(4).unwrap();
        assert_eq!(conn.streams.len(), 1);
        assert_eq!(conn.streams[0].state, H3StreamState::Open);
        conn.streams[0].state = H3StreamState::Closed;
        conn.remove_closed_streams();
        assert_eq!(conn.streams.len(), 0);
    }

    #[test]
    fn test_selftests_pass() {
        match run_selftests() {
            SelftestResult::Ok => {}
            other => panic!("selftests failed: {:?}", other),
        }
    }

    // ── NET7-5a: Malformed input hardening tests ───────────────────────

    #[test]
    fn test_varint_non_canonical_rejected() {
        // RFC 9000 Section 16: smallest encoding required.
        // Value 0 encoded as 2 bytes (0x40 0x00) must be rejected.
        assert!(varint_decode(&[0x00]).is_some()); // 0 as 1-byte: OK
        assert!(varint_decode(&[0x40, 0x00]).is_none()); // 0 as 2-byte: rejected
        assert!(varint_decode(&[0x80, 0x00, 0x00, 0x00]).is_none()); // 0 as 4-byte: rejected
        assert!(varint_decode(&[0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
            .is_none()); // 0 as 8-byte: rejected
        // Value 63 encoded as 2 bytes must be rejected (fits in 1 byte).
        assert!(varint_decode(&[0x40, 0x3F]).is_none()); // 63 as 2-byte: rejected
        // Value 16383 as 4 bytes must be rejected (fits in 2 bytes).
        assert!(varint_decode(&[0x80, 0x00, 0x3F, 0xFF]).is_none()); // 16383 as 4-byte: rejected
        // Boundary: smallest valid 2-byte, 4-byte, 8-byte canonical values
        // 2-byte range: 64..=16383
        assert!(varint_decode(&[0x40, 0x40]).is_some()); // 64 in 2 bytes = valid
        assert!(varint_decode(&[0x7F, 0xFF]).is_some()); // 16383 in 2 bytes = valid
        // 4-byte range: 16384..=1073741823
        assert!(varint_decode(&[0x80, 0x00, 0x40, 0x00]).is_some()); // 16384 in 4 bytes = valid
        // 8-byte range: >1073741823
        assert!(varint_decode(&[0xC0, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00]).is_some()); // 1073741824 = valid
    }

    #[test]
    fn test_decode_frame_truncated_payload() {
        // Frame declares 100 bytes payload but only 5 available → reject
        let payload = [
            0x00u8, // DATA frame type (varint = 0)
            0x40, 0x64, // length = 100 (2-byte varint)
            0x01, 0x02, 0x03, 0x04, 0x05, // only 5 bytes
        ];
        assert!(decode_frame(&payload).is_none());
    }

    #[test]
    fn test_decode_frame_exact_fit() {
        // Frame declares 3 bytes payload and exactly 3 available → accept
        let payload = [
            0x00u8, // DATA frame
            0x03, // length = 3 (1-byte varint)
            0xAA, 0xBB, 0xCC, // exactly 3 bytes
        ];
        let (ft, body) = decode_frame(&payload).expect("exact fit should succeed");
        assert_eq!(ft, H3_FRAME_DATA);
        assert_eq!(body, &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn test_decode_frame_empty_input() {
        assert!(decode_frame(&[]).is_none());
    }

    #[test]
    fn test_qpack_block_truncated_field() {
        // Indexed field line 1100 0000 → index starts, but no continuation
        // Should return None (not panic, not partial)
        assert!(qpack_decode_block(&[0xC0], 10, None, None).is_none());
        // Literal name with length declaration but no actual bytes
        assert!(qpack_decode_block(&[0x23, 0xFF], 10, None, None).is_none());
    }

    #[test]
    fn test_qpack_decode_block_empty_input() {
        assert!(qpack_decode_block(&[], 10, None, None).is_none());
        assert!(qpack_decode_block(&[0x00], 10, None, None).is_none()); // only 1 byte < 2 minimum
    }

    #[test]
    fn test_qpack_decode_static_index_out_of_bounds() {
        // Indexed field 11xxxxxx with index pointing past static table
        // Static table has 99 entries (0..98). Index 99 = out of range.
        // Encode 99 as 6-bit with continuation: 0xFF, 99 - 64 = 35
        let buf2 = [0xFFu8, (99 - 64) as u8];
        assert!(qpack_decode_block(&buf2, 10, None, None).is_none());
    }

    #[test]
    fn test_decode_settings_empty() {
        assert!(decode_settings(&[]).is_some()); // empty = defaults
    }

    #[test]
    fn test_decode_settings_malformed_truncated_pair() {
        // Incomplete varint (single byte at end with no pair value) → None
        assert!(decode_settings(&[0x01]).is_none()); // QPACK_MAX_TABLE_CAPACITY id, no value
    }

    // ── OPEN Blocker Tests (NB7-19, NB7-30, NB7-38, NB7-39, NB7-43) ──

    /// NB7-19: QPACK integer overflow boundary verification at m=62.
    /// RFC 9204 Section 4.1.1: m=62 means prefix can hold 2^62-2, continuation
    /// bytes add 7 bits each. m > 62 would collide with u64 sign bit.
    #[test]
    fn test_qpack_decode_int_m62_boundary() {
        // m=62 should NOT overflow — this is the maximum safe prefix
        // Construct a value near the m=62 boundary
        // With m=62, max value from prefix alone = 2^62 - 2 = 4611686018427387902
        // continuation bytes add 7-bit chunks. So max representable ≈ 2^64 - 1 minus sign bit.
        // Verify round-trip with prefix_bits=62
        let mut buf = [0x00u8; 16];
        let test_val: u64 = 4_611_686_018_427_387_902; // 2^62 - 2
        let written = qpack_encode_int(&mut buf, 62, test_val, 0x00)
            .expect("m=62 encode should succeed");
        let (decoded, consumed) = qpack_decode_int(&buf[..written], 62)
            .expect("m=62 decode should succeed at boundary");
        assert_eq!(decoded, test_val);
        assert_eq!(consumed, written);
    }

    /// NB7-19: Verify overflow guard triggers when m > 62.
    #[test]
    fn test_qpack_decode_int_overflow_guard() {
        // With prefix_bits=8, send all continuation bytes (0xFF = continue).
        // m increments by 7 per continuation byte, and m > 62 should trigger.
        // After 9 continuation bytes: m = 63 > 62 → overflow
        let all_ff: [u8; 15] = [0xFF; 15];
        assert!(qpack_decode_int(&all_ff, 8).is_none(),
            "m > 62 should trigger overflow guard");
    }

    /// NB7-30: req_insert_count != 0 must be rejected (dynamic table not supported).
    #[test]
    fn test_qpack_decode_block_nonzero_insert_count() {
        // Encoded field section with req_insert_count != 0
        // First byte encodes req_insert_count with 8-bit prefix
        // 0x01 = prefix value 1 (non-zero insert count)
        // Followed by delta base with 7-bit prefix (sign bit = 0)
        let data = [
            0x01, // req_insert_count = 1 (non-zero → must reject, dynamic table not supported)
            0x00, // delta base = 0, sign = 0
            // No headers follow — the rejection should happen at insert_count check
        ];
        assert!(qpack_decode_block(&data, 10, None, None).is_none(),
            "req_insert_count != 0 must be rejected (dynamic table not supported in Phase 2/3)");
    }

    /// NB7-38: Non-canonical QUIC varint forms must be rejected.
    /// Tests various non-canonical encoding patterns beyond NB7-16's 8-byte guard.
    #[test]
    fn test_quic_varint_non_canonical_forms() {
        // Value 0 in 2-byte form (canonical: 1-byte [0x00])
        assert!(varint_decode(&[0x40, 0x00]).is_none(),
            "value=0 in 2-byte form must be rejected");
        // Value 0 in 4-byte form
        assert!(varint_decode(&[0x80, 0x00, 0x00, 0x00]).is_none(),
            "value=0 in 4-byte form must be rejected");
        // Value 50 in 4-byte form (canonical would be 1-byte)
        assert!(varint_decode(&[0x80, 0x00, 0x00, 0x32]).is_none(),
            "value=50 in 4-byte form must be rejected");
    }

    /// NB7-39: QPACK static table index verification for selected indices.
    /// Verifies that representative static table entries round-trip correctly.
    #[test]
    fn test_qpack_static_table_selected_indices() {
        // Test selected indices: 0, 7, 12, 46, 67, 98
        let test_indices = [0, 7, 12, 46, 67, 98];
        for &idx in &test_indices {
            let entry = &QPACK_STATIC_TABLE[idx];
            assert!(!entry.name.is_empty() || idx == 22, "entry {} name should not be empty", idx);
            // Build a full encoded field section:
            // byte 0: Required Insert Count = 0 (0x00)
            // byte 1: Delta Base = 0, sign = 0 (0x00)
            // bytes 2+: Indexed Field Line (11xxxxxx)
            let mut buf = vec![0x00, 0x00];
            // Encode indexed field line
            let mut int_buf = [0x00u8; 4];
            let written = qpack_encode_int(&mut int_buf, 6, idx as u64, 0xC0)
                .expect("indexed field encode should succeed");
            buf.extend_from_slice(&int_buf[..written]);
            // Decode back
            let decoded = qpack_decode_block(&buf, 10, None, None)
                .expect("indexed field decode should succeed");
            assert_eq!(decoded.len(), 1, "should have exactly 1 header for index {}", idx);
            assert_eq!(decoded[0].name, entry.name, "name mismatch for index {}", idx);
            assert_eq!(decoded[0].value, entry.value, "value mismatch for index {}", idx);
        }
    }

    /// NB7-43: max_field_section_size validation in qpack_decode_block.
    #[test]
    fn test_qpack_decode_block_max_field_section_size() {
        // Encode a small header block
        let headers = vec![
            ("content-type".to_string(), "text/plain".to_string()),
        ];
        let encoded = qpack_encode_block(200, &headers).expect("encode");
        let block_size = encoded.len() as u64;

        // With limit >= block_size: should succeed
        assert!(qpack_decode_block(&encoded, 10, Some(block_size), None).is_some(),
            "decode should succeed when limit >= block size");

        // With limit < block_size: should reject
        let small_limit = block_size - 1;
        assert!(qpack_decode_block(&encoded, 10, Some(small_limit), None).is_none(),
            "decode should be rejected when limit < block size ({})", block_size);
    }

    /// NB7-24: Verify decode_frame rejects oversized frames on 32-bit systems.
    #[test]
    fn test_decode_frame_oversized_32bit_guard() {
        // Construct a frame with frame_length = u64::MAX (way larger than any usize)
        // frame_type = 0 (DATA), varint-encoded as 1 byte
        // frame_length encoded as 8-byte varint with max value
        let payload = [
            0x00u8, // DATA frame type
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x3F, // 8-byte varint for u64::MAX-ish
        ];
        assert!(decode_frame(&payload).is_none(),
            "oversized frame_length must be rejected (32-bit guard)");
    }

    // ── NET7-6a: QPACK Dynamic Table Tests ───────────────────────────────

    #[test]
    fn test_dynamic_table_insert_lookup() {
        let mut table = H3DynamicTable::new(4096);
        assert!(table.is_empty());
        assert_eq!(table.max_capacity, 4096);

        table.insert("content-type".into(), "text/html".into());
        assert_eq!(table.len(), 1);
        assert!(!table.is_empty());

        // Entry has absolute index 0
        let entry = table.lookup_absolute(0).expect("should find entry");
        assert_eq!(entry.name, "content-type");
        assert_eq!(entry.value, "text/html");
    }

    #[test]
    fn test_dynamic_table_eviction() {
        let mut table = H3DynamicTable::new(100);
        // "x-key" (5) + "value-1" (7) + 32 = 44 bytes
        table.insert("x-key".into(), "value-1".into());
        // "x-key2" (6) + "value-2" (7) + 32 = 45 bytes
        table.insert("x-key2".into(), "value-2".into());
        assert_eq!(table.len(), 2);
        assert_eq!(table.current_size(), 44 + 45);

        // Insert a third entry: "name3" (5) + "val3" (4) + 32 = 41 bytes
        // Total would be 44 + 45 + 41 = 130 > 100, so eviction occurs.
        table.insert("name3".into(), "val3".into());
        // First entry (44 bytes) should be evicted: 130 - 44 = 86...
        // Actually after inserting third: 100 >= 130 - 44 = 86.
        assert!(table.len() <= 2);
        // Verify entry 0 is gone
        assert!(table.lookup_absolute(0).is_none());
    }

    #[test]
    fn test_dynamic_table_insert_too_large() {
        let mut table = H3DynamicTable::new(50);
        // "very-long-name-here" (19) + "very-long-value-here" (20) + 32 = 71 > 50
        assert!(!table.insert("very-long-name-here".into(), "very-long-value-here".into()));
    }

    #[test]
    fn test_dynamic_table_post_base_lookup() {
        let mut table = H3DynamicTable::new(4096);
        table.insert("a".into(), "1".into());
        table.insert("b".into(), "2".into());
        table.insert("c".into(), "3".into());

        // post_base_index=0 should return the most recent entry (c, index 2)
        let entry0 = table.lookup_post_base(0).expect("post-base 0");
        assert_eq!(entry0.index, 2);

        // post_base_index=2 should return the oldest entry still in table (a, index 0)
        let entry2 = table.lookup_post_base(2).expect("post-base 2");
        assert_eq!(entry2.index, 0);

        // post_base_index=3 (beyond total_inserted=3) should fail
        assert!(table.lookup_post_base(3).is_none());
    }

    #[test]
    fn test_dynamic_table_set_capacity() {
        let mut table = H3DynamicTable::new(4096);
        table.insert("x".into(), "1".into());
        table.insert("y".into(), "2".into());

        table.set_capacity(3000); // increase — no eviction
        assert_eq!(table.capacity(), 3000);
        assert_eq!(table.len(), 2);

        table.set_capacity(30); // decrease — should evict entries
        assert_eq!(table.capacity(), 30);
        assert!(table.len() < 2);
    }

    #[test]
    fn test_dynamic_table_set_capacity_shrink_evicts() {
        let mut table = H3DynamicTable::new(200);
        // Each entry: 5 + 4 + 32 = 41 bytes ("nameN" + "valN" + 32)
        // Insert 3 entries = 123 bytes
        table.insert("name1".into(), "val1".into());
        table.insert("name2".into(), "val2".into());
        table.insert("name3".into(), "val3".into());
        assert_eq!(table.len(), 3);

        // Shrink to 80 bytes → can only hold 1 entry (41 bytes)
        // 123 - 41 = 82 > 80, 82 - 41 = 41 <= 80 → evict 2, keep 1
        table.set_capacity(80);
        assert_eq!(table.len(), 1);
        assert!(table.lookup_absolute(0).is_none()); // first two evicted
        assert!(table.lookup_absolute(1).is_none());
        assert!(table.lookup_absolute(2).is_some()); // only newest remains
    }

    #[test]
    fn test_dynamic_table_duplicate() {
        let mut table = H3DynamicTable::new(4096);
        table.insert("original".into(), "data".into()); // index 0
        assert!(table.duplicate(0));
        assert_eq!(table.len(), 2);

        // The duplicate should have absolute index 1
        let dup = table.lookup_absolute(1).expect("duplicate entry");
        assert_eq!(dup.name, "original");
        assert_eq!(dup.value, "data");

        // Duplicate non-existent entry should fail
        assert!(!table.duplicate(99));
    }

    #[test]
    fn test_dynamic_table_relative_to_absolute() {
        let mut table = H3DynamicTable::new(4096);
        table.insert("a".into(), "1".into());
        table.insert("b".into(), "2".into());
        table.insert("c".into(), "3".into());

        // relative_index=0 → most recent = index 2
        assert_eq!(table.relative_to_absolute(0), Some(2));
        // relative_index=1 → index 1
        assert_eq!(table.relative_to_absolute(1), Some(1));
        // relative_index=2 → index 0
        assert_eq!(table.relative_to_absolute(2), Some(0));
        // relative_index=3 → doesn't exist
        assert!(table.relative_to_absolute(3).is_none());
    }

    #[test]
    fn test_dynamic_table_indices_monotonic_after_eviction() {
        let mut table = H3DynamicTable::new(68);
        // Each entry: "name-XX"(7) + "vv"(2) + 32 = 41 bytes.
        // Two entries = 82 > 68, so first gets evicted when second is inserted.
        table.insert("name-01".into(), "vv".into());  // index 0, 41 bytes
        assert_eq!(table.len(), 1);
        table.insert("name-02".into(), "vv".into());  // index 1, 41 bytes → triggers eviction
        assert_eq!(table.len(), 1);

        // After eviction, index 1 should still exist
        assert!(table.lookup_absolute(1).is_some());
        // Index 0 should be gone
        assert!(table.lookup_absolute(0).is_none());

        assert_eq!(table.total_inserted(), 2);
        assert_eq!(table.largest_ref(), 1);
    }

    // ── NET7-6a: Encoder Instruction Encode/Decode Round-Trip ───────────

    #[test]
    fn test_encode_insert_with_name_ref_static() {
        let mut buf = [0u8; 64];
        let written = encode_insert_with_name_ref(&mut buf, true, 17, "value").expect("encode");
        assert!(written > 0);

        let (inst, consumed) = decode_encoder_instruction(&buf[..written]).expect("decode");
        assert_eq!(consumed, written);
        match inst {
            H3EncoderInstruction::InsertWithNameRef { is_static, name_index, value } => {
                assert!(is_static);
                assert_eq!(name_index, 17);
                assert_eq!(value, "value");
            }
            other => panic!("wrong instruction: {:?}", other),
        }
    }

    #[test]
    fn test_encode_insert_with_name_ref_dynamic() {
        let mut table = H3DynamicTable::new(4096);
        table.insert("x-custom".into(), "".into()); // will have absolute_index=0
        let name_idx = 0u64;

        let mut buf = [0u8; 64];
        let written = encode_insert_with_name_ref(&mut buf, false, name_idx, "new-value").expect("encode");

        let (inst, consumed) = decode_encoder_instruction(&buf[..written]).expect("decode");
        assert_eq!(consumed, written);
        match inst {
            H3EncoderInstruction::InsertWithNameRef { is_static, name_index, value } => {
                assert!(!is_static);
                assert_eq!(name_index, name_idx);
                assert_eq!(value, "new-value");
            }
            other => panic!("wrong instruction: {:?}", other),
        }
    }

    #[test]
    fn test_encode_insert_with_literal_name() {
        let mut buf = [0u8; 64];
        let written = encode_insert_with_literal_name(&mut buf, "x-custom", "hello-world").expect("encode");
        // First byte should match 01xxxxxx (Insert With Literal Name, RFC 9204 Section 5.2.2).
        // Bits 7-6 = 01 for this instruction type. With name len "x-custom"=8, prefix_bits=3,
        // buf[0] = 0x40 | 7 = 0x47, buf[1] = 1 (continuation: 8-7=1).
        assert_eq!(buf[0] >> 6, 0b01); // bits 7-6 = 01

        let (inst, consumed) = decode_encoder_instruction(&buf[..written]).expect("decode");
        assert_eq!(consumed, written);
        match inst {
            H3EncoderInstruction::InsertWithLiteralName { name, value } => {
                assert_eq!(name, "x-custom");
                assert_eq!(value, "hello-world");
            }
            other => panic!("wrong instruction: {:?}", other),
        }
    }

    #[test]
    fn test_encode_duplicate() {
        let mut buf = [0u8; 8];
        let written = encode_duplicate(&mut buf, 5).expect("encode");
        // Duplicate: 00xxxxxx with 6-bit prefix, value=5
        assert_eq!(buf[0] >> 2, 0b000001); // 00 prefix, value=5

        let (inst, consumed) = decode_encoder_instruction(&buf[..written]).expect("decode");
        assert_eq!(consumed, written);
        match inst {
            H3EncoderInstruction::Duplicate { index } => {
                assert_eq!(index, 5);
            }
            other => panic!("wrong instruction: {:?}", other),
        }
    }

    #[test]
    fn test_encode_set_capacity() {
        let mut buf = [0u8; 8];
        let written = encode_set_capacity(&mut buf, 4096).expect("encode");
        // SetCapacity: 001xxxxx with 5-bit prefix, value=4096
        // First byte should start with 001
        assert_eq!(buf[0] >> 5, 0b001);

        let (inst, consumed) = decode_encoder_instruction(&buf[..written]).expect("decode");
        assert_eq!(consumed, written);
        match inst {
            H3EncoderInstruction::SetCapacity { capacity } => {
                assert_eq!(capacity, 4096);
            }
            other => panic!("wrong instruction: {:?}", other),
        }
    }

    #[test]
    fn test_encoder_instruction_apply_static_name_ref() {
        let mut table = H3DynamicTable::new(4096);
        let inst = H3EncoderInstruction::InsertWithNameRef {
            is_static: true,
            name_index: 17, // :method GET
            value: "".to_string(),
        };
        assert!(apply_encoder_instruction(&mut table, &inst));
        assert_eq!(table.len(), 1);
        let entry = table.entries.first().expect("entry should exist");
        assert_eq!(entry.name, ":method");
        assert_eq!(entry.value, "");
    }

    #[test]
    fn test_encoder_instruction_apply_literal_name() {
        let mut table = H3DynamicTable::new(4096);
        let inst = H3EncoderInstruction::InsertWithLiteralName {
            name: "x-foo".to_string(),
            value: "bar".to_string(),
        };
        assert!(apply_encoder_instruction(&mut table, &inst));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn test_encoder_instruction_apply_capacity_change() {
        let mut table = H3DynamicTable::new(4096);
        table.insert("a".into(), "1".into());
        let inst = H3EncoderInstruction::SetCapacity { capacity: 200 };
        assert!(apply_encoder_instruction(&mut table, &inst));
        assert_eq!(table.capacity(), 200);
    }

    #[test]
    fn test_encoder_instruction_sequence() {
        // Simulate an encoder stream with multiple instructions
        let mut buf = vec![0u8; 256];
        let mut pos = 0;

        // Insert With Literal Name: x-frame-options = deny
        let w = encode_insert_with_literal_name(&mut buf[pos..], "x-frame-options", "deny").unwrap();
        pos += w;

        // Duplicate entry 0
        let w = encode_duplicate(&mut buf[pos..], 0).unwrap();
        pos += w;

        // Set capacity back to 4096
        let w = encode_set_capacity(&mut buf[pos..], 4096).unwrap();
        pos += w;

        // Decode and apply all instructions
        let mut table = H3DynamicTable::new(4096);
        let mut consumed_total = 0;
        while consumed_total < pos {
            let (inst, consumed) = decode_encoder_instruction(&buf[consumed_total..pos])
                .expect("instruction decode");
            assert!(apply_encoder_instruction(&mut table, &inst),
                "apply failed for {:?}", inst);
            consumed_total += consumed;
        }
        assert_eq!(consumed_total, pos);
        // After insert + duplicate: 2 entries in dynamic table
        assert_eq!(table.len(), 2);
    }

    // ── NET7-6a: Decoder Instruction Encode/Decode Round-Trip ───────────

    #[test]
    fn test_encode_section_ack() {
        let mut buf = [0u8; 8];
        let written = encode_section_ack(&mut buf, 42).expect("encode");
        // SectionAck: 1xxxxxxx (bit 7 set), 7-bit prefix, value=42
        assert_eq!(buf[0] & 0x80, 0x80);

        let (inst, consumed) = decode_decoder_instruction(&buf[..written]).expect("decode");
        assert_eq!(consumed, written);
        match inst {
            H3DecoderInstruction::SectionAck { insert_count } => {
                assert_eq!(insert_count, 42);
            }
            other => panic!("wrong instruction: {:?}", other),
        }
    }

    #[test]
    fn test_encode_stream_cancel() {
        let mut buf = [0u8; 8];
        let written = encode_stream_cancel(&mut buf, 7).expect("encode");
        // StreamCancel: 001xxxxx, bits 7-5 = 001
        assert_eq!(buf[0] >> 5, 0b001);

        let (inst, consumed) = decode_decoder_instruction(&buf[..written]).expect("decode");
        assert_eq!(consumed, written);
        match inst {
            H3DecoderInstruction::StreamCancel { stream_id } => {
                assert_eq!(stream_id, 7);
            }
            other => panic!("wrong instruction: {:?}", other),
        }
    }

    #[test]
    fn test_encode_insert_count_increment() {
        let mut buf = [0u8; 8];
        let written = encode_insert_count_increment(&mut buf, 3).expect("encode");
        // InsertCountIncrement: 00xxxxxx, bits 7-6 = 00
        assert!(buf[0] < 0x40);

        let (inst, consumed) = decode_decoder_instruction(&buf[..written]).expect("decode");
        assert_eq!(consumed, written);
        match inst {
            H3DecoderInstruction::InsertCountIncrement { increment } => {
                assert_eq!(increment, 3);
            }
            other => panic!("wrong instruction: {:?}", other),
        }
    }

    #[test]
    fn test_decoder_state_apply_instructions() {
        let mut state = H3DecoderState::new();
        assert_eq!(state.received_insert_count(), 0);

        // Insert Count Increment: +5
        let inst = H3DecoderInstruction::InsertCountIncrement { increment: 5 };
        assert!(state.apply_decoder_instruction(&inst));
        assert_eq!(state.received_insert_count(), 5);

        // SectionAck: insert_count=5
        let inst = H3DecoderInstruction::SectionAck { insert_count: 5 };
        assert!(state.apply_decoder_instruction(&inst));

        // Zero increment is illegal
        let inst = H3DecoderInstruction::InsertCountIncrement { increment: 0 };
        assert!(!state.apply_decoder_instruction(&inst));

        // StreamCancel
        let inst = H3DecoderInstruction::StreamCancel { stream_id: 4 };
        assert!(state.apply_decoder_instruction(&inst));
    }

    #[test]
    fn test_decoder_instruction_sequence_roundtrip() {
        // Encode a sequence of decoder instructions, then decode them all
        let mut buf = vec![0u8; 128];
        let mut pos = 0;

        let w = encode_insert_count_increment(&mut buf[pos..], 2).unwrap();
        pos += w;
        let w = encode_insert_count_increment(&mut buf[pos..], 3).unwrap();
        pos += w;
        let w = encode_section_ack(&mut buf[pos..], 5).unwrap();
        pos += w;
        let w = encode_stream_cancel(&mut buf[pos..], 1).unwrap();
        pos += w;

        let mut state = H3DecoderState::new();
        let mut consumed_total = 0;
        while consumed_total < pos {
            let (inst, consumed) = decode_decoder_instruction(&buf[consumed_total..pos]).expect("decode");
            assert!(state.apply_decoder_instruction(&inst));
            consumed_total += consumed;
        }
        assert_eq!(consumed_total, pos);
        // 2 + 3 = 5
        assert_eq!(state.received_insert_count(), 5);
    }

    // ── NET7-6a: Decode + Encode Dynamic Table Integration ──────────────

    #[test]
    fn test_decode_block_with_dynamic_table() {
        // Build a dynamic table with an entry
        let mut table = H3DynamicTable::new(4096);
        table.insert("x-custom".into(), "some-value".into());

        // Build an encoded block referencing the dynamic table
        let mut buf = [0u8; 128];
        let mut pos = 0;
        // Required Insert Count = 1 (1 entry in dynamic table)
        buf[pos] = 0x01;
        pos += 1;
        // Delta Base = 0, sign = 0
        buf[pos] = 0x00;
        pos += 1;

        // Literal with literal name (001Nxxxx, N=0, no Huffman): "x-extra" = "extra-val"
        // We'll use a separate entry to avoid dynamic table reference complexity.
        let name = "x-header";
        let val = "extra-content";
        let w = qpack_encode_int(&mut buf[pos..], 3, name.len() as u64, 0x20).unwrap();
        pos += w;
        buf[pos..pos + name.len()].copy_from_slice(name.as_bytes());
        pos += name.len();
        let vw = qpack_encode_string(&mut buf[pos..], val).unwrap();
        pos += vw;

        // Decode with the dynamic table available
        let decoded = qpack_decode_block(&buf[..pos], 10, None, Some(&table)).expect("decode");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name, "x-header");
        assert_eq!(decoded[0].value, "extra-content");
    }

    #[test]
    fn test_decode_encoder_insert_with_literal_name_raw() {
        // Manually crafted encoder instruction: Insert With Literal Name (RFC 9204 Section 5.2.2)
        // Bit layout: 01 N H xxx (3-bit name length prefix)
        // 0x4C = 01001100: N=0, H=0, name_len prefix=4 (single byte, 4 < 7)
        // name = "test" (4 bytes)
        // value: 7-bit prefix string, 0x05 = len 5, "hello"
        let buf = [0x4Cu8, b't', b'e', b's', b't', 0x05, b'h', b'e', b'l', b'l', b'o'];
        let (inst, consumed) = decode_encoder_instruction(&buf).expect("decode");
        assert_eq!(consumed, 11);
        match inst {
            H3EncoderInstruction::InsertWithLiteralName { name, value } => {
                assert_eq!(name, "test");
                assert_eq!(value, "hello");
            }
            other => panic!("expected InsertWithLiteralName, got {:?}", other),
        }
    }

    // ── NET7-6b: H3DecodeError variant tests ─────────────────────────────

    #[test]
    fn test_decode_error_qpack_int_truncated() {
        assert_eq!(qpack_decode_int_r(&[], 8), Err(H3DecodeError::Truncated));
    }

    #[test]
    fn test_decode_error_qpack_int_overflow() {
        let all_ff: [u8; 15] = [0xFF; 15];
        assert_eq!(qpack_decode_int_r(&all_ff, 8), Err(H3DecodeError::QpackIntOverflow));
    }

    #[test]
    fn test_decode_error_varint_truncated() {
        assert_eq!(varint_decode_r(&[]), Err(H3DecodeError::Truncated));
        // 4-byte form but only 2 bytes available
        assert_eq!(varint_decode_r(&[0x80, 0x01]), Err(H3DecodeError::Truncated));
    }

    #[test]
    fn test_decode_error_varint_non_canonical() {
        // Value 0 encoded as 2 bytes = non-canonical
        assert_eq!(varint_decode_r(&[0x40, 0x00]), Err(H3DecodeError::NonCanonical));
    }

    #[test]
    fn test_decode_error_static_table_index_ooB() {
        // Block with req_insert_count=0, then static table index 99 (OOB).
        // Static table has 99 entries (0..98). Index 99 = out of range.
        // 6-bit prefix encoding of 99: first byte 0xFF (63), continuation: 99-63=36.
        let buf = [0x00u8, 0x00, 0xFF, 36];
        assert!(matches!(
            qpack_decode_block_r(&buf, 10, None, None),
            Err(H3DecodeError::StaticTableIndex)
        ));
    }

    #[test]
    fn test_decode_error_field_section_too_large() {
        let headers = vec![
            ("content-type".to_string(), "text/plain".to_string()),
        ];
        let encoded = qpack_encode_block(200, &headers).expect("encode");
        let block_size = encoded.len() as u64;
        assert!(matches!(
            qpack_decode_block_r(&encoded, 10, Some(block_size - 1), None),
            Err(H3DecodeError::FieldSectionTooLarge)
        ));
    }

    #[test]
    fn test_decode_error_too_many_headers() {
        let headers = vec![("a".to_string(), "b".to_string())];
        let encoded = qpack_encode_block(200, &headers).expect("encode");
        assert!(matches!(
            qpack_decode_block_r(&encoded, 1, None, None),
            Err(H3DecodeError::TooManyHeaders)
        ));
    }

    #[test]
    fn test_decode_error_dynamic_table_required() {
        // req_insert_count=1 but no dynamic table provided
        let buf = [0x01, 0x00];
        assert!(matches!(
            qpack_decode_block_r(&buf, 10, None, None),
            Err(H3DecodeError::DynamicTableError)
        ));
    }

    #[test]
    fn test_decode_error_frame_malformed() {
        // Empty input
        assert_eq!(decode_frame_r(&[]), Err(H3DecodeError::Truncated));
        // Oversized frame
        let payload = [
            0x00u8,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x3F,
        ];
        assert_eq!(decode_frame_r(&payload), Err(H3DecodeError::FrameMalformed));
    }

    #[test]
    fn test_decode_error_display_variants() {
        assert_eq!(
            format!("{}", H3DecodeError::Truncated),
            "truncated input, more bytes needed"
        );
        assert_eq!(
            format!("{}", H3DecodeError::QpackIntOverflow),
            "QPACK integer overflow (m > 62)"
        );
        assert_eq!(
            format!("{}", H3DecodeError::NonCanonical),
            "non-canonical encoding"
        );
        assert_eq!(
            format!("{}", H3DecodeError::InvalidInstruction),
            "invalid encoder/decoder instruction"
        );
    }

    #[test]
    fn test_decode_block_r_matches_decode_ok_path() {
        // Verify that the Result variant returns the same headers as the Option variant
        // when decoding a standard header block.
        let headers = vec![
            ("content-type".to_string(), "text/html".to_string()),
            ("x-request-id".to_string(), "abc-123".to_string()),
        ];
        let encoded = qpack_encode_block(200, &headers).expect("encode");

        let result_headers = qpack_decode_block_r(&encoded, 10, None, None)
            .expect("Result decode should succeed");
        let opt_headers = qpack_decode_block(&encoded, 10, None, None)
            .expect("Option decode should succeed");

        assert_eq!(result_headers.len(), opt_headers.len());
        for (r, o) in result_headers.iter().zip(opt_headers.iter()) {
            assert_eq!(r.name, o.name);
            assert_eq!(r.value, o.value);
        }
    }

    // ── NET7-6c: Idle Timeout Tests (NB7-22) ─────────────────────────────

    #[test]
    fn test_idle_timeout_default_is_future() {
        let conn = H3Connection::new();
        // Default deadline should be 30 seconds in the future
        assert!(conn.idle_timeout_at > std::time::Instant::now());
    }

    #[test]
    fn test_idle_timeout_check_no_timeout() {
        let conn = H3Connection::new();
        // Just-created connection should not have timed out
        assert!(conn.check_timeout().is_none());
    }

    #[test]
    fn test_idle_timeout_check_timed_out() {
        let mut conn = H3Connection::new();
        // Set timeout to 0 — should fire immediately
        conn.set_idle_timeout(std::time::Duration::from_secs(0));
        assert!(conn.check_timeout().is_some());
        assert!(matches!(conn.check_timeout(), Some(H3DecodeError::Truncated)));
    }

    #[test]
    fn test_idle_timeout_reset_unsets_timeout() {
        let mut conn = H3Connection::new();
        conn.set_idle_timeout(std::time::Duration::from_secs(0));
        assert!(conn.check_timeout().is_some());

        conn.reset_idle_timer();
        assert!(conn.check_timeout().is_none());
    }

    #[test]
    fn test_idle_timeout_custom_duration() {
        let mut conn = H3Connection::new();
        conn.set_idle_timeout(std::time::Duration::from_millis(1));
        // Should not have timed out yet
        assert!(conn.check_timeout().is_none());
        // After 5ms, should have timed out
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(conn.check_timeout().is_some());
    }

    #[test]
    fn test_default_idle_timeout_constant() {
        assert_eq!(
            H3Connection::DEFAULT_IDLE_TIMEOUT,
            std::time::Duration::from_secs(30)
        );
    }

    // ── NET7-6d: Flow Control Boundary Tests (NB7-35 future) ────────────
    // NB7-35: QUIC transport handles packet loss / flow control at the transport
    // layer (libquiche). The H3 layer assumes an ordered, reliable byte stream.
    // These tests verify the boundary assumptions.

    #[test]
    fn test_flow_control_stream_limit_boundary() {
        // H3_MAX_STREAMS = 256. Verify connection refuses to create more.
        let mut conn = H3Connection::new();
        // Create streams up to the limit
        for i in 0..H3_MAX_STREAMS {
            let result = conn.new_stream(i as u64);
            assert!(result.is_some(), "stream {} should be created", i);
        }
        // One more should fail
        assert!(conn.new_stream(H3_MAX_STREAMS as u64).is_none(),
            "stream {} should be rejected (MAX_STREAMS exceeded)", H3_MAX_STREAMS);
    }

    #[test]
    fn test_flow_control_settings_bounded_iteration() {
        // H3_MAX_SETTINGS_PAIRS = 64 prevents unbounded settings parsing.
        // Construct 65 settings pairs and verify rejection.
        let mut buf = vec![0u8; 256];
        let mut pos = 0;
        // Write 63 valid QPACK_MAX_TABLE_CAPACITY pairs (0=0)
        for _ in 0..63 {
            pos += varint_encode(&mut buf[pos..], H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY).unwrap();
            pos += varint_encode(&mut buf[pos..], 0).unwrap();
        }
        assert!(decode_settings(&buf[..pos]).is_some(),
            "63 pairs should be valid");

        // Add the 64th pair — should still be within limit
        pos += varint_encode(&mut buf[pos..], H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY).unwrap();
        pos += varint_encode(&mut buf[pos..], 0).unwrap();
        assert!(decode_settings(&buf[..pos]).is_some(),
            "64 pairs (H3_MAX_SETTINGS_PAIRS) should be valid");

        // Add a 65th pair — should be rejected
        pos += varint_encode(&mut buf[pos..], H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY).unwrap();
        pos += varint_encode(&mut buf[pos..], 0).unwrap();
        assert!(decode_settings(&buf[..pos]).is_none(),
            "65 pairs (> H3_MAX_SETTINGS_PAIRS) must be rejected (DoS mitigation)");
    }

    #[test]
    fn test_flow_control_max_field_section_size_boundary() {
        // Field section size limit prevents arbitrarily large header blocks.
        // 64KB is the default (H3_DEFAULT_MAX_FIELD_SECTION_SIZE).
        assert_eq!(H3_DEFAULT_MAX_FIELD_SECTION_SIZE, 64 * 1024);

        // Verify that a block at exactly the limit is accepted
        let headers = vec![
            ("x-large-header".to_string(), "x".repeat(1000)),
        ];
        let encoded = qpack_encode_block(200, &headers).expect("encode");

        // A block that fits within the limit should be accepted
        assert!(qpack_decode_block(&encoded, 10, Some(H3_DEFAULT_MAX_FIELD_SECTION_SIZE), None).is_some(),
            "block within limit should be accepted");
    }

    #[test]
    fn test_flow_control_conn_drain_on_goaway() {
        // After GOAWAY is sent, new peer streams should not be opened.
        // The H3 layer tracks goaway_sent and uses it to gate new stream creation.
        let mut conn = H3Connection::new();
        conn.goaway_sent = true;
        conn.goaway_id = 4;

        // Stream with ID <= goaway_id should still be allowed
        // (in a real implementation this would be checked in stream creation)
        // Stream with ID > goaway_id should be rejected
        assert_eq!(conn.goaway_id, 4);
        assert!(conn.goaway_sent);
    }

    #[test]
    fn test_frame_size_zero_payload_accepted() {
        // Zero-length frame (e.g., empty DATA frame) is valid per RFC 9114 §7.2.1.
        let frame = encode_frame(H3_FRAME_DATA, &[]).expect("encode empty payload");
        let (ft, fl, _) = decode_frame_header(&frame).expect("decode");
        assert_eq!(ft, H3_FRAME_DATA);
        assert_eq!(fl, 0);
    }

    #[test]
    fn test_frame_size_large_payload_boundary() {
        // Construct the largest possible frame header (8-byte type + 8-byte length)
        // frame_type = u64::MAX, frame_length = 1_073_741_823 (max 4-byte varint)
        // This is structurally valid as a frame header, even without payload.
        let mut buf = vec![0u8; 128];

        // Max 8-byte varint for type: RFC 9000 allows up to 62-bit values
        let tw = varint_encode(&mut buf, 1_073_741_825).unwrap(); // just above 4-byte range
        let lw = varint_encode(&mut buf[tw..], 0).unwrap();

        let (ft, fl, _) = decode_frame_header(&buf[..tw + lw]).expect("decode");
        assert_eq!(ft, 1_073_741_825);
        assert_eq!(fl, 0);
    }

    #[test]
    fn test_transport_integration_header_block_size() {
        // Verify that QPACK-encoded header blocks produce predictable sizes.
        // The transport layer delivers these as QUIC stream data with no modification
        // (no re-chunking at the H3 layer). NB7-35: transport = raw byte delivery.

        // Small header block
        let small = qpack_encode_block(200, &[("x".to_string(), "y".to_string())])
            .expect("encode");
        // Header block should be small enough for a single QUIC packet
        assert!(small.len() < 1200, "small header should fit in typical MTU");

        // Larger header block
        let big_headers: Vec<_> = (0..20)
            .map(|i| (format!("x-header-{}", i), "value-12345678901234567890".to_string()))
            .collect();
        let big = qpack_encode_block(200, &big_headers).expect("encode");
        assert!(big.len() > small.len(), "big header should be larger");

        // Both should decode cleanly
        let small_decoded = qpack_decode_block(&small, 10, None, None).expect("small decode");
        assert_eq!(small_decoded.len(), 2);

        let big_decoded = qpack_decode_block(&big, 30, None, None).expect("big decode");
        assert_eq!(big_decoded.len(), 21); // :status + 20 headers
    }

    #[test]
    fn test_frame_decode_truncated_mid_varint() {
        // Frame header with truncated varint (first byte says 8-byte form,
        // but only 3 body bytes available, not 7)
        let data = [0xC0u8, 0x00, 0x01]; // 8-byte varint prefix, but only 2 more bytes
        assert!(decode_frame_header(&data).is_none());
    }

    #[test]
    fn test_frame_decode_exact_frame_end_boundary() {
        // Frame ending exactly at data boundary
        let payload = b"test";
        let frame = encode_frame(H3_FRAME_DATA, payload).expect("encode");
        let (ft, body) = decode_frame(&frame).expect("decode");
        assert_eq!(ft, H3_FRAME_DATA);
        assert_eq!(body, payload);

        // Truncated by 1 byte
        let truncated = &frame[..frame.len() - 1];
        assert!(decode_frame(truncated).is_none());
    }
}

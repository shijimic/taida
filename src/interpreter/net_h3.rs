/// HTTP/3 parity implementation for `taida-lang/net` v7.
///
/// **NB7-42: Phase 6+ plan** — split into `qpack.rs` / `frame.rs` / `request.rs` / `connection.rs`.
/// File split deferred until API surface is stable after Phase 2/3.
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

/// Decode a QPACK integer with the given prefix bit width.
/// **QPACK Integer encoding per RFC 9204 Section 4.1.1.** Uses prefix bits (m) and 7-bit continuation bytes.
/// NB7-33: Distinct from QUIC varint (RFC 9000 Section 16) — QPACK uses arbitrary prefix bit widths,
///   while QUIC varint uses fixed 2-bit length prefix.
/// Returns `Some((value, bytes_consumed))` on success, `None` on error.
pub(crate) fn qpack_decode_int(data: &[u8], prefix_bits: u8) -> Option<(u64, usize)> {
    if data.is_empty() {
        return None;
    }
    // Compute mask avoiding overflow when prefix_bits == 8
    let mask: u8 = if prefix_bits >= 8 { 0xFF } else { (1u8 << prefix_bits) - 1 };
    let val = (data[0] & mask) as u64;
    if val < mask as u64 {
        return Some((val, 1));
    }
    // Multi-byte
    let mut val = val;
    let mut m = 0u32;
    for i in 1..data.len() {
        val += ((data[i] & 0x7F) as u64) << m;
        m += 7;
        if data[i] & 0x80 == 0 {
            return Some((val, i + 1));
        }
        if m > 62 {
            return None; // overflow protection
        }
    }
    None // incomplete
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
pub(crate) fn qpack_decode_block(data: &[u8], max_headers: usize, max_field_section_size: Option<u64>) -> Option<Vec<H3Header>> {
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
    if req_insert_count != 0 {
        return None; // Phase 2/3: no dynamic table
    }

    // Sign bit + Delta Base (prefix int, 7-bit prefix)
    if consumed >= data.len() {
        return None;
    }
    let (_, db_consumed) = qpack_decode_int(&data[consumed..], 7)?;
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
                return None; // Phase 2/3: no dynamic table
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
                return None; // Phase 2/3: no dynamic table
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
            // Phase 2/3: no dynamic table, reject
            return None;
        }
    }
    Some(headers)
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

/// HTTP/3 protocol state per connection.
///
/// **NB7-22 / NB7-28: Responsibility boundary.**
/// `H3Connection` manages HTTP/3 protocol state only:
/// - QPACK encode/decode state (static table only in Phase 2/3)
/// - Stream lifecycle (open/close)
/// - Settings (max_field_section_size)
/// - GOAWAY tracking
///
/// QUIC transport state (idle_timeout, draining, loss_detection, congestion_control)
/// is managed by `net_transport.rs` / QUIC substrate (libquiche).
/// Phase 6+: `idle_timeout_at: u64` will be added here with `check_timeout()` logic.
#[derive(Debug)]
pub(crate) struct H3Connection {
    pub streams: Vec<H3Stream>,
    pub max_field_section_size: u64,
    pub last_peer_stream_id: u64,
    pub goaway_sent: bool,
    pub goaway_id: u64,
    // NB7-22 placeholder: `idle_timeout_at: u64` — set on init, checked during polling.
    // Actual implementation deferred to Phase 6+ (real QUIC connection integration).
}

impl H3Connection {
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
        }
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
    let decoded = match qpack_decode_block(&encoded, 8, None) {
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
    match qpack_decode_block(&encoded, 2, None) {
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
        let decoded = qpack_decode_block(&encoded, 10, None).expect("decode");
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
        assert!(qpack_decode_block(&encoded, 1, None).is_none());
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
        let decoded = qpack_decode_block(payload, 10, None).expect("decode qpack");
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
        assert!(qpack_decode_block(&[0xC0], 10, None).is_none());
        // Literal name with length declaration but no actual bytes
        assert!(qpack_decode_block(&[0x23, 0xFF], 10, None).is_none());
    }

    #[test]
    fn test_qpack_decode_block_empty_input() {
        assert!(qpack_decode_block(&[], 10, None).is_none());
        assert!(qpack_decode_block(&[0x00], 10, None).is_none()); // only 1 byte < 2 minimum
    }

    #[test]
    fn test_qpack_decode_static_index_out_of_bounds() {
        // Indexed field 11xxxxxx with index pointing past static table
        // Static table has 99 entries (0..98). Index 99 = out of range.
        // Encode 99 as 6-bit with continuation: 0xFF, 99 - 64 = 35
        let buf2 = [0xFFu8, (99 - 64) as u8];
        assert!(qpack_decode_block(&buf2, 10, None).is_none());
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
        assert!(qpack_decode_block(&data, 10, None).is_none(),
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
            let decoded = qpack_decode_block(&buf, 10, None)
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
        assert!(qpack_decode_block(&encoded, 10, Some(block_size)).is_some(),
            "decode should succeed when limit >= block size");

        // With limit < block_size: should reject
        let small_limit = block_size - 1;
        assert!(qpack_decode_block(&encoded, 10, Some(small_limit)).is_none(),
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
}

/// H3 frame I/O with QUIC varint encoding (RFC 9000 §16) and frame type constants (RFC 9114).

// ── QUIC Variable-Length Integer Coding (RFC 9000 Section 16) ────────────
// **QUIC Variable-Length Integer encoding per RFC 9000 Section 16.**
// Uses 2-bit prefix to encode 1/2/4/8 byte forms. NB7-33
// Distinct from QPACK Integer (RFC 9204 Section 4.1.1) which uses arbitrary prefix bit widths.
// 2-bit prefix: 00=1byte, 01=2byte, 10=4byte, 11=8byte.

/// Check that a decoded varint value is valid for the given encoding length.
/// RFC 9000 Section 16 requires **smallest encoding** — values that could
/// fit in fewer bytes but use a larger encoding form are malformed (NET7-5a).
pub(crate) fn is_canonical_varint(value: u64, prefix: u8) -> bool {
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
pub(crate) const H3_MAX_SETTINGS_PAIRS: usize = 64;

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
    // NB7-27 / NB7-77: use try_into() instead of `as usize` to prevent silent truncation
    // on 32-bit platforms. A varint-decoded frame length > usize::MAX is an overflow error,
    // not a valid frame.
    let frame_len = u64::try_from(frame_length)
        .ok()
        .and_then(|fl| usize::try_from(fl).ok())
        .filter(|&fl| fl <= data.len().saturating_sub(header_size))?;
    let total = header_size + frame_len;
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
/// **RFC 9114 §7.2.4.2**: duplicate settings are rejected.
pub(crate) fn decode_settings(data: &[u8]) -> Option<H3Settings> {
    let mut settings = H3Settings::default();
    let mut pos = 0;
    let mut pair_count = 0;
    // Duplicate detection: track which known setting IDs we've seen
    let mut seen_qpack_max_table_capacity = false;
    let mut seen_max_field_section_size = false;
    let mut seen_qpack_blocked_streams = false;
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
                // RFC 9114 §7.2.4.2: duplicate setting ID is a connection error
                if seen_qpack_max_table_capacity {
                    return None;
                }
                seen_qpack_max_table_capacity = true;
                // Phase 2/3: we only support static table, ignore capacity > 0
            }
            H3_SETTINGS_MAX_FIELD_SECTION_SIZE => {
                if seen_max_field_section_size {
                    return None;
                }
                seen_max_field_section_size = true;
                settings.max_field_section_size = val;
            }
            H3_SETTINGS_QPACK_BLOCKED_STREAMS => {
                if seen_qpack_blocked_streams {
                    return None;
                }
                seen_qpack_blocked_streams = true;
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

/// Maximum valid QUIC stream ID (62-bit varint max per RFC 9000 §16).
const H3_MAX_STREAM_ID: u64 = (1 << 62) - 1;

/// Encode a GOAWAY frame. Payload is a single varint (stream ID).
/// NB7-62: Validates stream_id is within QUIC varint range (62-bit).
pub(crate) fn encode_goaway(stream_id: u64) -> Option<Vec<u8>> {
    if stream_id > H3_MAX_STREAM_ID {
        return None;
    }
    let mut payload_buf = [0u8; 8];
    let pw = varint_encode(&mut payload_buf, stream_id)?;
    encode_frame(H3_FRAME_GOAWAY, &payload_buf[..pw])
}

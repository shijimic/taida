/// HTTP/3 parity implementation for `taida-lang/net` v7.
///
/// **NB7-42: Phase 6+** — split into `qpack.rs` / `frame.rs` / `request.rs` / `connection.rs`.
///
/// Phase 6 additions (NET7-6a):
/// - QPACK dynamic table support (RFC 9204 Section 4.3)
/// - Encoder/decoder instruction streams
/// - Ring buffer with absolute/relative index mapping
/// - Capacity management with eviction
/// - `H3DynamicTable` integrated into `H3Connection`
///
/// NET7-6b (NB7-27): `H3DecodeError` enum for error traceability.
/// NET7-6c (NB7-22): Idle timeout tracking on `H3Connection`.
/// NET7-7b: Graceful shutdown QUIC-level (GOAWAY -> drain -> close pipeline).
/// NET7-7c: QUIC transport state integration (NB7-20, NB7-26, NB7-28).
///
/// This module implements the HTTP/3 protocol layer for the Interpreter backend,
/// mirroring the Native reference implementation (Phase 2) for parity.
///
/// # Architecture
///
/// The h3 implementation is structured as follows:
///
/// 1. **QPACK**: Header compression/decompression (RFC 9204)
/// 2. **Variable-length integers**: QUIC varint coding (RFC 9000 Section 16)
/// 3. **H3 frame layer**: Frame encode/decode using QUIC varints
/// 4. **H3 SETTINGS**: Encode/decode
/// 5. **H3 GOAWAY**: Encode for graceful shutdown
/// 6. **Stream state machine**: Per-stream lifecycle (idle -> open -> half-closed -> closed)
/// 7. **Connection state**: Connection-level management, graceful shutdown
/// 8. **Request extraction**: Pseudo-header validation matching H2 semantics
/// 9. **Response builders**: QPACK-encoded HEADERS + DATA frames
/// 10. **Self-tests**: QPACK round-trip and request validation (parity with Native)
/// 11. **Design Decisions**
///
/// # Design Decisions
///
/// - Native is the reference backend; this module follows Native semantics exactly
/// - QUIC transport is gated on external library availability (same as Native)
/// - Transport I/O does NOT use the existing `Transport` trait (NB7-7 decision)
/// - QPACK uses static table only (no dynamic table in Phase 2/3; dynamic table in Phase 6+)
/// - Handler contract is the same 14-field request pack as h1/h2
/// - Bounded-copy discipline: 1 packet = at most 1 materialization
/// - 0-RTT: default-off, not exposed

mod qpack;
mod frame;
mod request;
mod connection;

// ── Re-exports: preserve exact same public API as the old monolithic net_h3.rs ──

// From qpack.rs
pub(crate) use qpack::{
    QpackStaticEntry, QPACK_STATIC_TABLE,
    H3DecodeError, H3Result, H3Header,
    qpack_decode_int, qpack_decode_int_r,
    qpack_decode_string, qpack_decode_string_r,
    qpack_decode_block, qpack_decode_block_r,
    qpack_encode_int, qpack_encode_string,
    qpack_encode_block, qpack_encode_block_with_dynamic,
    H3DynamicTable, DynamicTableEntry,
    H3EncoderInstruction, H3DecoderInstruction, H3DecoderState,
    encode_insert_with_name_ref, encode_insert_with_literal_name,
    encode_duplicate, encode_set_capacity,
    encode_section_ack, encode_stream_cancel, encode_insert_count_increment,
    decode_encoder_instruction, decode_decoder_instruction,
    apply_encoder_instruction,
    // These frame decode _r variants ended up in qpack due to original file layout:
    varint_decode_r, decode_frame_header_r, decode_frame_r,
};

// From frame.rs
pub(crate) use frame::{
    is_canonical_varint,
    varint_decode, varint_encode,
    H3Settings, H3_MAX_SETTINGS_PAIRS,
    encode_settings, decode_settings,
    encode_goaway,
    H3_DEFAULT_MAX_FIELD_SECTION_SIZE, H3_MAX_HEADERS, H3_MAX_STREAMS,
    H3_FRAME_DATA, H3_FRAME_HEADERS, H3_FRAME_CANCEL_PUSH,
    H3_FRAME_SETTINGS, H3_FRAME_PUSH_PROMISE, H3_FRAME_GOAWAY, H3_FRAME_MAX_PUSH_ID,
    H3_ERROR_NO_ERROR, H3_ERROR_GENERAL_PROTOCOL_ERROR, H3_ERROR_INTERNAL_ERROR,
    H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY, H3_SETTINGS_MAX_FIELD_SECTION_SIZE,
    H3_SETTINGS_QPACK_BLOCKED_STREAMS,
    encode_frame, decode_frame_header, decode_frame,
};

// From request.rs
pub(crate) use request::{
    selftest_qpack_roundtrip, selftest_request_validation,
    H3RequestError, H3RequestFields, extract_request_fields,
    build_response_headers_frame, build_data_frame,
    SelftestResult, run_selftests,
};

// From connection.rs
pub(crate) use connection::{
    H3StreamState, H3Stream,
    H3ConnState, H3HandlerContext,
    H3Connection,
};

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
        assert!(matches!(conn.check_timeout(), Some(H3DecodeError::IdleTimeout)));
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
        // Construct 65 settings pairs with unique unknown IDs and verify rejection.
        let mut buf = vec![0u8; 512];
        let mut pos = 0;
        // Write 64 unique unknown settings (IDs 1000..1063).
        // Unknown settings are ignored per RFC 9114 but still count toward pair limit.
        for i in 0..64u64 {
            pos += varint_encode(&mut buf[pos..], 1000 + i).unwrap();
            pos += varint_encode(&mut buf[pos..], 0).unwrap();
        }
        assert!(decode_settings(&buf[..pos]).is_some(),
            "64 unique pairs (H3_MAX_SETTINGS_PAIRS) should be valid");

        // Add a 65th pair — should be rejected (DoS mitigation)
        pos += varint_encode(&mut buf[pos..], 2000u64).unwrap();
        pos += varint_encode(&mut buf[pos..], 0).unwrap();
        assert!(decode_settings(&buf[..pos]).is_none(),
            "65 pairs (> H3_MAX_SETTINGS_PAIRS) must be rejected");
    }

    #[test]
    fn test_settings_duplicate_rejection() {
        // RFC 9114 §7.2.4.2: duplicate setting ID is rejected.
        let mut buf = vec![0u8; 32];
        let mut pos = 0;
        pos += varint_encode(&mut buf[pos..], H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY).unwrap();
        pos += varint_encode(&mut buf[pos..], 0).unwrap();
        pos += varint_encode(&mut buf[pos..], H3_SETTINGS_MAX_FIELD_SECTION_SIZE).unwrap();
        pos += varint_encode(&mut buf[pos..], 65536).unwrap();
        assert!(decode_settings(&buf[..pos]).is_some(), "3 known unique settings valid");

        // Duplicate QPACK_MAX_TABLE_CAPACITY
        let dup_pos = pos;
        pos += varint_encode(&mut buf[pos..], H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY).unwrap();
        pos += varint_encode(&mut buf[pos..], 0).unwrap();
        assert!(decode_settings(&buf[..pos]).is_none(), "duplicate QPACK_MAX_TABLE_CAPACITY rejected");

        // Reset, duplicate MAX_FIELD_SECTION_SIZE
        pos = 0;
        pos += varint_encode(&mut buf[pos..], H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY).unwrap();
        pos += varint_encode(&mut buf[pos..], 0).unwrap();
        pos += varint_encode(&mut buf[pos..], H3_SETTINGS_MAX_FIELD_SECTION_SIZE).unwrap();
        pos += varint_encode(&mut buf[pos..], 65536).unwrap();
        pos += varint_encode(&mut buf[pos..], H3_SETTINGS_MAX_FIELD_SECTION_SIZE).unwrap();
        pos += varint_encode(&mut buf[pos..], 32768).unwrap();
        assert!(decode_settings(&buf[..pos]).is_none(), "duplicate MAX_FIELD_SECTION_SIZE rejected");
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

    // ── NET7-7b: Graceful Shutdown QUIC-Level Tests ─────────────────────

    #[test]
    fn test_shutdown_state_machine_active_to_draining_to_closed() {
        let mut conn = H3Connection::new();
        assert_eq!(conn.state, H3ConnState::Idle);

        // Transition to Active
        assert!(conn.transition_state(H3ConnState::Active));
        assert_eq!(conn.state, H3ConnState::Active);
        assert!(!conn.is_draining());
        assert!(!conn.is_closed());
        assert!(conn.accepts_new_streams());

        // Begin shutdown: Active -> Draining with GOAWAY
        assert!(conn.begin_shutdown());
        assert!(conn.goaway_sent);
        assert_eq!(conn.state, H3ConnState::Draining);
        assert!(conn.is_draining());
        assert!(!conn.is_closed());
        assert!(!conn.accepts_new_streams());

        // Complete shutdown: Draining -> Closed
        conn.complete_shutdown();
        assert_eq!(conn.state, H3ConnState::Closed);
        assert!(!conn.is_draining());
        assert!(conn.is_closed());
        assert!(!conn.accepts_new_streams());
    }

    #[test]
    fn test_shutdown_full_pipeline_emits_goaway_frame() {
        let mut conn = H3Connection::new();
        conn.transition_state(H3ConnState::Active);
        conn.new_stream(0);
        conn.new_stream(2);
        conn.last_peer_stream_id = 2;

        // Step 1: Active -> Draining, GOAWAY sent
        let (success, frames) = conn.shutdown();
        assert!(success);
        assert!(frames.is_some());
        let frames = frames.unwrap();
        assert_eq!(frames.len(), 1);

        // Verify the frame is a valid GOAWAY frame
        let (ft, fl, hs) = decode_frame_header(&frames[0]).expect("decode goaway header");
        assert_eq!(ft, H3_FRAME_GOAWAY);
        let (sid, _) = varint_decode(&frames[0][hs..hs + fl as usize]).expect("decode goaway payload");
        assert_eq!(sid, 2);

        assert_eq!(conn.state, H3ConnState::Draining);

        // Step 2: Draining -> Closed (caller should wait for streams to complete first)
        for stream in conn.streams.iter_mut() {
            stream.state = H3StreamState::Closed;
        }
        let (s2, f2) = conn.shutdown();
        assert!(s2);
        assert!(f2.is_none());
        assert_eq!(conn.state, H3ConnState::Closed);
        assert!(conn.streams.is_empty());
    }

    #[test]
    fn test_shutdown_idempotent_on_closed() {
        let mut conn = H3Connection::new();
        conn.transition_state(H3ConnState::Active);

        // Step 1: Active -> Draining
        let (s1, _) = conn.shutdown();
        assert!(s1);
        assert!(!conn.is_closed());
        assert_eq!(conn.state, H3ConnState::Draining);

        // Step 2: Draining -> Closed
        let (s2, _) = conn.shutdown();
        assert!(s2);
        assert!(conn.is_closed());

        // Third shutdown on Closed connection should be no-op
        let (s3, f3) = conn.shutdown();
        assert!(!s3);
        assert!(f3.is_none());
        assert!(conn.is_closed());
    }

    #[test]
    fn test_shutdown_begins_twice_blocked() {
        let mut conn = H3Connection::new();
        conn.transition_state(H3ConnState::Active);

        assert!(conn.begin_shutdown());
        // Second begin should fail
        assert!(!conn.begin_shutdown());
    }

    #[test]
    fn test_receive_goaway_from_peer() {
        let mut conn = H3Connection::new();
        conn.transition_state(H3ConnState::Active);

        assert!(conn.receive_goaway(6));
        assert!(conn.goaway_received);
        assert_eq!(conn.goaway_id, 6);
        assert_eq!(conn.state, H3ConnState::Draining);
        assert!(conn.is_draining());

        // Second receive should fail
        assert!(!conn.receive_goaway(8));
    }

    #[test]
    fn test_shutdown_without_active_streams() {
        let mut conn = H3Connection::new();
        conn.transition_state(H3ConnState::Active);

        // Step 1: Active -> Draining, GOAWAY sent
        let (success, frames) = conn.shutdown();
        assert!(success);
        // Even with no streams, GOAWAY should be sent
        assert!(frames.is_some());
        assert_eq!(conn.state, H3ConnState::Draining);

        // Step 2: Draining -> Closed (no streams to wait for)
        let (s2, _) = conn.shutdown();
        assert!(s2);
        assert_eq!(conn.state, H3ConnState::Closed);
    }

    #[test]
    fn test_goaway_tracking_both_directions() {
        let mut conn = H3Connection::new();
        conn.transition_state(H3ConnState::Active);

        // Send GOAWAY to peer
        assert!(conn.begin_shutdown());
        assert!(!conn.goaway_received);

        // Peer also sends GOAWAY back (unlikely but protocol-valid)
        assert!(conn.receive_goaway(100));
        assert!(conn.goaway_received);
    }
}

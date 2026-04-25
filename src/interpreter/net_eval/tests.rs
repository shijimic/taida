//! Test module for net_eval (C12B-025 mechanical split).
//!
//! Extracted verbatim from net_eval.rs lines 5714..12591.

use super::helpers::*;
use super::types::*;
use super::*;
use crate::interpreter::value::AsyncStatus;

#[test]
fn test_net_symbols_count() {
    // HTTP v1 (3) + HTTP v2 (1) + HTTP v3 (4) + HTTP v4 (6) + v5 (1) = 15
    assert_eq!(NET_SYMBOLS.len(), 15);
    assert!(!NET_SYMBOLS.contains(&"dnsResolve"));
    assert!(NET_SYMBOLS.contains(&"httpServe"));
    assert!(NET_SYMBOLS.contains(&"httpParseRequestHead"));
    assert!(NET_SYMBOLS.contains(&"wsCloseCode"));
    assert!(NET_SYMBOLS.contains(&"httpEncodeResponse"));
    assert!(NET_SYMBOLS.contains(&"readBody"));
    // v3 streaming
    assert!(NET_SYMBOLS.contains(&"startResponse"));
    assert!(NET_SYMBOLS.contains(&"writeChunk"));
    assert!(NET_SYMBOLS.contains(&"endResponse"));
    assert!(NET_SYMBOLS.contains(&"sseEvent"));
    // v4 request body streaming
    assert!(NET_SYMBOLS.contains(&"readBodyChunk"));
    assert!(NET_SYMBOLS.contains(&"readBodyAll"));
    // v4 WebSocket
    assert!(NET_SYMBOLS.contains(&"wsUpgrade"));
    assert!(NET_SYMBOLS.contains(&"wsSend"));
    assert!(NET_SYMBOLS.contains(&"wsReceive"));
    assert!(NET_SYMBOLS.contains(&"wsClose"));
}

// ── WebSocket tests ──

#[test]
fn test_ws_accept_computation() {
    // RFC 6455 Section 4.2.2 example:
    // Key: "dGhlIHNhbXBsZSBub25jZQ==" → Accept: "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    let accept = Interpreter::compute_ws_accept("dGhlIHNhbXBsZSBub25jZQ==");
    assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
}

#[test]
fn test_ws_frame_write() {
    use std::io::Read;
    // Create a pair of connected streams to test frame write.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut client = connect_with_retry(port);
    let (server_tcp, _) = listener.accept().unwrap();
    let mut server = ConnStream::Plain(server_tcp);

    // Write a text frame "Hello" from server to client.
    Interpreter::write_ws_frame(&mut server, 0x1, b"Hello").unwrap();

    // Read and verify the frame.
    let mut buf = [0u8; 64];
    let n = client.read(&mut buf).unwrap();
    assert!(n >= 7);
    // byte 0: FIN=1, opcode=0x1 → 0x81
    assert_eq!(buf[0], 0x81);
    // byte 1: MASK=0, len=5 → 0x05
    assert_eq!(buf[1], 0x05);
    // payload
    assert_eq!(&buf[2..7], b"Hello");
}

// ── Sentinel guard tests ──

#[test]
fn test_sentinel_guard_blocks_without_import() {
    let mut interp = Interpreter::new();
    let args: Vec<Expr> = vec![];
    let result = interp.try_net_func("httpServe", &args).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_sentinel_guard_passes_with_correct_sentinel() {
    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
    let args: Vec<Expr> = vec![];
    let result = interp.try_net_func("httpServe", &args);
    assert!(result.is_err());
}

#[test]
fn test_sentinel_guard_with_alias() {
    // >>> taida-lang/net => @(httpServe: serve)
    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("serve", Value::str("__net_builtin_httpServe".into()));
    let args: Vec<Expr> = vec![];
    let result = interp.try_net_func("serve", &args);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("httpServe"));
}

#[test]
fn test_sentinel_guard_blocks_wrong_sentinel() {
    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("httpServe", Value::str("__os_builtin_httpServe".into()));
    let args: Vec<Expr> = vec![];
    assert!(interp.try_net_func("httpServe", &args).unwrap().is_none());
}

#[test]
fn test_sentinel_guard_blocks_user_function() {
    let mut interp = Interpreter::new();
    interp.env.define_force("httpServe", Value::Int(42));
    let args: Vec<Expr> = vec![];
    assert!(interp.try_net_func("httpServe", &args).unwrap().is_none());
}

// ── httpParseRequestHead tests ──

#[test]
fn test_parse_complete_get() {
    let raw = b"GET /path?x=1 HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    let fields = match &result {
        Value::BuchiPack(f) => f,
        _ => panic!("expected BuchiPack"),
    };
    // Result success: __type = "Result", throw = Unit
    assert!(matches!(
        fields.iter().find(|(k, _)| k == "__type"),
        Some((_, Value::Str(s))) if s.as_str() == "Result"
    ));
    assert!(matches!(
        fields.iter().find(|(k, _)| k == "throw"),
        Some((_, Value::Unit))
    ));
    // Inner value
    let inner = match fields.iter().find(|(k, _)| k == "__value") {
        Some((_, v)) => v,
        _ => panic!("no __value"),
    };
    let inner_fields = match inner {
        Value::BuchiPack(f) => f,
        _ => panic!("expected BuchiPack"),
    };
    // complete = true
    assert!(matches!(
        inner_fields.iter().find(|(k, _)| k == "complete"),
        Some((_, Value::Bool(true)))
    ));
    // method span: "GET" starts at 0, len 3
    let method = match inner_fields.iter().find(|(k, _)| k == "method") {
        Some((_, Value::BuchiPack(f))) => f,
        _ => panic!("no method"),
    };
    assert!(matches!(
        method.iter().find(|(k, _)| k == "start"),
        Some((_, Value::Int(0)))
    ));
    assert!(matches!(
        method.iter().find(|(k, _)| k == "len"),
        Some((_, Value::Int(3)))
    ));
    // path span: "/path" starts at 4, len 5
    let path = match inner_fields.iter().find(|(k, _)| k == "path") {
        Some((_, Value::BuchiPack(f))) => f,
        _ => panic!("no path"),
    };
    assert!(matches!(
        path.iter().find(|(k, _)| k == "start"),
        Some((_, Value::Int(4)))
    ));
    assert!(matches!(
        path.iter().find(|(k, _)| k == "len"),
        Some((_, Value::Int(5)))
    ));
    // query span: "x=1" starts at 10, len 3
    let query = match inner_fields.iter().find(|(k, _)| k == "query") {
        Some((_, Value::BuchiPack(f))) => f,
        _ => panic!("no query"),
    };
    assert!(matches!(
        query.iter().find(|(k, _)| k == "start"),
        Some((_, Value::Int(10)))
    ));
    assert!(matches!(
        query.iter().find(|(k, _)| k == "len"),
        Some((_, Value::Int(3)))
    ));
}

#[test]
fn test_parse_post_with_body() {
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: 5\r\nHost: localhost\r\n\r\nhello";
    let result = parse_request_head(raw);
    let inner = extract_result_inner(&result);
    assert!(get_bool(inner, "complete"));
    assert_eq!(get_int(inner, "contentLength"), 5);
    // bodyOffset should equal consumed (end of headers)
    let consumed = get_int(inner, "consumed");
    assert!(consumed > 0);
    assert_eq!(get_int(inner, "bodyOffset"), consumed);
}

#[test]
fn test_parse_incomplete() {
    let raw = b"GET / HTTP/1.1\r\nHost: local";
    let result = parse_request_head(raw);
    let inner = extract_result_inner(&result);
    assert!(!get_bool(inner, "complete"));
}

#[test]
fn test_parse_malformed() {
    let raw = b"INVALID\x00\x01\x02";
    let result = parse_request_head(raw);
    let fields = match &result {
        Value::BuchiPack(f) => f,
        _ => panic!("expected BuchiPack"),
    };
    // Result failure: throw is not Unit
    assert!(!matches!(
        fields.iter().find(|(k, _)| k == "throw"),
        Some((_, Value::Unit))
    ));
}

#[test]
fn test_parse_no_query() {
    let raw = b"GET /path HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    let inner = extract_result_inner(&result);
    // query span should have len=0
    let query = match inner.iter().find(|(k, _)| k == "query").map(|(_, v)| v) {
        Some(Value::BuchiPack(f)) => f,
        _ => panic!("no query"),
    };
    assert!(matches!(
        query.iter().find(|(k, _)| k == "len"),
        Some((_, Value::Int(0)))
    ));
}

// ── httpEncodeResponse tests ──

#[test]
fn test_encode_200_text() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::str("content-type".into())),
                ("value".into(), Value::str("text/plain".into())),
            ])]),
        ),
        ("body".into(), Value::str("Hello".into())),
    ]);
    let result = encode_response(&response);
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(text.contains("content-type: text/plain\r\n"));
    assert!(text.contains("Content-Length: 5\r\n"));
    assert!(text.ends_with("\r\n\r\nHello"));
}

#[test]
fn test_encode_404_empty() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(404)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(text.contains("Content-Length: 0\r\n"));
}

#[test]
fn test_encode_binary_body() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::bytes(vec![0x00, 0xFF, 0x42])),
    ]);
    let result = encode_response(&response);
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    assert!(bytes.ends_with(&[0x00, 0xFF, 0x42]));
}

#[test]
fn test_encode_user_content_length_preserved() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::str("Content-Length".into())),
                ("value".into(), Value::str("99".into())),
            ])]),
        ),
        ("body".into(), Value::str("Hi".into())),
    ]);
    let result = encode_response(&response);
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    // User's Content-Length should be preserved, no auto-append
    assert!(text.contains("Content-Length: 99\r\n"));
    assert_eq!(text.matches("Content-Length").count(), 1);
}

// ── Test helpers ──

fn extract_result_inner(result: &Value) -> &Vec<(String, Value)> {
    let fields = match result {
        Value::BuchiPack(f) => f,
        _ => panic!("expected Result BuchiPack"),
    };
    match fields.iter().find(|(k, _)| k == "__value") {
        Some((_, Value::BuchiPack(f))) => f,
        _ => panic!("no __value BuchiPack"),
    }
}

fn get_bool(fields: &[(String, Value)], key: &str) -> bool {
    match fields.iter().find(|(k, _)| k == key) {
        Some((_, Value::Bool(b))) => *b,
        _ => panic!("missing bool field: {}", key),
    }
}

fn get_int(fields: &[(String, Value)], key: &str) -> i64 {
    match fields.iter().find(|(k, _)| k == key) {
        Some((_, Value::Int(n))) => *n,
        _ => panic!("missing int field: {}", key),
    }
}

fn is_result_failure(result: &Value) -> bool {
    match result {
        Value::BuchiPack(f) => {
            !matches!(f.iter().find(|(k, _)| k == "throw"), Some((_, Value::Unit)))
        }
        _ => false,
    }
}

fn get_failure_message(result: &Value) -> String {
    let fields = match result {
        Value::BuchiPack(f) => f,
        _ => panic!("expected BuchiPack"),
    };
    match fields.iter().find(|(k, _)| k == "throw") {
        Some((_, Value::Error(e))) => e.message.clone(),
        _ => panic!("no Error in throw"),
    }
}

// ── Content-Length validation tests ──

#[test]
fn test_parse_invalid_content_length_non_numeric() {
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: abc\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

#[test]
fn test_parse_invalid_content_length_negative() {
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: -5\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

#[test]
fn test_parse_invalid_content_length_leading_plus() {
    // "+5" is accepted by Rust's parse::<i64>() but must be rejected for JS parity.
    // Both backends must use strict digits-only validation (/^\d+$/ equivalent).
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: +5\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

#[test]
fn test_parse_invalid_content_length_trailing_chars() {
    // "5abc" must be rejected (not silently parsed as 5).
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: 5abc\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

#[test]
fn test_parse_invalid_content_length_empty() {
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: \r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

#[test]
fn test_parse_duplicate_content_length() {
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 10\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("duplicate Content-Length"));
}

#[test]
fn test_parse_valid_content_length_zero() {
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: 0\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    assert_eq!(get_int(inner, "contentLength"), 0);
}

#[test]
fn test_parse_content_length_i64_overflow() {
    // Value exceeds i64::MAX (9223372036854775807). Interpreter rejects via parse::<i64>().
    // JS must also reject (string-length guard) for cross-backend parity.
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: 999999999999999999999999\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

#[test]
fn test_parse_content_length_max_safe_integer_boundary() {
    // Exactly Number.MAX_SAFE_INTEGER = 9007199254740991 (2^53 - 1) — should succeed.
    // This is the cross-backend upper limit (JS Number precision boundary).
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: 9007199254740991\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    assert_eq!(get_int(inner, "contentLength"), 9_007_199_254_740_991);
}

#[test]
fn test_parse_content_length_max_safe_integer_plus_one() {
    // Number.MAX_SAFE_INTEGER + 1 = 9007199254740992 — must be rejected.
    // Beyond this value, JS Number loses precision, breaking cross-backend parity.
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: 9007199254740992\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

#[test]
fn test_parse_content_length_i64_max_rejected() {
    // i64::MAX = 9223372036854775807 — exceeds MAX_SAFE_INTEGER, must be rejected.
    let raw =
        b"POST /data HTTP/1.1\r\nContent-Length: 9223372036854775807\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

#[test]
fn test_parse_content_length_i64_max_plus_one() {
    // i64::MAX + 1 = 9223372036854775808 — must be rejected.
    let raw =
        b"POST /data HTTP/1.1\r\nContent-Length: 9223372036854775808\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

// ── Content-Length leading-zero tests (NB-20 parity fix) ──

#[test]
fn test_parse_content_length_leading_zeros_simple() {
    // "007" should be accepted as 7 (RFC 9110: Content-Length = 1*DIGIT, leading zeros valid).
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: 007\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    assert_eq!(get_int(inner, "contentLength"), 7);
}

#[test]
fn test_parse_content_length_leading_zeros_17_digits() {
    // "00000000000000005" (17 chars) should be accepted as 5.
    // JS must strip leading zeros before length check for parity.
    let raw =
        b"POST /data HTTP/1.1\r\nContent-Length: 00000000000000005\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    assert_eq!(get_int(inner, "contentLength"), 5);
}

#[test]
fn test_parse_content_length_all_zeros_long() {
    // "00000000000000000" (17 zeros) should be accepted as 0.
    let raw =
        b"POST /data HTTP/1.1\r\nContent-Length: 00000000000000000\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    assert_eq!(get_int(inner, "contentLength"), 0);
}

#[test]
fn test_parse_content_length_leading_zeros_0042() {
    // "0042" should be accepted as 42.
    let raw = b"POST /data HTTP/1.1\r\nContent-Length: 0042\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    assert_eq!(get_int(inner, "contentLength"), 42);
}

#[test]
fn test_parse_content_length_leading_zeros_over_max_safe() {
    // Leading zeros + value > MAX_SAFE_INTEGER must still be rejected.
    // "009007199254740992" = 9007199254740992 > MAX_SAFE_INTEGER
    let raw =
        b"POST /data HTTP/1.1\r\nContent-Length: 009007199254740992\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("invalid Content-Length"));
}

// ── Encode strict validation tests ──

#[test]
fn test_encode_missing_status() {
    let response = Value::pack(vec![
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str("Hello".into())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("missing required field 'status'"));
}

#[test]
fn test_encode_wrong_type_status() {
    let response = Value::pack(vec![
        ("status".into(), Value::str("200".into())),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str("Hello".into())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("status must be Int"));
}

#[test]
fn test_encode_status_out_of_range() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(99)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str("Hello".into())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("status must be 100-999"));
}

#[test]
fn test_encode_missing_headers() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        ("body".into(), Value::str("Hello".into())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("missing required field 'headers'"));
}

#[test]
fn test_encode_missing_body() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        ("headers".into(), Value::list(vec![])),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("missing required field 'body'"));
}

#[test]
fn test_encode_crlf_in_header_name() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::str("Bad\r\nHeader".into())),
                ("value".into(), Value::str("ok".into())),
            ])]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("CR/LF"));
}

#[test]
fn test_encode_crlf_in_header_value() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::str("X-Test".into())),
                ("value".into(), Value::str("inject\r\nEvil: header".into())),
            ])]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("CR/LF"));
}

#[test]
fn test_encode_wrong_type_body() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::Int(42)),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("body must be Bytes or Str"));
}

#[test]
fn test_encode_header_name_not_str() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::Int(42)),
                ("value".into(), Value::str("ok".into())),
            ])]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("headers[0].name must be Str"));
}

// ── NB-7: header name/value length limits ──

#[test]
fn test_encode_header_name_exceeds_limit() {
    let long_name = "X".repeat(8193);
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::str(long_name)),
                ("value".into(), Value::str("ok".into())),
            ])]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(
        get_failure_message(&result).contains("name exceeds 8192 bytes"),
        "Expected name length error, got: {}",
        get_failure_message(&result)
    );
}

#[test]
fn test_encode_header_value_exceeds_limit() {
    let long_value = "V".repeat(65537);
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::str("X-Data".into())),
                ("value".into(), Value::str(long_value)),
            ])]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(
        get_failure_message(&result).contains("value exceeds 65536 bytes"),
        "Expected value length error, got: {}",
        get_failure_message(&result)
    );
}

#[test]
fn test_encode_header_name_at_limit_ok() {
    let name = "X".repeat(8192);
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::str(name)),
                ("value".into(), Value::str("ok".into())),
            ])]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
}

#[test]
fn test_encode_header_value_at_limit_ok() {
    let value = "V".repeat(65536);
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::str("X-Data".into())),
                ("value".into(), Value::str(value)),
            ])]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
}

// ── No-body status tests ──

#[test]
fn test_encode_204_empty_body_ok() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(204)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("HTTP/1.1 204 No Content\r\n"));
    // No Content-Length for 204
    assert!(!text.contains("Content-Length"));
    // No body after final CRLF
    assert!(text.ends_with("\r\n\r\n"));
}

#[test]
fn test_encode_204_with_body_rejected() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(204)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str("oops".into())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("must not have a body"));
}

#[test]
fn test_encode_304_with_body_rejected() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(304)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str("cached".into())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("must not have a body"));
}

#[test]
fn test_encode_205_with_body_rejected() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(205)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str("data".into())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("must not have a body"));
}

#[test]
fn test_encode_205_empty_body_ok() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(205)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("HTTP/1.1 205 Reset Content\r\n"));
    assert!(!text.contains("Content-Length"));
}

#[test]
fn test_encode_1xx_with_body_rejected() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(100)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str("data".into())),
    ]);
    let result = encode_response(&response);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("must not have a body"));
}

#[test]
fn test_encode_204_content_length_stripped() {
    // User-provided Content-Length should be silently dropped for 204
    let response = Value::pack(vec![
        ("status".into(), Value::Int(204)),
        (
            "headers".into(),
            Value::list(vec![Value::pack(vec![
                ("name".into(), Value::str("Content-Length".into())),
                ("value".into(), Value::str("0".into())),
            ])]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("Content-Length"));
}

// ── Reason phrase tests ──

#[test]
fn test_encode_429_reason_phrase() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(429)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
}

#[test]
fn test_encode_unknown_status_no_fake_reason() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(599)),
        ("headers".into(), Value::list(vec![])),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    // Should NOT say "OK" for unknown status
    assert!(text.starts_with("HTTP/1.1 599 \r\n"));
}

// ── Helper function tests ──

#[test]
fn test_make_fulfilled_async() {
    let inner = Value::Int(42);
    let async_val = make_fulfilled_async(inner);
    match async_val {
        Value::Async(a) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            assert!(matches!(*a.value, Value::Int(42)));
        }
        _ => panic!("expected Async"),
    }
}

#[test]
fn test_extract_result_value_success() {
    let result = make_result_success(Value::pack(vec![("ok".into(), Value::Bool(true))]));
    let inner = extract_result_value(&result);
    assert!(inner.is_some());
}

#[test]
fn test_extract_result_value_failure() {
    let result = make_result_failure_msg("TestError", "test failed");
    let inner = extract_result_value(&result);
    assert!(inner.is_none());
}

#[test]
fn test_get_field_helpers() {
    let fields = vec![
        ("complete".into(), Value::Bool(true)),
        ("count".into(), Value::Int(42)),
        ("name".into(), Value::str("test".into())),
    ];
    assert_eq!(get_field_bool(&fields, "complete"), Some(true));
    assert_eq!(get_field_int(&fields, "count"), Some(42));
    assert!(get_field_value(&fields, "name").is_some());
    assert!(get_field_value(&fields, "missing").is_none());
}

// ── NB-23: Multiple header span verification ──

/// Helper to extract span (start, len) from a header entry.
fn get_header_span(headers: &[Value], idx: usize, field: &str) -> (i64, i64) {
    let entry = match &headers[idx] {
        Value::BuchiPack(f) => f,
        _ => panic!("header[{}] is not BuchiPack", idx),
    };
    let span = match entry.iter().find(|(k, _)| k == field) {
        Some((_, Value::BuchiPack(f))) => f,
        _ => panic!("header[{}].{} not found", idx, field),
    };
    let start = match span.iter().find(|(k, _)| k == "start") {
        Some((_, Value::Int(n))) => *n,
        _ => panic!("no start"),
    };
    let len = match span.iter().find(|(k, _)| k == "len") {
        Some((_, Value::Int(n))) => *n,
        _ => panic!("no len"),
    };
    (start, len)
}

#[test]
fn test_parse_multiple_headers_span() {
    // "GET / HTTP/1.1\r\nHost: example.com\r\nContent-Type: text/plain\r\nX-Custom: value\r\n\r\n"
    let raw = b"GET / HTTP/1.1\r\nHost: example.com\r\nContent-Type: text/plain\r\nX-Custom: value\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);

    // Extract headers list
    let headers = match inner.iter().find(|(k, _)| k == "headers") {
        Some((_, Value::List(h))) => h,
        _ => panic!("no headers list"),
    };
    assert_eq!(headers.len(), 3, "expected 3 headers");

    // Verify each header's name/value span against raw bytes.
    // Header 0: "Host" / "example.com"
    let (name_start, name_len) = get_header_span(headers, 0, "name");
    assert_eq!(
        &raw[name_start as usize..(name_start + name_len) as usize],
        b"Host"
    );
    let (val_start, val_len) = get_header_span(headers, 0, "value");
    assert_eq!(
        &raw[val_start as usize..(val_start + val_len) as usize],
        b"example.com"
    );

    // Header 1: "Content-Type" / "text/plain"
    let (name_start, name_len) = get_header_span(headers, 1, "name");
    assert_eq!(
        &raw[name_start as usize..(name_start + name_len) as usize],
        b"Content-Type"
    );
    let (val_start, val_len) = get_header_span(headers, 1, "value");
    assert_eq!(
        &raw[val_start as usize..(val_start + val_len) as usize],
        b"text/plain"
    );

    // Header 2: "X-Custom" / "value"
    let (name_start, name_len) = get_header_span(headers, 2, "name");
    assert_eq!(
        &raw[name_start as usize..(name_start + name_len) as usize],
        b"X-Custom"
    );
    let (val_start, val_len) = get_header_span(headers, 2, "value");
    assert_eq!(
        &raw[val_start as usize..(val_start + val_len) as usize],
        b"value"
    );
}

#[test]
fn test_parse_single_header_span() {
    let raw = b"GET / HTTP/1.1\r\nAccept: */*\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);

    let headers = match inner.iter().find(|(k, _)| k == "headers") {
        Some((_, Value::List(h))) => h,
        _ => panic!("no headers list"),
    };
    assert_eq!(headers.len(), 1);

    let (name_start, name_len) = get_header_span(headers, 0, "name");
    assert_eq!(
        &raw[name_start as usize..(name_start + name_len) as usize],
        b"Accept"
    );
    let (val_start, val_len) = get_header_span(headers, 0, "value");
    assert_eq!(
        &raw[val_start as usize..(val_start + val_len) as usize],
        b"*/*"
    );
}

#[test]
fn test_parse_no_headers_empty_list() {
    // Minimal valid HTTP request with no headers (just terminator).
    let raw = b"GET / HTTP/1.1\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);

    let headers = match inner.iter().find(|(k, _)| k == "headers") {
        Some((_, Value::List(h))) => h,
        _ => panic!("no headers list"),
    };
    assert_eq!(headers.len(), 0);
}

// ── NB-24: HTTP version validation ──

#[test]
fn test_parse_http_version_1_0_accepted() {
    let raw = b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);

    // version.minor should be 0
    let version = match inner.iter().find(|(k, _)| k == "version") {
        Some((_, Value::BuchiPack(f))) => f,
        _ => panic!("no version"),
    };
    assert!(matches!(
        version.iter().find(|(k, _)| k == "minor"),
        Some((_, Value::Int(0)))
    ));
}

#[test]
fn test_parse_http_version_1_1_accepted() {
    let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);

    let version = match inner.iter().find(|(k, _)| k == "version") {
        Some((_, Value::BuchiPack(f))) => f,
        _ => panic!("no version"),
    };
    assert!(matches!(
        version.iter().find(|(k, _)| k == "minor"),
        Some((_, Value::Int(1)))
    ));
}

#[test]
fn test_parse_http_version_alpha_rejected() {
    // "HTTP/a.b" — httparse rejects non-digit version components
    let raw = b"GET / HTTP/a.b\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("Malformed"));
}

#[test]
fn test_parse_http_version_multi_digit_rejected() {
    // "HTTP/12.34" — httparse rejects multi-digit version numbers
    let raw = b"GET / HTTP/12.34\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("Malformed"));
}

#[test]
fn test_parse_http_version_2_0_rejected() {
    // "HTTP/2.0" — httparse rejects major version != 1
    let raw = b"GET / HTTP/2.0\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("Malformed"));
}

#[test]
fn test_parse_http_version_1_9_rejected() {
    // "HTTP/1.9" — httparse only accepts HTTP/1.0 and HTTP/1.1
    let raw = b"GET / HTTP/1.9\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("Malformed"));
}

#[test]
fn test_parse_http_version_0_9_rejected() {
    // "HTTP/0.9" — httparse rejects major version != 1
    let raw = b"GET / HTTP/0.9\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(is_result_failure(&result));
    assert!(get_failure_message(&result).contains("Malformed"));
}

// ── NB-25: Multiple header encode verification ──

#[test]
fn test_encode_multiple_headers() {
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![
                Value::pack(vec![
                    ("name".into(), Value::str("Content-Type".into())),
                    ("value".into(), Value::str("application/json".into())),
                ]),
                Value::pack(vec![
                    ("name".into(), Value::str("X-Request-Id".into())),
                    ("value".into(), Value::str("abc-123".into())),
                ]),
                Value::pack(vec![
                    ("name".into(), Value::str("Cache-Control".into())),
                    ("value".into(), Value::str("no-cache".into())),
                ]),
            ]),
        ),
        ("body".into(), Value::str("{\"ok\":true}".into())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(text.contains("Content-Type: application/json\r\n"));
    assert!(text.contains("X-Request-Id: abc-123\r\n"));
    assert!(text.contains("Cache-Control: no-cache\r\n"));
    assert!(text.contains("Content-Length: 11\r\n"));
    assert!(text.ends_with("\r\n\r\n{\"ok\":true}"));
}

#[test]
fn test_encode_multiple_headers_order_preserved() {
    // Headers should appear in the order provided by the user.
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![
                Value::pack(vec![
                    ("name".into(), Value::str("X-First".into())),
                    ("value".into(), Value::str("1".into())),
                ]),
                Value::pack(vec![
                    ("name".into(), Value::str("X-Second".into())),
                    ("value".into(), Value::str("2".into())),
                ]),
                Value::pack(vec![
                    ("name".into(), Value::str("X-Third".into())),
                    ("value".into(), Value::str("3".into())),
                ]),
            ]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();

    // Find positions to verify ordering
    let pos_first = text.find("X-First: 1\r\n").expect("X-First missing");
    let pos_second = text.find("X-Second: 2\r\n").expect("X-Second missing");
    let pos_third = text.find("X-Third: 3\r\n").expect("X-Third missing");
    assert!(
        pos_first < pos_second && pos_second < pos_third,
        "Headers not in order: first={}, second={}, third={}",
        pos_first,
        pos_second,
        pos_third
    );
}

#[test]
fn test_encode_duplicate_header_names_preserved() {
    // Multiple headers with the same name should all appear (e.g. Set-Cookie).
    let response = Value::pack(vec![
        ("status".into(), Value::Int(200)),
        (
            "headers".into(),
            Value::list(vec![
                Value::pack(vec![
                    ("name".into(), Value::str("Set-Cookie".into())),
                    ("value".into(), Value::str("a=1".into())),
                ]),
                Value::pack(vec![
                    ("name".into(), Value::str("Set-Cookie".into())),
                    ("value".into(), Value::str("b=2".into())),
                ]),
            ]),
        ),
        ("body".into(), Value::str(String::new())),
    ]);
    let result = encode_response(&response);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    let bytes = match inner.iter().find(|(k, _)| k == "bytes") {
        Some((_, Value::Bytes(b))) => (**b).clone(),
        _ => panic!("no bytes"),
    };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("Set-Cookie: a=1\r\n"));
    assert!(text.contains("Set-Cookie: b=2\r\n"));
    assert_eq!(text.matches("Set-Cookie").count(), 2);
}

// ── httpServe integration tests ──

use crate::lexer::Span;
use crate::parser::{BuchiField, Param};

fn dummy_span() -> Span {
    Span::new(0, 0, 1, 1)
}

/// C27B-003 root-cause fix (C27 wE, 2026-04-25):
/// Replace blind `sleep(100ms)` + bare `TcpStream::connect(...).unwrap()` with
/// poll-until-bound loop. The pre-existing pattern assumed 100 ms was enough
/// for a freshly spawned interpreter thread to reach `TcpListener::bind()`,
/// but on busy CI 2C runners the actual time-to-bind can spike to 300 ms-1.5 s
/// (compilation cache, mold init, env setup), causing the very first
/// `connect()` to surface ConnectionRefused (errno 111). This is the dominant
/// remaining failure mode of `test_http_serve_max_requests_3` and siblings
/// after C26B-003 closed the kernel-ephemeral port collision window.
///
/// The fix is to wait for the actual bind, not for a wall-clock guess. We
/// poll up to `max_attempts` times with `sleep_ms` between attempts (default
/// 200 × 10 ms = 2 s ceiling, well above worst-observed CI bind latency).
///
/// This is a test-helper-only change: production surface is unchanged
/// (D27/D28 escalation 3 points all NO).
fn connect_with_retry(port: u16) -> std::net::TcpStream {
    const MAX_ATTEMPTS: usize = 200;
    const SLEEP_MS: u64 = 10;
    for attempt in 0..MAX_ATTEMPTS {
        match std::net::TcpStream::connect(format!("127.0.0.1:{}", port)) {
            Ok(s) => return s,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                std::thread::sleep(std::time::Duration::from_millis(SLEEP_MS));
                continue;
            }
            Err(e) => panic!(
                "connect_with_retry: unexpected error on attempt {} for port {}: {:?}",
                attempt, port, e
            ),
        }
    }
    panic!(
        "connect_with_retry: server on port {} never became reachable after {} attempts",
        port, MAX_ATTEMPTS
    );
}

/// Build a simple handler lambda expression that returns 200 OK with a given body.
fn make_handler_expr(body_text: &str) -> Expr {
    Expr::Lambda(
        vec![Param {
            name: "req".into(),
            type_annotation: None,
            default_value: None,
            span: dummy_span(),
        }],
        Box::new(Expr::BuchiPack(
            vec![
                BuchiField {
                    name: "status".into(),
                    value: Expr::IntLit(200, dummy_span()),
                    span: dummy_span(),
                },
                BuchiField {
                    name: "headers".into(),
                    value: Expr::ListLit(
                        vec![Expr::BuchiPack(
                            vec![
                                BuchiField {
                                    name: "name".into(),
                                    value: Expr::StringLit("content-type".into(), dummy_span()),
                                    span: dummy_span(),
                                },
                                BuchiField {
                                    name: "value".into(),
                                    value: Expr::StringLit("text/plain".into(), dummy_span()),
                                    span: dummy_span(),
                                },
                            ],
                            dummy_span(),
                        )],
                        dummy_span(),
                    ),
                    span: dummy_span(),
                },
                BuchiField {
                    name: "body".into(),
                    value: Expr::StringLit(body_text.into(), dummy_span()),
                    span: dummy_span(),
                },
            ],
            dummy_span(),
        )),
        dummy_span(),
    )
}

#[test]
fn test_http_serve_bind_failure_returns_fulfilled_async() {
    // Bind a listener to grab a port, then try httpServe on same port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
    let args = vec![
        Expr::IntLit(port as i64, dummy_span()),
        make_handler_expr("ok"),
        Expr::IntLit(1, dummy_span()),
    ];

    let result = interp.try_net_func("httpServe", &args).unwrap().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            // The result should be a failure (bind error)
            let inner = extract_result_value(&a.value);
            assert!(inner.is_none(), "Expected bind failure, but got success");
        }
        _ => panic!("expected Async value"),
    }
}

#[test]
fn test_http_serve_max_requests_1_self_terminates() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18100);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("Hello from Taida!"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    // Wait for server to start
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send an HTTP request
    let mut client = connect_with_retry(port);
    std::io::Write::write_all(&mut client, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();

    // Read response
    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1 200 OK"),
        "Expected 200 OK, got: {}",
        response_str
    );
    assert!(
        response_str.contains("Hello from Taida!"),
        "Expected body in response"
    );

    // Server should have terminated
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value);
            assert!(inner.is_some(), "Expected success result");
            let inner = inner.unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

#[test]
fn test_http_serve_request_pack_has_all_fields() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18200);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("ok"),
            Expr::IntLit(1, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send POST request with body
    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"POST /data?key=val HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello",
    )
    .unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(response_str.contains("200 OK"), "Expected 200 OK");

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

#[test]
fn test_http_serve_max_requests_3() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18300);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("ok"),
            Expr::IntLit(3, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send 3 requests
    for _ in 0..3 {
        let mut client = connect_with_retry(port);
        std::io::Write::write_all(&mut client, b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = Vec::new();
        let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(2)));
        loop {
            let mut buf = [0u8; 4096];
            match std::io::Read::read(&mut client, &mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        let resp = String::from_utf8_lossy(&response);
        assert!(resp.contains("200 OK"));
    }

    // Server should terminate
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_int(inner, "requests"), Some(3));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

#[test]
fn test_http_serve_malformed_request_returns_400() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18400);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("ok"),
            Expr::IntLit(1, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send malformed request
    let mut client = connect_with_retry(port);
    std::io::Write::write_all(&mut client, b"NOT_HTTP\x00\x01\x02\r\n\r\n").unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let resp = String::from_utf8_lossy(&response);
    assert!(
        resp.contains("400 Bad Request"),
        "Expected 400, got: {}",
        resp
    );

    server_handle.join().unwrap();
}

#[test]
fn test_http_serve_missing_args() {
    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
    let result = interp.try_net_func("httpServe", &[]);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .message
            .contains("missing argument 'port'")
    );
}

#[test]
fn test_http_serve_missing_handler() {
    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
    let args = vec![Expr::IntLit(8080, dummy_span())];
    let result = interp.try_net_func("httpServe", &args);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .message
            .contains("missing argument 'handler'")
    );
}

#[test]
fn test_http_serve_port_validation() {
    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
    let handler = make_handler_expr("ok");
    let args = vec![Expr::IntLit(99999, dummy_span()), handler];
    let result = interp.try_net_func("httpServe", &args);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("port must be 0-65535"));
}

/// TCP fragmentation: head split across two writes must still succeed
#[test]
fn test_http_serve_split_head() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18500);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("split-ok"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);

    // Send head in two fragments with a small delay between them
    std::io::Write::write_all(&mut client, b"GET / HT").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::io::Write::write_all(&mut client, b"TP/1.1\r\nHost: localhost\r\n\r\n").unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("200 OK"),
        "Split head should succeed, got: {}",
        response_str
    );
    assert!(response_str.contains("split-ok"));

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// TCP fragmentation: body arriving after head in a separate write.
/// 200 OK proves the server waited for the full body (incomplete bodies get 400).
#[test]
fn test_http_serve_split_body() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18600);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("body-ok"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);

    // Send complete head with Content-Length, but body arrives in a separate write
    std::io::Write::write_all(
        &mut client,
        b"POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 11\r\n\r\n",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::io::Write::write_all(&mut client, b"hello world").unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    // 200 OK proves the server waited for the full 11-byte body;
    // an incomplete body would have resulted in 400.
    assert!(
        response_str.contains("200 OK"),
        "Split body should succeed (200 proves full body arrived), got: {}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// Incomplete body: Content-Length declares 100 bytes but client sends only 5 then closes.
/// Server must return 400, not pass truncated body to handler.
#[test]
fn test_http_serve_incomplete_body_returns_400() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18700);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("should-not-reach"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);

    // Send head claiming 100-byte body, but only send 5 bytes then close
    std::io::Write::write_all(
        &mut client,
        b"POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\nhello",
    )
    .unwrap();
    // Shut down the write side to signal EOF to the server
    let _ = std::net::TcpStream::shutdown(&client, std::net::Shutdown::Write);

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("400 Bad Request"),
        "Incomplete body must be rejected with 400, got: {}",
        response_str
    );
    // Response must NOT contain handler's body (handler should never be called)
    assert!(
        !response_str.contains("should-not-reach"),
        "Handler must not be called for incomplete body"
    );

    let _ = server_handle.join();
}

/// EOF during head: client connects then immediately closes without sending any data.
/// Server must count it as a request and not hang.
#[test]
fn test_http_serve_eof_during_head_does_not_count_request() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18800);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("idle-ok"),
            Expr::IntLit(1, dummy_span()), // maxRequests=1
            Expr::IntLit(3000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Connect and immediately close (EOF before any HTTP data).
    // This idle connection should NOT consume the request budget.
    let client = connect_with_retry(port);
    drop(client); // close immediately

    std::thread::sleep(std::time::Duration::from_millis(200));

    // Now send a real request — this should succeed and consume the budget.
    let mut real = connect_with_retry(port);
    let _ = real.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    let req = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let _ = std::io::Write::write_all(&mut real, req);
    let mut resp = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match std::io::Read::read(&mut real, &mut buf) {
            Ok(0) => break,
            Ok(n) => resp.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    let resp_str = String::from_utf8_lossy(&resp);
    assert!(
        resp_str.contains("200 OK"),
        "real request after idle close should get 200, got: {}",
        resp_str
    );

    // Server should terminate because maxRequests=1 is now reached.
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            // Only the real request counted, not the idle close.
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// Close after partial head: client sends an incomplete request line then closes.
/// Server must return 400 and count it.
#[test]
fn test_http_serve_close_after_partial_head() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18810);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("should-not-reach"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(3000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);

    // Send partial HTTP request (no \r\n\r\n terminator) then close
    std::io::Write::write_all(&mut client, b"GET /hello HTTP/1.1\r\nHost: loc").unwrap();
    let _ = std::net::TcpStream::shutdown(&client, std::net::Shutdown::Write);

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    // Server should respond with 400 for incomplete head
    let resp = String::from_utf8_lossy(&response);
    assert!(
        resp.contains("400 Bad Request"),
        "Partial head should get 400, got: {}",
        resp
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NB-3: Content-Length under 1 MiB but head + body exceeds 1 MiB → 413
/// The early reject condition must be `head_consumed + content_length > MAX_REQUEST_BUF`,
/// not just `content_length > MAX_REQUEST_BUF`.
#[test]
fn test_nb3_head_plus_body_exceeds_limit_returns_413() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18900);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("should not reach"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Craft a request where Content-Length < MAX_REQUEST_BUF (1 MiB)
    // but head_consumed + Content-Length > MAX_REQUEST_BUF.
    // Header is ~60 bytes, so CL = 1048576 - 10 = 1048566 (under 1 MiB).
    // head (~60) + 1048566 > 1048576 → must trigger 413.
    let cl_value = 1_048_576usize - 10; // just under 1 MiB
    let request = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
        cl_value
    );

    let mut client = connect_with_retry(port);
    let _ = client.set_write_timeout(Some(std::time::Duration::from_secs(3)));
    std::io::Write::write_all(&mut client, request.as_bytes()).unwrap();

    // Read response
    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let resp = String::from_utf8_lossy(&response);
    assert!(
        resp.contains("413 Content Too Large"),
        "NB-3: head + body > 1 MiB should get 413, got: {}",
        resp
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NB-3: Content-Length that exactly fits (head + body == MAX_REQUEST_BUF) → 200 OK, not 413
#[test]
fn test_nb3_head_plus_body_exactly_fits_returns_200() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18910);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    // Pre-calculate the header size to set Content-Length so head + body == 1 MiB exactly.
    // "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: NNNNNN\r\n\r\n"
    // We need to know consumed size; build the header template first.
    let header_template = "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: ";
    let header_suffix = "\r\n\r\n";
    // CL digits: we'll target ~6 digits. head_consumed = template + digits + suffix
    // Try CL = 1048576 - 62 = 1048514 (6 digits). head = 48 + 7 + 4 = 59 => 59 + 1048514 = 1048573 < 1048576, fits.
    // Actually compute: template.len() = 48, digits of CL, suffix.len() = 4
    // Let's just compute iteratively.
    let max = 1_048_576usize;
    let template_len = header_template.len() + header_suffix.len(); // 48 + 4 = 52
    // head_consumed = template_len + cl_digits_len
    // We need head_consumed + cl_value == max
    // cl_value = max - head_consumed = max - template_len - cl_digits_len
    // For 6-digit CL: cl = max - 52 - 6 = 1048518, which is 7 digits → contradiction
    // For 7-digit CL: cl = max - 52 - 7 = 1048517, which is 7 digits ✓
    let cl_digits = 7;
    let cl_value = max - template_len - cl_digits;
    assert_eq!(
        cl_value.to_string().len(),
        cl_digits,
        "CL digit count mismatch"
    );

    let request_head = format!("{}{}{}", header_template, cl_value, header_suffix);
    let head_len = request_head.len();
    assert_eq!(
        head_len + cl_value,
        max,
        "head + body must equal MAX_REQUEST_BUF"
    );

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("ok"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);
    let _ = client.set_write_timeout(Some(std::time::Duration::from_secs(5)));
    // Send head + exactly cl_value bytes of body
    std::io::Write::write_all(&mut client, request_head.as_bytes()).unwrap();
    let body = vec![b'A'; cl_value];
    std::io::Write::write_all(&mut client, &body).unwrap();

    // Read response
    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let resp = String::from_utf8_lossy(&response);
    assert!(
        resp.contains("200 OK"),
        "NB-3: head + body == 1 MiB should get 200, got: {}",
        resp
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NB-28: Verify that timeoutMs causes the server to close an idle connection
/// (connects but sends no data). With the "idle = no budget" rule, this idle
/// connection should be cleanly closed without a 400 and without consuming
/// the request budget.
#[test]
fn test_nb28_timeout_closes_connection() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(18920);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("timeout-ok"),
            Expr::IntLit(1, dummy_span()),   // maxRequests=1
            Expr::IntLit(500, dummy_span()), // timeoutMs=500 (short timeout)
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Connect but send NO data — the server should timeout after ~500ms
    let start = std::time::Instant::now();
    let mut client = connect_with_retry(port);

    // Set a generous read timeout on the client side so we don't hang forever
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(5)));

    // Wait for the server to close the connection (clean close, no 400)
    let mut response = Vec::new();
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }
    let elapsed = start.elapsed();

    // Verify: idle connection with no data gets clean close (no 400)
    assert!(
        response.is_empty(),
        "NB-28: idle connection (no data) should get clean close, got: {}",
        String::from_utf8_lossy(&response)
    );

    // Verify: elapsed time should be at least ~400ms (timeout was 500ms)
    // but not more than 3s (proving it was the timeout, not client-side timeout)
    assert!(
        elapsed.as_millis() >= 400,
        "NB-28: elapsed {}ms is too short — timeout did not fire",
        elapsed.as_millis()
    );
    assert!(
        elapsed.as_millis() < 3000,
        "NB-28: elapsed {}ms is too long — timeout should be ~500ms",
        elapsed.as_millis()
    );

    // Idle close did not consume budget. Send a real request to terminate the server.
    std::thread::sleep(std::time::Duration::from_millis(100));
    let mut real = connect_with_retry(port);
    let _ = real.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    let req = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let _ = std::io::Write::write_all(&mut real, req);
    let mut resp2 = Vec::new();
    let mut buf2 = [0u8; 4096];
    loop {
        match std::io::Read::read(&mut real, &mut buf2) {
            Ok(0) => break,
            Ok(n) => resp2.extend_from_slice(&buf2[..n]),
            Err(_) => break,
        }
    }
    let real_resp = String::from_utf8_lossy(&resp2);
    assert!(
        real_resp.contains("200 OK"),
        "NB-28: real request after idle timeout should get 200, got: {}",
        real_resp
    );

    // Verify: server terminates successfully (maxRequests=1 reached by real request)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            // Only the real request counted, not the idle timeout.
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// ── NB-26: Content-Length absent GET should have contentLength=0 ──

#[test]
fn test_nb26_get_without_content_length_has_cl_zero() {
    // A GET request with no Content-Length header.
    // The parser must default contentLength to 0.
    let raw = b"GET /hello HTTP/1.1\r\nHost: localhost\r\nAccept: */*\r\n\r\n";
    let result = parse_request_head(raw);
    assert!(!is_result_failure(&result));
    let inner = extract_result_inner(&result);
    assert!(get_bool(inner, "complete"));
    // contentLength must be 0 when Content-Length header is absent
    assert_eq!(
        get_int(inner, "contentLength"),
        0,
        "GET without Content-Length header must have contentLength=0"
    );
}

// ── NB-27: empty path parse ──

#[test]
fn test_nb27_empty_path_parse() {
    // "GET  HTTP/1.1\r\n..." — double space means the path token is empty.
    // httparse treats this as malformed because it cannot find a valid URI
    // between the method and version.
    let raw = b"GET  HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    // httparse rejects double-space as a malformed request line → ParseError.
    assert!(
        is_result_failure(&result),
        "NB-27: double-space path should be rejected as malformed, got success"
    );
    let msg = get_failure_message(&result);
    assert!(
        msg.contains("Malformed"),
        "NB-27: expected Malformed error, got: {}",
        msg
    );
}

// ── NB-29: sentinel shadow by unmold ──

#[test]
fn test_nb29_sentinel_shadow_by_unmold() {
    // Simulates: >>> taida-lang/net => @(httpServe)
    //            someResult ]=> httpServe
    // After unmold, httpServe is overwritten with a non-sentinel value.
    // try_net_func must return None (sentinel guard blocks dispatch).
    let mut interp = Interpreter::new();

    // Step 1: Set up sentinel (as if imported via >>> taida-lang/net)
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
    // Verify sentinel is active
    let args: Vec<Expr> = vec![];
    let result = interp.try_net_func("httpServe", &args);
    assert!(
        result.is_err(),
        "Before shadow: sentinel should be active (httpServe requires args)"
    );

    // Step 2: Simulate unmold shadow (]=> httpServe overwrites with a value)
    interp.env.define_force("httpServe", Value::Int(99));

    // Step 3: try_net_func must return None — sentinel is gone
    let result = interp.try_net_func("httpServe", &args).unwrap();
    assert!(
        result.is_none(),
        "After unmold shadow: sentinel is gone, try_net_func must return None"
    );
}

// ── readBody tests ──

#[test]
fn test_read_body_content_length() {
    // Build a fake request pack with raw bytes containing a body
    let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\nhello".to_vec();
    let body_start = 35i64; // "hello" starts at offset 35
    let body_len = 5i64;
    let req = Value::pack(vec![
        ("raw".into(), Value::bytes(raw)),
        (
            "body".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(body_start)),
                ("len".into(), Value::Int(body_len)),
            ]),
        ),
    ]);
    let result = eval_read_body(&req).unwrap();
    assert_eq!(result, Value::bytes(b"hello".to_vec()));
}

#[test]
fn test_read_body_no_body() {
    // body.len == 0 should return empty Bytes
    let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec();
    let req = Value::pack(vec![
        ("raw".into(), Value::bytes(raw)),
        (
            "body".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(35)),
                ("len".into(), Value::Int(0)),
            ]),
        ),
    ]);
    let result = eval_read_body(&req).unwrap();
    assert_eq!(result, Value::bytes(vec![]));
}

#[test]
fn test_read_body_missing_raw() {
    // Request pack without 'raw' field should produce RuntimeError
    let req = Value::pack(vec![(
        "body".into(),
        Value::pack(vec![
            ("start".into(), Value::Int(0)),
            ("len".into(), Value::Int(5)),
        ]),
    )]);
    let result = eval_read_body(&req);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("raw"));
}

#[test]
fn test_read_body_not_buchipack() {
    // Argument that is not a BuchiPack should produce RuntimeError
    let req = Value::Int(42);
    let result = eval_read_body(&req);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("request pack"));
}

// ── determine_keep_alive unit tests (NET2-1a/1b/1c) ──

#[test]
fn test_keep_alive_http11_default() {
    // HTTP/1.1 without Connection header → keep-alive
    let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let headers = vec![Value::pack(vec![
        (
            "name".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(16)), // "Host"
                ("len".into(), Value::Int(4)),
            ]),
        ),
        (
            "value".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(22)), // "localhost"
                ("len".into(), Value::Int(9)),
            ]),
        ),
    ])];
    assert!(determine_keep_alive(raw, &headers, 1));
}

#[test]
fn test_keep_alive_http11_connection_close() {
    // HTTP/1.1 with Connection: close → not keep-alive
    let raw = b"GET / HTTP/1.1\r\nConnection: close\r\nHost: localhost\r\n\r\n";
    let headers = vec![
        Value::pack(vec![
            (
                "name".into(),
                Value::pack(vec![
                    ("start".into(), Value::Int(16)),
                    ("len".into(), Value::Int(10)), // "Connection"
                ]),
            ),
            (
                "value".into(),
                Value::pack(vec![
                    ("start".into(), Value::Int(28)),
                    ("len".into(), Value::Int(5)), // "close"
                ]),
            ),
        ]),
        Value::pack(vec![
            (
                "name".into(),
                Value::pack(vec![
                    ("start".into(), Value::Int(35)),
                    ("len".into(), Value::Int(4)), // "Host"
                ]),
            ),
            (
                "value".into(),
                Value::pack(vec![
                    ("start".into(), Value::Int(41)),
                    ("len".into(), Value::Int(9)), // "localhost"
                ]),
            ),
        ]),
    ];
    assert!(!determine_keep_alive(raw, &headers, 1));
}

#[test]
fn test_keep_alive_http10_default() {
    // HTTP/1.0 without Connection header → not keep-alive
    let raw = b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n";
    let headers = vec![Value::pack(vec![
        (
            "name".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(16)),
                ("len".into(), Value::Int(4)), // "Host"
            ]),
        ),
        (
            "value".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(22)),
                ("len".into(), Value::Int(9)), // "localhost"
            ]),
        ),
    ])];
    assert!(!determine_keep_alive(raw, &headers, 0));
}

#[test]
fn test_keep_alive_http10_explicit() {
    // HTTP/1.0 with Connection: keep-alive → keep-alive
    let raw = b"GET / HTTP/1.0\r\nConnection: keep-alive\r\nHost: localhost\r\n\r\n";
    let headers = vec![
        Value::pack(vec![
            (
                "name".into(),
                Value::pack(vec![
                    ("start".into(), Value::Int(16)),
                    ("len".into(), Value::Int(10)), // "Connection"
                ]),
            ),
            (
                "value".into(),
                Value::pack(vec![
                    ("start".into(), Value::Int(28)),
                    ("len".into(), Value::Int(10)), // "keep-alive"
                ]),
            ),
        ]),
        Value::pack(vec![
            (
                "name".into(),
                Value::pack(vec![
                    ("start".into(), Value::Int(40)),
                    ("len".into(), Value::Int(4)), // "Host"
                ]),
            ),
            (
                "value".into(),
                Value::pack(vec![
                    ("start".into(), Value::Int(46)),
                    ("len".into(), Value::Int(9)), // "localhost"
                ]),
            ),
        ]),
    ];
    assert!(determine_keep_alive(raw, &headers, 0));
}

#[test]
fn test_keep_alive_case_insensitive() {
    // Connection header name and value should be case-insensitive
    let raw = b"GET / HTTP/1.1\r\nCONNECTION: CLOSE\r\n\r\n";
    let headers = vec![Value::pack(vec![
        (
            "name".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(16)),
                ("len".into(), Value::Int(10)), // "CONNECTION"
            ]),
        ),
        (
            "value".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(28)),
                ("len".into(), Value::Int(5)), // "CLOSE"
            ]),
        ),
    ])];
    assert!(!determine_keep_alive(raw, &headers, 1));
}

#[test]
fn test_keep_alive_token_list_close_with_upgrade() {
    // "Connection: close, upgrade" — HTTP/1.1 should NOT keep alive (close token present)
    let raw = b"GET / HTTP/1.1\r\nConnection: close, upgrade\r\n\r\n";
    let headers = vec![Value::pack(vec![
        (
            "name".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(16)),
                ("len".into(), Value::Int(10)), // "Connection"
            ]),
        ),
        (
            "value".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(28)),
                ("len".into(), Value::Int(14)), // "close, upgrade"
            ]),
        ),
    ])];
    assert!(!determine_keep_alive(raw, &headers, 1));
}

#[test]
fn test_keep_alive_token_list_keep_alive_with_extra() {
    // "Connection: keep-alive, foo" — HTTP/1.0 should keep alive (keep-alive token present)
    let raw = b"GET / HTTP/1.0\r\nConnection: keep-alive, foo\r\n\r\n";
    let headers = vec![Value::pack(vec![
        (
            "name".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(16)),
                ("len".into(), Value::Int(10)), // "Connection"
            ]),
        ),
        (
            "value".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(28)),
                ("len".into(), Value::Int(15)), // "keep-alive, foo"
            ]),
        ),
    ])];
    assert!(determine_keep_alive(raw, &headers, 0));
}

#[test]
fn test_keep_alive_close_wins_over_keep_alive_same_header() {
    // "Connection: keep-alive, close" — close wins on both HTTP/1.0 and 1.1
    let raw = b"GET / HTTP/1.0\r\nConnection: keep-alive, close\r\n\r\n";
    let headers = vec![Value::pack(vec![
        (
            "name".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(16)),
                ("len".into(), Value::Int(10)),
            ]),
        ),
        (
            "value".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(28)),
                ("len".into(), Value::Int(17)), // "keep-alive, close"
            ]),
        ),
    ])];
    assert!(!determine_keep_alive(raw, &headers, 0)); // HTTP/1.0
    assert!(!determine_keep_alive(raw, &headers, 1)); // HTTP/1.1
}

#[test]
fn test_keep_alive_close_wins_across_duplicate_headers() {
    // Two Connection headers: one says keep-alive, the other says close
    let raw = b"GET / HTTP/1.0\r\nConnection: keep-alive\r\nConnection: close\r\n\r\n";
    let h1 = Value::pack(vec![
        (
            "name".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(16)),
                ("len".into(), Value::Int(10)),
            ]),
        ),
        (
            "value".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(28)),
                ("len".into(), Value::Int(10)), // "keep-alive"
            ]),
        ),
    ]);
    let h2 = Value::pack(vec![
        (
            "name".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(40)),
                ("len".into(), Value::Int(10)),
            ]),
        ),
        (
            "value".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(52)),
                ("len".into(), Value::Int(5)), // "close"
            ]),
        ),
    ]);
    let headers = vec![h1, h2];
    assert!(!determine_keep_alive(raw, &headers, 0)); // HTTP/1.0
    assert!(!determine_keep_alive(raw, &headers, 1)); // HTTP/1.1
}

// ── Keep-Alive integration tests (NET2-1h) ──

/// Helper: read all HTTP responses from a stream, splitting on double CRLF boundaries.
/// Returns a Vec of raw response strings.
fn read_responses(stream: &mut std::net::TcpStream, expected: usize) -> Vec<String> {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut all_data = Vec::new();
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(stream, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                all_data.extend_from_slice(&buf[..n]);
                // Count complete responses by looking for Content-Length pattern
                let text = String::from_utf8_lossy(&all_data);
                let responses_found = text.matches("HTTP/1.1 ").count();
                if responses_found >= expected {
                    // Check if all bodies are complete
                    let mut complete = true;
                    let mut offset = 0;
                    for _ in 0..expected {
                        if let Some(pos) = text[offset..].find("HTTP/1.1 ") {
                            let resp_start = offset + pos;
                            // Find Content-Length
                            if let Some(cl_pos) = text[resp_start..]
                                .to_ascii_lowercase()
                                .find("content-length: ")
                            {
                                let cl_start = resp_start + cl_pos + 16;
                                if let Some(cl_end) = text[cl_start..].find("\r\n") {
                                    let cl: usize =
                                        text[cl_start..cl_start + cl_end].parse().unwrap_or(0);
                                    if let Some(body_start) = text[resp_start..].find("\r\n\r\n") {
                                        let body_offset = resp_start + body_start + 4;
                                        if body_offset + cl > all_data.len() {
                                            complete = false;
                                            break;
                                        }
                                        offset = body_offset + cl;
                                    } else {
                                        complete = false;
                                        break;
                                    }
                                } else {
                                    complete = false;
                                    break;
                                }
                            } else {
                                complete = false;
                                break;
                            }
                        } else {
                            complete = false;
                            break;
                        }
                    }
                    if complete {
                        break;
                    }
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }
    // Split into individual responses
    let text = String::from_utf8_lossy(&all_data).to_string();
    let mut results = Vec::new();
    let mut remaining = text.as_str();
    while let Some(pos) = remaining.find("HTTP/1.1 ") {
        let start = pos;
        // Find next response or end of data
        let next = remaining[start + 9..]
            .find("HTTP/1.1 ")
            .map(|p| start + 9 + p)
            .unwrap_or(remaining.len());
        results.push(remaining[start..next].to_string());
        remaining = &remaining[next..];
    }
    results
}

/// NET2-1h: 1 connection, 2 requests → 2 responses (keep-alive works)
#[test]
fn test_keep_alive_two_requests_one_connection() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19100);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("keep-alive-ok"),
            Expr::IntLit(2, dummy_span()), // maxRequests=2
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send 2 requests on the SAME connection (HTTP/1.1 default keep-alive)
    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET /first HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();

    // Read first response
    let responses = read_responses(&mut client, 1);
    assert!(!responses.is_empty(), "Should receive first response");
    assert!(
        responses[0].contains("200 OK"),
        "First response should be 200 OK, got: {}",
        responses[0]
    );
    assert!(
        responses[0].contains("keep-alive-ok"),
        "First response should contain body"
    );

    // Send second request on same connection
    std::io::Write::write_all(
        &mut client,
        b"GET /second HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();

    // Read second response
    let responses2 = read_responses(&mut client, 1);
    assert!(!responses2.is_empty(), "Should receive second response");
    assert!(
        responses2[0].contains("200 OK"),
        "Second response should be 200 OK, got: {}",
        responses2[0]
    );

    // Server should terminate (maxRequests=2 reached)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(2));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-1h: Connection: close → connection terminates after one request
#[test]
fn test_keep_alive_connection_close_terminates() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19200);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("close-ok"),
            Expr::IntLit(1, dummy_span()), // maxRequests=1
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send request with Connection: close
    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let resp = String::from_utf8_lossy(&response);
    assert!(resp.contains("200 OK"), "Should get 200 OK, got: {}", resp);
    assert!(resp.contains("close-ok"), "Response body should be present");

    // Server terminates (maxRequests=1 reached after one request with Connection: close)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-1h: HTTP/1.0 + Connection: keep-alive → connection maintained
#[test]
fn test_keep_alive_http10_explicit_keep_alive() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19300);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("http10-ka-ok"),
            Expr::IntLit(2, dummy_span()), // maxRequests=2
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // HTTP/1.0 with explicit Connection: keep-alive
    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET /first HTTP/1.0\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n",
    )
    .unwrap();

    // Read first response
    let responses = read_responses(&mut client, 1);
    assert!(
        !responses.is_empty(),
        "Should receive first HTTP/1.0 keep-alive response"
    );
    assert!(
        responses[0].contains("200 OK"),
        "First response should be 200 OK, got: {}",
        responses[0]
    );

    // Send second request (still HTTP/1.0 + keep-alive)
    std::io::Write::write_all(
        &mut client,
        b"GET /second HTTP/1.0\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n",
    )
    .unwrap();

    // Read second response
    let responses2 = read_responses(&mut client, 1);
    assert!(
        !responses2.is_empty(),
        "Should receive second HTTP/1.0 keep-alive response"
    );
    assert!(
        responses2[0].contains("200 OK"),
        "Second response should be 200 OK, got: {}",
        responses2[0]
    );

    // Server terminates (maxRequests=2)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(2));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-1h: HTTP/1.0 without Connection header → connection closes after one request
#[test]
fn test_keep_alive_http10_default_close() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19400);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        // maxRequests=2 but HTTP/1.0 should close after 1
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("http10-close-ok"),
            Expr::IntLit(2, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // HTTP/1.0 without Connection header → default close
    let mut client = connect_with_retry(port);
    std::io::Write::write_all(&mut client, b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n").unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let resp = String::from_utf8_lossy(&response);
    assert!(resp.contains("200 OK"), "Should get 200, got: {}", resp);
    assert!(resp.contains("http10-close-ok"), "Body should be present");

    // Connection should be closed after this single request.
    // Server is still running (maxRequests=2, only used 1).
    // Send another connection to consume the second request and terminate.
    let mut client2 = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client2,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let mut response2 = Vec::new();
    let _ = client2.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client2, &mut buf) {
            Ok(0) => break,
            Ok(n) => response2.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    let resp2 = String::from_utf8_lossy(&response2);
    assert!(resp2.contains("200 OK"), "Second connection should get 200");

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(2));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-1h: maxRequests across connections — verify count is global
#[test]
fn test_keep_alive_max_requests_across_connections() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19500);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("max-req-ok"),
            Expr::IntLit(3, dummy_span()), // maxRequests=3 total
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Connection 1: send 2 requests (keep-alive)
    let mut client1 = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client1,
        b"GET /req1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();
    let resp1 = read_responses(&mut client1, 1);
    assert!(!resp1.is_empty() && resp1[0].contains("200 OK"));

    std::io::Write::write_all(
        &mut client1,
        b"GET /req2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let resp2 = read_responses(&mut client1, 1);
    assert!(!resp2.is_empty() && resp2[0].contains("200 OK"));
    drop(client1);

    // Connection 2: send 1 request (this is the 3rd overall → triggers maxRequests)
    let mut client2 = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client2,
        b"GET /req3 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();
    let resp3 = read_responses(&mut client2, 1);
    assert!(!resp3.is_empty() && resp3[0].contains("200 OK"));

    // Server should terminate (3 total requests reached)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(3));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// ── Chunked Transfer Encoding unit tests (NET2-2) ──

#[test]
fn test_chunked_compact_basic() {
    // "4\r\nWiki\r\n7\r\npedia i\r\n0\r\n\r\n"
    // Should compact to "Wikipedia i"
    let head = b"GET / HTTP/1.1\r\n\r\n";
    let chunked_body = b"4\r\nWiki\r\n7\r\npedia i\r\n0\r\n\r\n";
    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(chunked_body);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
    assert_eq!(result.body_len, 11); // "Wikipedia i" = 11 bytes
    assert_eq!(
        &buf[body_offset..body_offset + result.body_len],
        b"Wikipedia i"
    );
    // wire_consumed should cover all chunked framing
    assert_eq!(result.wire_consumed, chunked_body.len());
}

#[test]
fn test_chunked_compact_single_chunk() {
    // Single chunk: "5\r\nhello\r\n0\r\n\r\n"
    let head = b"POST /data HTTP/1.1\r\nHost: h\r\n\r\n";
    let chunked_body = b"5\r\nhello\r\n0\r\n\r\n";
    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(chunked_body);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
    assert_eq!(result.body_len, 5);
    assert_eq!(&buf[body_offset..body_offset + result.body_len], b"hello");
}

#[test]
fn test_chunked_compact_zero_only() {
    // Terminator only: "0\r\n\r\n" → empty body
    let head = b"GET / HTTP/1.1\r\n\r\n";
    let chunked_body = b"0\r\n\r\n";
    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(chunked_body);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
    assert_eq!(result.body_len, 0);
    assert_eq!(result.wire_consumed, 5); // "0\r\n\r\n"
}

#[test]
fn test_chunked_compact_hex_sizes() {
    // Hex chunk sizes: a (10) + 10 (16) = 26 bytes
    let head = b"GET / HTTP/1.1\r\n\r\n";
    let data_a = b"0123456789"; // 10 bytes
    let data_10 = b"abcdefghijklmnop"; // 16 bytes
    let mut chunked = Vec::new();
    chunked.extend_from_slice(b"a\r\n");
    chunked.extend_from_slice(data_a);
    chunked.extend_from_slice(b"\r\n");
    chunked.extend_from_slice(b"10\r\n");
    chunked.extend_from_slice(data_10);
    chunked.extend_from_slice(b"\r\n");
    chunked.extend_from_slice(b"0\r\n\r\n");

    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(&chunked);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
    assert_eq!(result.body_len, 26);
    assert_eq!(
        &buf[body_offset..body_offset + result.body_len],
        b"0123456789abcdefghijklmnop"
    );
}

#[test]
fn test_chunked_compact_chunk_ext_ignored() {
    // Chunk extensions should be ignored: "5;ext=val\r\nhello\r\n0\r\n\r\n"
    let head = b"GET / HTTP/1.1\r\n\r\n";
    let chunked_body = b"5;ext=val\r\nhello\r\n0\r\n\r\n";
    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(chunked_body);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
    assert_eq!(result.body_len, 5);
    assert_eq!(&buf[body_offset..body_offset + result.body_len], b"hello");
}

#[test]
fn test_chunked_compact_trailers_skipped() {
    // Trailers after 0 chunk: "0\r\nTrailer: val\r\n\r\n"
    let head = b"GET / HTTP/1.1\r\n\r\n";
    let chunked_body = b"5\r\nhello\r\n0\r\nTrailer: val\r\n\r\n";
    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(chunked_body);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset).unwrap();
    assert_eq!(result.body_len, 5);
    assert_eq!(&buf[body_offset..body_offset + result.body_len], b"hello");
}

#[test]
fn test_chunked_compact_malformed_chunk_size() {
    // Invalid hex in chunk size → error
    let head = b"GET / HTTP/1.1\r\n\r\n";
    let chunked_body = b"XY\r\nhello\r\n0\r\n\r\n";
    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(chunked_body);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid chunk-size"));
}

#[test]
fn test_chunked_compact_truncated_data() {
    // Chunk promises 10 bytes but only 5 available → truncated
    let head = b"GET / HTTP/1.1\r\n\r\n";
    let chunked_body = b"a\r\nhello";
    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(chunked_body);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("truncated"));
}

#[test]
fn test_chunked_compact_missing_crlf_after_data() {
    // Data present but CRLF after it is wrong
    let head = b"GET / HTTP/1.1\r\n\r\n";
    let chunked_body = b"5\r\nhelloXX0\r\n\r\n"; // XX instead of \r\n
    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(chunked_body);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("CRLF after chunk data"));
}

#[test]
fn test_chunked_compact_empty_chunk_size() {
    // Empty chunk size line → error
    let head = b"GET / HTTP/1.1\r\n\r\n";
    let chunked_body = b"\r\nhello\r\n0\r\n\r\n";
    let mut buf = Vec::new();
    buf.extend_from_slice(head);
    buf.extend_from_slice(chunked_body);
    let body_offset = head.len();

    let result = chunked_in_place_compact(&mut buf, body_offset);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty chunk-size"));
}

// ── parse_request_head: Transfer-Encoding detection (NET2-2a) ──

#[test]
fn test_parse_head_detects_chunked() {
    let raw = b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n";
    let result = parse_request_head(raw);
    let inner = extract_result_value(&result).unwrap();
    assert_eq!(get_field_bool(inner, "chunked"), Some(true));
    assert_eq!(get_field_int(inner, "contentLength"), Some(0));
}

#[test]
fn test_parse_head_no_chunked() {
    let raw = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let result = parse_request_head(raw);
    let inner = extract_result_value(&result).unwrap();
    assert_eq!(get_field_bool(inner, "chunked"), Some(false));
}

#[test]
fn test_parse_head_chunked_case_insensitive() {
    let raw = b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: Chunked\r\n\r\n";
    let result = parse_request_head(raw);
    let inner = extract_result_value(&result).unwrap();
    assert_eq!(get_field_bool(inner, "chunked"), Some(true));
}

#[test]
fn test_parse_head_chunked_in_token_list() {
    // "Transfer-Encoding: gzip, chunked"
    let raw = b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: gzip, chunked\r\n\r\n";
    let result = parse_request_head(raw);
    let inner = extract_result_value(&result).unwrap();
    assert_eq!(get_field_bool(inner, "chunked"), Some(true));
}

// ── parse_request_head: Content-Length + chunked rejection (NET2-2e) ──

#[test]
fn test_parse_head_rejects_cl_and_chunked() {
    // RFC 7230 §3.3.3: Content-Length + Transfer-Encoding: chunked = reject
    let raw = b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\nhello";
    let result = parse_request_head(raw);
    // Should be a failure result
    assert!(
        extract_result_value(&result).is_none(),
        "Should reject CL + TE:chunked"
    );
}

#[test]
fn test_parse_head_cl_without_chunked_ok() {
    // Content-Length without chunked should work fine
    let raw = b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello";
    let result = parse_request_head(raw);
    let inner = extract_result_value(&result).unwrap();
    assert_eq!(get_field_bool(inner, "chunked"), Some(false));
    assert_eq!(get_field_int(inner, "contentLength"), Some(5));
}

// ── C12-12 / FB-2: BodyEncoding internal representation ──

#[test]
fn test_body_encoding_classify_empty_when_all_zero() {
    // C12B-032: absent CL, no TE:chunked → Empty with
    // `had_content_length_header: false`.
    assert_eq!(
        BodyEncoding::classify(false, false, 0),
        BodyEncoding::Empty {
            had_content_length_header: false,
        },
    );
    // C12B-032: explicit `Content-Length: 0` collapses to Empty
    // at the wire layer but preserves the presence bit so the v2
    // chunked / HTTP/2 DATA promotion path can reconstruct the
    // original framing without re-parsing the request.
    assert_eq!(
        BodyEncoding::classify(false, true, 0),
        BodyEncoding::Empty {
            had_content_length_header: true,
        },
    );
}

/// C12B-032: `Empty` must expose the two sub-cases as distinct
/// values so downstream consumers (v2 chunked, HTTP/2 DATA
/// promotion) can branch on them. Before C12B-032 these were
/// indistinguishable at the enum level.
#[test]
fn test_body_encoding_empty_distinguishes_absent_vs_zero() {
    let absent = BodyEncoding::classify(false, false, 0);
    let zero = BodyEncoding::classify(false, true, 0);
    assert_ne!(
        absent, zero,
        "BodyEncoding must distinguish absent Content-Length from Content-Length: 0 \
         (C12B-032 / FB-2). Got both as {:?}",
        absent
    );
    assert!(!absent.had_content_length_header());
    assert!(zero.had_content_length_header());
}

#[test]
fn test_body_encoding_classify_content_length_positive() {
    assert_eq!(
        BodyEncoding::classify(false, true, 1),
        BodyEncoding::ContentLength(1),
    );
    assert_eq!(
        BodyEncoding::classify(false, true, 9_007_199_254_740_991),
        BodyEncoding::ContentLength(9_007_199_254_740_991),
    );
}

#[test]
fn test_body_encoding_classify_chunked_wins() {
    // TE:chunked alone is the Chunked variant. `content_length_val`
    // is meaningless when `has_chunked` is true (the parser rejects
    // the CL+chunked combination before we get here), but the
    // classifier must still preserve the chunked decision.
    assert_eq!(
        BodyEncoding::classify(true, false, 0),
        BodyEncoding::Chunked,
    );
}

#[test]
fn test_body_encoding_from_parsed_absent_body() {
    // GET with no body headers → Empty with presence bit = false.
    // `from_parsed_result_value` reconstructs from the flat surface
    // fields (`contentLength: 0`, `chunked: false`) so the bit is
    // conservatively inferred as false.
    let raw = b"GET / HTTP/1.1\r\nHost: h\r\n\r\n";
    let parsed = parse_request_head(raw);
    assert_eq!(
        BodyEncoding::from_parsed_result_value(&parsed),
        Some(BodyEncoding::Empty {
            had_content_length_header: false,
        }),
    );
}

#[test]
fn test_body_encoding_from_parsed_content_length_zero() {
    // C12B-032: `Content-Length: 0` and absent-CL are distinguished
    // at the parser's internal `__hasContentLengthHeader` field.
    // Through the handler-visible flattening, however, both appear
    // as `contentLength: 0 / chunked: false`, so the
    // backward-compatible `from_parsed_result_value` reconstruction
    // yields `had_content_length_header: false`. The internal
    // classifier path (`parse_request_head` → `ConnReadResult` →
    // `RequestBodyState::new`) preserves the true bit — verified
    // separately via `test_parse_head_records_cl_presence_bit`.
    let raw = b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 0\r\n\r\n";
    let parsed = parse_request_head(raw);
    assert_eq!(
        BodyEncoding::from_parsed_result_value(&parsed),
        Some(BodyEncoding::Empty {
            had_content_length_header: false,
        }),
    );
}

/// C12B-032: the internal `__hasContentLengthHeader` field on the
/// parsed BuchiPack must be `true` for an explicit
/// `Content-Length: 0` and `false` when the header is absent, so
/// the `ConnReadResult::Ready` path can preserve the distinction
/// into `RequestBodyState::new`.
#[test]
fn test_parse_head_records_cl_presence_bit() {
    let raw_absent = b"GET / HTTP/1.1\r\nHost: h\r\n\r\n";
    let parsed_absent = parse_request_head(raw_absent);
    let inner = extract_result_value(&parsed_absent).unwrap();
    assert_eq!(
        get_field_bool(inner, "__hasContentLengthHeader"),
        Some(false),
        "absent Content-Length must set __hasContentLengthHeader = false"
    );

    let raw_zero = b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 0\r\n\r\n";
    let parsed_zero = parse_request_head(raw_zero);
    let inner = extract_result_value(&parsed_zero).unwrap();
    assert_eq!(
        get_field_bool(inner, "__hasContentLengthHeader"),
        Some(true),
        "explicit `Content-Length: 0` must set __hasContentLengthHeader = true"
    );

    let raw_nonzero = b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello";
    let parsed_nonzero = parse_request_head(raw_nonzero);
    let inner = extract_result_value(&parsed_nonzero).unwrap();
    assert_eq!(
        get_field_bool(inner, "__hasContentLengthHeader"),
        Some(true),
        "`Content-Length: 5` must also set __hasContentLengthHeader = true"
    );
}

#[test]
fn test_body_encoding_from_parsed_content_length_positive() {
    let raw = b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nhello";
    let parsed = parse_request_head(raw);
    assert_eq!(
        BodyEncoding::from_parsed_result_value(&parsed),
        Some(BodyEncoding::ContentLength(5)),
    );
}

#[test]
fn test_body_encoding_from_parsed_chunked() {
    let raw = b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n";
    let parsed = parse_request_head(raw);
    assert_eq!(
        BodyEncoding::from_parsed_result_value(&parsed),
        Some(BodyEncoding::Chunked),
    );
}

#[test]
fn test_body_encoding_accessors() {
    let empty_absent = BodyEncoding::Empty {
        had_content_length_header: false,
    };
    let empty_zero = BodyEncoding::Empty {
        had_content_length_header: true,
    };
    assert_eq!(BodyEncoding::ContentLength(42).fixed_length(), Some(42));
    assert_eq!(BodyEncoding::Chunked.fixed_length(), None);
    assert_eq!(empty_absent.fixed_length(), None);
    assert_eq!(empty_zero.fixed_length(), None);

    assert!(empty_absent.is_empty());
    assert!(empty_zero.is_empty());
    assert!(!BodyEncoding::Chunked.is_empty());
    assert!(!BodyEncoding::ContentLength(1).is_empty());

    // C12B-032 accessor: had_content_length_header reads the bit.
    assert!(!empty_absent.had_content_length_header());
    assert!(empty_zero.had_content_length_header());
    assert!(BodyEncoding::ContentLength(1).had_content_length_header());
    assert!(!BodyEncoding::Chunked.had_content_length_header());
}

#[test]
fn test_request_body_state_records_encoding() {
    // The streaming body state must expose the same `BodyEncoding`
    // that the classifier produces so downstream consumers can
    // branch off a single source of truth (the existing
    // `is_chunked` / `content_length` fields remain as redundant
    // projections — see the struct doc-comment in net_eval.rs).
    //
    // C12B-032: the legacy 3-arg constructor (`new_legacy`)
    // conservatively infers `had_content_length_header` from
    // `content_length > 0`. Callers that need to distinguish
    // explicit `Content-Length: 0` from absent headers must use
    // the 4-arg `RequestBodyState::new` directly.
    let s_empty = RequestBodyState::new_legacy(false, 0, Vec::new());
    assert_eq!(
        s_empty.body_encoding,
        BodyEncoding::Empty {
            had_content_length_header: false,
        }
    );
    assert!(
        s_empty.fully_read,
        "Empty body must mark fully_read upfront"
    );

    let s_cl = RequestBodyState::new_legacy(false, 7, Vec::new());
    assert_eq!(s_cl.body_encoding, BodyEncoding::ContentLength(7));
    assert!(!s_cl.fully_read);

    let s_chunked = RequestBodyState::new_legacy(true, 0, Vec::new());
    assert_eq!(s_chunked.body_encoding, BodyEncoding::Chunked);
    assert!(!s_chunked.fully_read);

    // C12B-032: the 4-arg constructor preserves the presence bit
    // so explicit `Content-Length: 0` produces a distinct value
    // from absent-CL even though the wire behaviour is identical.
    let s_cl_zero = RequestBodyState::new(false, 0, true, Vec::new());
    assert_eq!(
        s_cl_zero.body_encoding,
        BodyEncoding::Empty {
            had_content_length_header: true,
        }
    );
    assert_ne!(s_cl_zero.body_encoding, s_empty.body_encoding);
    assert!(s_cl_zero.fully_read, "Empty CL:0 body still fully_read");
}

// ── readBody with chunked body (NET2-2h) ──

#[test]
fn test_read_body_chunked() {
    // After in-place compaction, raw contains head + compacted body.
    // The body span points to the compacted region.
    let head = b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n";
    let compacted_body = b"Wikipedia i"; // Result of compaction
    let mut raw = Vec::new();
    raw.extend_from_slice(head);
    raw.extend_from_slice(compacted_body);

    let body_start = head.len() as i64;
    let body_len = compacted_body.len() as i64;

    let req = Value::pack(vec![
        ("raw".into(), Value::bytes(raw)),
        (
            "body".into(),
            Value::pack(vec![
                ("start".into(), Value::Int(body_start)),
                ("len".into(), Value::Int(body_len)),
            ]),
        ),
    ]);
    let result = eval_read_body(&req).unwrap();
    assert_eq!(result, Value::bytes(b"Wikipedia i".to_vec()));
}

// ── httpServe integration test: chunked body (NET2-2i) ──

/// NET2-2i: httpServe with chunked request body
#[test]
fn test_http_serve_chunked_body() {
    use std::sync::atomic::{AtomicU16, Ordering};
    // NB2-11: Use distinct port range to avoid collision with test_keep_alive_max_requests_across_connections
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19700);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("chunked-echo"),
            Expr::IntLit(1, dummy_span()), // maxRequests=1
            Expr::IntLit(1000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);
    client
        .set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .unwrap();

    // Send a chunked request: "hello" + " world" = "hello world"
    let chunked_request = b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    std::io::Write::write_all(&mut client, chunked_request).unwrap();

    let mut response = Vec::new();
    let _ = std::io::Read::read_to_end(&mut client, &mut response);
    let response_str = String::from_utf8_lossy(&response);

    assert!(
        response_str.contains("200 OK"),
        "Expected 200 OK for chunked body, got: {}",
        response_str
    );
    assert!(
        response_str.contains("chunked-echo"),
        "Expected handler body text in response, got: {}",
        response_str
    );

    server_handle.join().unwrap();
}

/// NET2-2i: httpServe rejects Content-Length + Transfer-Encoding: chunked
#[test]
fn test_http_serve_rejects_cl_and_chunked() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19510);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("reject-test"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(1000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);
    client
        .set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .unwrap();

    // Send request with both Content-Length and Transfer-Encoding: chunked
    let bad_request = b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\nhello";
    std::io::Write::write_all(&mut client, bad_request).unwrap();

    let mut response = Vec::new();
    let _ = std::io::Read::read_to_end(&mut client, &mut response);
    let response_str = String::from_utf8_lossy(&response);

    assert!(
        response_str.contains("400 Bad Request"),
        "Expected 400 for CL+TE:chunked, got: {}",
        response_str
    );

    server_handle.join().unwrap();
}

/// NET2-2i: httpServe with malformed chunk size → 400
#[test]
fn test_http_serve_malformed_chunk() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19520);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("malformed-test"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(1000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);
    client
        .set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .unwrap();

    // Malformed chunk: "XY" is not valid hex
    let bad_request = b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nXY\r\nhello\r\n0\r\n\r\n";
    std::io::Write::write_all(&mut client, bad_request).unwrap();

    let mut response = Vec::new();
    let _ = std::io::Read::read_to_end(&mut client, &mut response);
    let response_str = String::from_utf8_lossy(&response);

    assert!(
        response_str.contains("400 Bad Request"),
        "Expected 400 for malformed chunk, got: {}",
        response_str
    );

    server_handle.join().unwrap();
}

/// NET2-2i: chunked body + keep-alive (chunked first, then Content-Length on same connection)
#[test]
fn test_http_serve_chunked_then_normal_keep_alive() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19530);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("mixed-test"),
            Expr::IntLit(2, dummy_span()), // maxRequests=2
            Expr::IntLit(2000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);
    client
        .set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .unwrap();

    // Request 1: chunked body (HTTP/1.1 keep-alive by default)
    let req1 = b"POST /first HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
    std::io::Write::write_all(&mut client, req1).unwrap();

    let resp1 = read_responses(&mut client, 1);
    assert!(
        !resp1.is_empty() && resp1[0].contains("200 OK"),
        "First chunked request should succeed, got: {:?}",
        resp1
    );

    // Request 2: normal Content-Length body on same connection
    let req2 = b"POST /second HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nworld";
    std::io::Write::write_all(&mut client, req2).unwrap();

    let resp2 = read_responses(&mut client, 1);
    assert!(
        !resp2.is_empty() && resp2[0].contains("200 OK"),
        "Second normal request should succeed on same connection, got: {:?}",
        resp2
    );

    // Server should terminate (maxRequests=2)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(2));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-2i: large chunked body (multiple chunks totaling > 8KB)
#[test]
fn test_http_serve_chunked_large_body() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19540);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("large-chunked"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(2000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);
    client
        .set_read_timeout(Some(std::time::Duration::from_millis(1000)))
        .unwrap();

    // Build a chunked body with 3 chunks of 4096 bytes each (12KB total)
    let chunk_data = vec![b'A'; 4096];
    let mut chunked_body = Vec::new();
    for _ in 0..3 {
        chunked_body.extend_from_slice(format!("{:x}\r\n", chunk_data.len()).as_bytes());
        chunked_body.extend_from_slice(&chunk_data);
        chunked_body.extend_from_slice(b"\r\n");
    }
    chunked_body.extend_from_slice(b"0\r\n\r\n");

    let mut request = Vec::new();
    request.extend_from_slice(
        b"POST /large HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
    );
    request.extend_from_slice(&chunked_body);

    std::io::Write::write_all(&mut client, &request).unwrap();

    let mut response = Vec::new();
    let _ = std::io::Read::read_to_end(&mut client, &mut response);
    let response_str = String::from_utf8_lossy(&response);

    assert!(
        response_str.contains("200 OK"),
        "Large chunked body should succeed, got: {}",
        response_str
    );

    server_handle.join().unwrap();
}

// ── NET2-3g: Concurrent handler dispatch tests ──

/// NET2-3g: Two clients connect simultaneously, both get responses.
#[test]
fn test_concurrent_two_clients_both_get_responses() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19600);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("concurrent-ok"),
            Expr::IntLit(2, dummy_span()), // maxRequests=2 (one per client)
            Expr::IntLit(5000, dummy_span()), // timeoutMs
            Expr::IntLit(4, dummy_span()), // maxConnections=4
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Connect two clients simultaneously before sending any data
    let mut client1 = connect_with_retry(port);
    let mut client2 = connect_with_retry(port);

    // Both send requests
    std::io::Write::write_all(
        &mut client1,
        b"GET /client1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    std::io::Write::write_all(
        &mut client2,
        b"GET /client2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    // Both should get responses
    let responses1 = read_responses(&mut client1, 1);
    let responses2 = read_responses(&mut client2, 1);

    assert!(!responses1.is_empty(), "Client 1 should receive a response");
    assert!(
        responses1[0].contains("200 OK"),
        "Client 1 should get 200 OK, got: {}",
        responses1[0]
    );
    assert!(
        responses1[0].contains("concurrent-ok"),
        "Client 1 should get body"
    );

    assert!(!responses2.is_empty(), "Client 2 should receive a response");
    assert!(
        responses2[0].contains("200 OK"),
        "Client 2 should get 200 OK, got: {}",
        responses2[0]
    );
    assert!(
        responses2[0].contains("concurrent-ok"),
        "Client 2 should get body"
    );

    // Server should terminate (maxRequests=2)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(2));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-3g: maxConnections limits simultaneous connections.
/// Excess connections must wait for a slot or be processed later.
#[test]
fn test_concurrent_max_connections_limit() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19610);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("limited-ok"),
            Expr::IntLit(3, dummy_span()),    // maxRequests=3
            Expr::IntLit(5000, dummy_span()), // timeoutMs
            Expr::IntLit(2, dummy_span()),    // maxConnections=2
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Connect 3 clients. maxConnections=2, so only 2 can be in the pool at once.
    // The third will be accepted after one of the first two closes.
    let mut client1 = connect_with_retry(port);
    let mut client2 = connect_with_retry(port);

    // Send requests on first two
    std::io::Write::write_all(
        &mut client1,
        b"GET /c1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    std::io::Write::write_all(
        &mut client2,
        b"GET /c2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    // Both get responses
    let r1 = read_responses(&mut client1, 1);
    let r2 = read_responses(&mut client2, 1);
    assert!(
        !r1.is_empty() && r1[0].contains("200 OK"),
        "Client 1 should get 200"
    );
    assert!(
        !r2.is_empty() && r2[0].contains("200 OK"),
        "Client 2 should get 200"
    );

    // Drop clients to free slots
    drop(client1);
    drop(client2);

    // Third client can now connect (after slots free up)
    std::thread::sleep(std::time::Duration::from_millis(50));
    let mut client3 = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client3,
        b"GET /c3 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let r3 = read_responses(&mut client3, 1);
    assert!(
        !r3.is_empty() && r3[0].contains("200 OK"),
        "Client 3 should get 200"
    );

    // Server should terminate (maxRequests=3)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(3));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-3g: maxRequests counts across all connections.
#[test]
fn test_concurrent_max_requests_across_connections() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19620);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("counted"),
            Expr::IntLit(3, dummy_span()), // maxRequests=3 total
            Expr::IntLit(5000, dummy_span()),
            Expr::IntLit(4, dummy_span()), // maxConnections=4
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Client 1: keep-alive, sends 2 requests
    let mut client1 = connect_with_retry(port);
    std::io::Write::write_all(&mut client1, b"GET /r1 HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let r1 = read_responses(&mut client1, 1);
    assert!(!r1.is_empty() && r1[0].contains("200 OK"));

    std::io::Write::write_all(&mut client1, b"GET /r2 HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let r2 = read_responses(&mut client1, 1);
    assert!(!r2.is_empty() && r2[0].contains("200 OK"));

    // Client 2: sends 1 request (should be the 3rd total, hitting maxRequests)
    let mut client2 = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client2,
        b"GET /r3 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let r3 = read_responses(&mut client2, 1);
    assert!(!r3.is_empty() && r3[0].contains("200 OK"));

    // Server should terminate (maxRequests=3 reached)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(3));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-3g: Per-connection buffer isolation.
/// Verify that data from one connection does not leak into another.
#[test]
fn test_concurrent_buffer_isolation() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19630);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("isolated"),
            Expr::IntLit(2, dummy_span()), // maxRequests=2
            Expr::IntLit(5000, dummy_span()),
            Expr::IntLit(4, dummy_span()), // maxConnections=4
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Client 1: POST with body "AAAA"
    let mut client1 = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client1,
        b"POST /c1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\nAAAA",
    )
    .unwrap();

    // Client 2: POST with body "BBBB"
    let mut client2 = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client2,
        b"POST /c2 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\nBBBB",
    )
    .unwrap();

    // Both should get their own responses without data leakage
    let r1 = read_responses(&mut client1, 1);
    let r2 = read_responses(&mut client2, 1);

    assert!(
        !r1.is_empty() && r1[0].contains("200 OK"),
        "Client 1 should get 200"
    );
    assert!(
        !r2.is_empty() && r2[0].contains("200 OK"),
        "Client 2 should get 200"
    );

    // Server terminates
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(2));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-3g: maxConnections defaults to 128 when not specified (v1 compat).
#[test]
fn test_concurrent_max_connections_default() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19640);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        // No maxConnections arg — should default to 128
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("default-mc"),
            Expr::IntLit(1, dummy_span()), // maxRequests=1
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Single client should work fine with default maxConnections
    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET /test HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let r = read_responses(&mut client, 1);
    assert!(!r.is_empty() && r[0].contains("200 OK"));
    assert!(r[0].contains("default-mc"));

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-3g: Keep-alive works correctly in concurrent mode.
/// One connection does keep-alive while another connects separately.
#[test]
fn test_concurrent_keep_alive_with_multiple_connections() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19650);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("ka-concurrent"),
            Expr::IntLit(3, dummy_span()), // maxRequests=3 total
            Expr::IntLit(5000, dummy_span()),
            Expr::IntLit(4, dummy_span()), // maxConnections=4
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Client 1: keep-alive, 2 requests
    let mut client1 = connect_with_retry(port);

    // Client 2: single request with Connection: close
    let mut client2 = connect_with_retry(port);

    // Client 1 first request
    std::io::Write::write_all(
        &mut client1,
        b"GET /ka1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();
    let r1 = read_responses(&mut client1, 1);
    assert!(!r1.is_empty() && r1[0].contains("200 OK"));

    // Client 2 request
    std::io::Write::write_all(
        &mut client2,
        b"GET /close HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let r2 = read_responses(&mut client2, 1);
    assert!(!r2.is_empty() && r2[0].contains("200 OK"));

    // Client 1 second request (keep-alive, this is the 3rd total)
    std::io::Write::write_all(
        &mut client1,
        b"GET /ka2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let r3 = read_responses(&mut client1, 1);
    assert!(!r3.is_empty() && r3[0].contains("200 OK"));

    // Server should terminate (maxRequests=3)
    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(3));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// NET2-3g: Chunked body works with concurrent connections.
#[test]
fn test_concurrent_chunked_body() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19660);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("chunked-concurrent"),
            Expr::IntLit(2, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
            Expr::IntLit(4, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Client 1: normal Content-Length request
    let mut client1 = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client1,
        b"POST /normal HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    )
    .unwrap();

    // Client 2: chunked request
    let mut client2 = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client2,
        b"POST /chunked HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nworld\r\n0\r\n\r\n",
    )
    .unwrap();

    let r1 = read_responses(&mut client1, 1);
    let r2 = read_responses(&mut client2, 1);

    assert!(
        !r1.is_empty() && r1[0].contains("200 OK"),
        "Normal request should succeed"
    );
    assert!(
        !r2.is_empty() && r2[0].contains("200 OK"),
        "Chunked request should succeed"
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(2));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// Regression: keep-alive partial request timeout returns 400 (not silent close).
///
/// Scenario: send a complete first request (200 OK, keep-alive), then send a
/// partial second request and let it timeout. The server must respond with 400
/// Bad Request (not silently close the connection) and count the request.
#[test]
fn test_keep_alive_partial_timeout_returns_400() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19670);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("partial-timeout-test"),
            Expr::IntLit(3, dummy_span()), // maxRequests=3 (1 real + 1 partial-timeout + 1 margin)
            Expr::IntLit(300, dummy_span()), // timeoutMs=300 (short for test speed)
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Phase 1: send a complete request on keep-alive connection
    let mut client = connect_with_retry(port);
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    std::io::Write::write_all(
        &mut client,
        b"GET /first HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .unwrap();

    let resp1 = read_responses(&mut client, 1);
    assert!(
        !resp1.is_empty() && resp1[0].contains("200 OK"),
        "First request should get 200 OK, got: {:?}",
        resp1
    );

    // Phase 2: send a PARTIAL second request (incomplete head) and let it timeout
    std::io::Write::write_all(
        &mut client,
        b"GET /second HTTP/1.1\r\nHost: lo", // intentionally incomplete
    )
    .unwrap();

    // Wait for the server's 300ms timeout to fire
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Read whatever the server sends back (should be 400, not EOF)
    let _ = client.set_read_timeout(Some(std::time::Duration::from_millis(500)));
    let mut buf = [0u8; 4096];
    let mut all_data = Vec::new();
    loop {
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => all_data.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    let response = String::from_utf8_lossy(&all_data).to_string();
    assert!(
        response.contains("400 Bad Request"),
        "Partial request on keep-alive should get 400 Bad Request, got: {:?}",
        response
    );

    // Server should count the partial as a request (total=2).
    // Send one more request on a fresh connection to reach maxRequests=3.
    let mut client2 = connect_with_retry(port);
    let _ = client2.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    std::io::Write::write_all(
        &mut client2,
        b"GET /third HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let resp3 = read_responses(&mut client2, 1);
    assert!(
        !resp3.is_empty() && resp3[0].contains("200 OK"),
        "Third request should get 200 OK"
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(
                get_field_int(inner, "requests"),
                Some(3),
                "Server should count partial timeout as a request (1 OK + 1 partial-400 + 1 OK = 3)"
            );
        }
        _ => panic!("expected fulfilled Async"),
    }
}

/// Regression: slow-split request within timeout window is processed normally.
///
/// Scenario: send a request head in two parts, each spaced under the timeout
/// window. The server should reassemble and return 200 OK (not 400).
#[test]
fn test_slow_split_request_within_timeout_succeeds() {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(19680);
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_handler_expr("slow-split-ok"),
            Expr::IntLit(1, dummy_span()),   // maxRequests=1
            Expr::IntLit(500, dummy_span()), // timeoutMs=500
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut client = connect_with_retry(port);
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(5)));

    // Send first half of the request head
    std::io::Write::write_all(&mut client, b"GET /split HTTP/1.1\r\nHost: localhost\r\n").unwrap();

    // Wait 200ms — under the 500ms timeout, but long enough to trigger
    // a timeout if last_activity were not updated on byte reception.
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Send the rest (completing the head with double CRLF)
    std::io::Write::write_all(&mut client, b"Connection: close\r\n\r\n").unwrap();

    let resp = read_responses(&mut client, 1);
    assert!(
        !resp.is_empty() && resp[0].contains("200 OK"),
        "Slow-split request should be processed normally (200 OK), got: {:?}",
        resp
    );
    assert!(
        resp[0].contains("slow-split-ok"),
        "Response body should contain handler output"
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// ── NET3 Phase 1 tests ──

// NET3-1b: Writer state machine
#[test]
fn test_writer_state_initial() {
    let writer = StreamingWriter::new();
    assert_eq!(writer.state, WriterState::Idle);
    assert_eq!(writer.pending_status, 200);
    assert!(writer.pending_headers.is_empty());
    assert!(!writer.sse_mode);
}

#[test]
fn test_writer_state_transitions() {
    let mut writer = StreamingWriter::new();
    assert_eq!(writer.state, WriterState::Idle);

    // Idle -> HeadPrepared (via startResponse)
    writer.state = WriterState::HeadPrepared;
    assert_eq!(writer.state, WriterState::HeadPrepared);

    // HeadPrepared -> Streaming (via writeChunk)
    writer.state = WriterState::Streaming;
    assert_eq!(writer.state, WriterState::Streaming);

    // Streaming -> Ended (via endResponse)
    writer.state = WriterState::Ended;
    assert_eq!(writer.state, WriterState::Ended);
}

// NET3-1c: startResponse pending state
#[test]
fn test_start_response_pending_state() {
    let mut writer = StreamingWriter::new();
    assert_eq!(writer.pending_status, 200);

    // Update pending status/headers
    writer.pending_status = 201;
    writer.pending_headers = vec![("X-Custom".to_string(), "value1".to_string())];
    writer.state = WriterState::HeadPrepared;

    assert_eq!(writer.pending_status, 201);
    assert_eq!(writer.pending_headers.len(), 1);
    assert_eq!(writer.pending_headers[0].0, "X-Custom");
    assert_eq!(writer.pending_headers[0].1, "value1");
}

// NET3-1d: Head commit builds correct wire format
#[test]
fn test_build_streaming_head_basic() {
    let headers = vec![("Content-Type".to_string(), "text/plain".to_string())];
    let head = build_streaming_head(200, &headers);
    let head_str = String::from_utf8(head).unwrap();

    assert!(head_str.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(head_str.contains("Content-Type: text/plain\r\n"));
    assert!(head_str.contains("Transfer-Encoding: chunked\r\n"));
    assert!(head_str.ends_with("\r\n\r\n"));
}

#[test]
fn test_build_streaming_head_custom_status() {
    let head = build_streaming_head(404, &[]);
    let head_str = String::from_utf8(head).unwrap();
    assert!(head_str.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(head_str.contains("Transfer-Encoding: chunked\r\n"));
}

#[test]
fn test_build_streaming_head_multiple_headers() {
    let headers = vec![
        ("Content-Type".to_string(), "text/html".to_string()),
        ("X-Foo".to_string(), "bar".to_string()),
        ("Cache-Control".to_string(), "no-cache".to_string()),
    ];
    let head = build_streaming_head(200, &headers);
    let head_str = String::from_utf8(head).unwrap();

    assert!(head_str.contains("Content-Type: text/html\r\n"));
    assert!(head_str.contains("X-Foo: bar\r\n"));
    assert!(head_str.contains("Cache-Control: no-cache\r\n"));
    assert!(head_str.contains("Transfer-Encoding: chunked\r\n"));
}

// NET3-1e: Reserved header rejection
#[test]
fn test_reserved_header_content_length_rejected() {
    let headers = vec![("Content-Length".to_string(), "42".to_string())];
    let result = StreamingWriter::validate_reserved_headers(&headers);
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(
        msg.contains("Content-Length"),
        "error should mention Content-Length: {}",
        msg
    );
}

#[test]
fn test_reserved_header_transfer_encoding_rejected() {
    let headers = vec![("Transfer-Encoding".to_string(), "chunked".to_string())];
    let result = StreamingWriter::validate_reserved_headers(&headers);
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(
        msg.contains("Transfer-Encoding"),
        "error should mention Transfer-Encoding: {}",
        msg
    );
}

#[test]
fn test_reserved_header_case_insensitive() {
    // content-length (lowercase)
    let headers = vec![("content-length".to_string(), "42".to_string())];
    assert!(StreamingWriter::validate_reserved_headers(&headers).is_err());

    // TRANSFER-ENCODING (uppercase)
    let headers = vec![("TRANSFER-ENCODING".to_string(), "chunked".to_string())];
    assert!(StreamingWriter::validate_reserved_headers(&headers).is_err());
}

#[test]
fn test_non_reserved_headers_allowed() {
    let headers = vec![
        ("Content-Type".to_string(), "text/plain".to_string()),
        ("X-Custom".to_string(), "value".to_string()),
        ("Cache-Control".to_string(), "no-cache".to_string()),
    ];
    assert!(StreamingWriter::validate_reserved_headers(&headers).is_ok());
}

#[test]
fn test_empty_headers_allowed() {
    let headers: Vec<(String, String)> = vec![];
    assert!(StreamingWriter::validate_reserved_headers(&headers).is_ok());
}

// NET3-1f: Bodyless status validation
#[test]
fn test_bodyless_status_detection() {
    // 1xx
    assert!(StreamingWriter::is_bodyless_status(100));
    assert!(StreamingWriter::is_bodyless_status(101));
    assert!(StreamingWriter::is_bodyless_status(199));
    // 204, 205, 304
    assert!(StreamingWriter::is_bodyless_status(204));
    assert!(StreamingWriter::is_bodyless_status(205));
    assert!(StreamingWriter::is_bodyless_status(304));
    // Normal statuses are NOT bodyless
    assert!(!StreamingWriter::is_bodyless_status(200));
    assert!(!StreamingWriter::is_bodyless_status(201));
    assert!(!StreamingWriter::is_bodyless_status(301));
    assert!(!StreamingWriter::is_bodyless_status(302));
    assert!(!StreamingWriter::is_bodyless_status(400));
    assert!(!StreamingWriter::is_bodyless_status(404));
    assert!(!StreamingWriter::is_bodyless_status(500));
}

// NET3-1d: http_reason_phrase coverage
#[test]
fn test_http_reason_phrases() {
    assert_eq!(http_reason_phrase(200), "OK");
    assert_eq!(http_reason_phrase(201), "Created");
    assert_eq!(http_reason_phrase(204), "No Content");
    assert_eq!(http_reason_phrase(301), "Moved Permanently");
    assert_eq!(http_reason_phrase(400), "Bad Request");
    assert_eq!(http_reason_phrase(404), "Not Found");
    assert_eq!(http_reason_phrase(500), "Internal Server Error");
    assert_eq!(http_reason_phrase(999), "Unknown");
}

// NET3-1a: v3 streaming API sentinel guard
#[test]
fn test_v3_api_sentinel_without_import() {
    let mut interp = Interpreter::new();
    let args: Vec<Expr> = vec![];
    // Without sentinel, these should return None (not dispatched)
    assert!(
        interp
            .try_net_func("startResponse", &args)
            .unwrap()
            .is_none()
    );
    assert!(interp.try_net_func("writeChunk", &args).unwrap().is_none());
    assert!(interp.try_net_func("endResponse", &args).unwrap().is_none());
    assert!(interp.try_net_func("sseEvent", &args).unwrap().is_none());
}

#[test]
fn test_v3_api_sentinel_with_import_errors_outside_handler() {
    let mut interp = Interpreter::new();
    // Set sentinel as if imported from taida-lang/net
    for sym in &["startResponse", "writeChunk", "endResponse", "sseEvent"] {
        interp
            .env
            .define_force(sym, Value::str(format!("__net_builtin_{}", sym)));
    }
    let args: Vec<Expr> = vec![];
    // With sentinel but outside handler context, these should error
    for sym in &["startResponse", "writeChunk", "endResponse", "sseEvent"] {
        let result = interp.try_net_func(sym, &args);
        assert!(result.is_err(), "{} should error outside handler", sym);
        let msg = result.unwrap_err().message;
        assert!(
            msg.contains("2-argument httpServe handler"),
            "{} error should mention 2-arg handler: {}",
            sym,
            msg
        );
    }
}

// NET3-1a: 2-arg handler detection + one-shot fallback
// Build a 2-arg handler lambda expression (req, writer) that returns a response pack.
fn make_two_arg_handler_expr(body_text: &str) -> Expr {
    Expr::Lambda(
        vec![
            Param {
                name: "req".into(),
                type_annotation: None,
                default_value: None,
                span: dummy_span(),
            },
            Param {
                name: "writer".into(),
                type_annotation: None,
                default_value: None,
                span: dummy_span(),
            },
        ],
        Box::new(Expr::BuchiPack(
            vec![
                BuchiField {
                    name: "status".into(),
                    value: Expr::IntLit(200, dummy_span()),
                    span: dummy_span(),
                },
                BuchiField {
                    name: "headers".into(),
                    value: Expr::ListLit(
                        vec![Expr::BuchiPack(
                            vec![
                                BuchiField {
                                    name: "name".into(),
                                    value: Expr::StringLit("content-type".into(), dummy_span()),
                                    span: dummy_span(),
                                },
                                BuchiField {
                                    name: "value".into(),
                                    value: Expr::StringLit("text/plain".into(), dummy_span()),
                                    span: dummy_span(),
                                },
                            ],
                            dummy_span(),
                        )],
                        dummy_span(),
                    ),
                    span: dummy_span(),
                },
                BuchiField {
                    name: "body".into(),
                    value: Expr::StringLit(body_text.into(), dummy_span()),
                    span: dummy_span(),
                },
            ],
            dummy_span(),
        )),
        dummy_span(),
    )
}

/// Build a 2-arg handler that returns Unit (does nothing, no writer usage).
fn make_two_arg_noop_handler_expr() -> Expr {
    Expr::Lambda(
        vec![
            Param {
                name: "req".into(),
                type_annotation: None,
                default_value: None,
                span: dummy_span(),
            },
            Param {
                name: "writer".into(),
                type_annotation: None,
                default_value: None,
                span: dummy_span(),
            },
        ],
        Box::new(Expr::IntLit(1, dummy_span())),
        dummy_span(),
    )
}

/// Allocate a unique, bindable port for tests using a monotonic counter.
/// Same pattern as tests/parity.rs — avoids the TOCTOU race of bind(0)+drop.
fn v3_free_port() -> u16 {
    use std::sync::Once;
    use std::sync::atomic::{AtomicU16, Ordering};

    static INIT: Once = Once::new();
    static COUNTER: AtomicU16 = AtomicU16::new(0);

    INIT.call_once(|| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let seed = listener.local_addr().unwrap().port();
        COUNTER.store(seed, Ordering::Relaxed);
    });

    for _ in 0..200 {
        let port = COUNTER.fetch_add(1, Ordering::Relaxed);
        if !(10000..=65000).contains(&port) {
            // Counter drifted out of usable range — reseed from OS.
            // The OS-assigned port is verified bindable (listener is alive).
            // Advance the counter past it so no other caller gets the same number.
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let fresh = listener.local_addr().unwrap().port();
            COUNTER.store(fresh.wrapping_add(1), Ordering::Relaxed);
            // Drop the listener, then re-verify the port is still bindable.
            drop(listener);
            if (10000..=65000).contains(&fresh)
                && std::net::TcpListener::bind(("127.0.0.1", fresh)).is_ok()
            {
                return fresh;
            }
            // Rare: OS gave an out-of-range or immediately-reclaimed port. Retry.
            continue;
        }
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("v3_free_port: could not find a free port after 200 attempts");
}

#[test]
fn test_v3_two_arg_handler_one_shot_fallback() {
    let port = v3_free_port();

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_two_arg_handler_expr("fallback-ok"),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1 200 OK"),
        "2-arg one-shot fallback should return 200 OK: {}",
        response_str
    );
    assert!(
        response_str.contains("fallback-ok"),
        "2-arg one-shot fallback should return handler body: {}",
        response_str
    );
    // One-shot uses Content-Length, not chunked
    assert!(
        response_str.contains("Content-Length"),
        "2-arg one-shot fallback should use Content-Length: {}",
        response_str
    );
    assert!(
        !response_str.contains("Transfer-Encoding: chunked"),
        "2-arg one-shot fallback should NOT use chunked TE: {}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

#[test]
fn test_v3_two_arg_handler_no_return_fallback() {
    let port = v3_free_port();

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            make_two_arg_noop_handler_expr(),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("HTTP/1.1 200 OK"),
        "2-arg no-return fallback should return 200: {}",
        response_str
    );
    // Empty body → Content-Length: 0
    assert!(
        response_str.contains("Content-Length: 0"),
        "2-arg no-return fallback should have Content-Length: 0: {}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
            let inner = extract_result_value(&a.value).unwrap();
            assert_eq!(get_field_bool(inner, "ok"), Some(true));
            assert_eq!(get_field_int(inner, "requests"), Some(1));
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// ── NET3 Phase 2 tests ──

use crate::parser::Statement;
use std::sync::Arc;

/// Build a streaming handler FuncValue that calls the given sequence of
/// streaming API calls. Each call is a Statement::Expr(Expr::FuncCall(...)).
/// The handler has params (req, writer) and body is the statements.
fn make_streaming_handler(stmts: Vec<Statement>) -> Value {
    Value::Function(super::super::value::FuncValue {
        name: "<streaming_handler>".to_string(),
        params: vec![
            Param {
                name: "req".into(),
                type_annotation: None,
                default_value: None,
                span: dummy_span(),
            },
            Param {
                name: "writer".into(),
                type_annotation: None,
                default_value: None,
                span: dummy_span(),
            },
        ],
        body: stmts,
        closure: Arc::new(std::collections::HashMap::new()),
        return_type: None,
        module_type_defs: None,
        module_enum_defs: None,
    })
}

/// Build `writeChunk(writer, "data")` expression
fn make_write_chunk_call(data: &str) -> Expr {
    Expr::FuncCall(
        Box::new(Expr::Ident("writeChunk".into(), dummy_span())),
        vec![
            Expr::Ident("writer".into(), dummy_span()),
            Expr::StringLit(data.into(), dummy_span()),
        ],
        dummy_span(),
    )
}

/// Build `writeChunk(writer, Bytes)` expression using a pre-set variable
fn make_write_chunk_bytes_call(_data: Vec<u8>) -> Expr {
    Expr::FuncCall(
        Box::new(Expr::Ident("writeChunk".into(), dummy_span())),
        vec![
            Expr::Ident("writer".into(), dummy_span()),
            // Use a pre-evaluated Bytes value via a variable
            Expr::Ident("__test_bytes_data".into(), dummy_span()),
        ],
        dummy_span(),
    )
}

/// Build `endResponse(writer)` expression
fn make_end_response_call() -> Expr {
    Expr::FuncCall(
        Box::new(Expr::Ident("endResponse".into(), dummy_span())),
        vec![Expr::Ident("writer".into(), dummy_span())],
        dummy_span(),
    )
}

/// Build `startResponse(writer, status, headers)` expression
fn make_start_response_call(status: i64, headers: Vec<(String, String)>) -> Expr {
    let header_list: Vec<Expr> = headers
        .into_iter()
        .map(|(name, value)| {
            Expr::BuchiPack(
                vec![
                    BuchiField {
                        name: "name".into(),
                        value: Expr::StringLit(name, dummy_span()),
                        span: dummy_span(),
                    },
                    BuchiField {
                        name: "value".into(),
                        value: Expr::StringLit(value, dummy_span()),
                        span: dummy_span(),
                    },
                ],
                dummy_span(),
            )
        })
        .collect();

    Expr::FuncCall(
        Box::new(Expr::Ident("startResponse".into(), dummy_span())),
        vec![
            Expr::Ident("writer".into(), dummy_span()),
            Expr::IntLit(status, dummy_span()),
            Expr::ListLit(header_list, dummy_span()),
        ],
        dummy_span(),
    )
}

/// Build `sseEvent(writer, event, data)` expression
fn make_sse_event_call(event: &str, data: &str) -> Expr {
    Expr::FuncCall(
        Box::new(Expr::Ident("sseEvent".into(), dummy_span())),
        vec![
            Expr::Ident("writer".into(), dummy_span()),
            Expr::StringLit(event.into(), dummy_span()),
            Expr::StringLit(data.into(), dummy_span()),
        ],
        dummy_span(),
    )
}

/// Helper to set up streaming API sentinels on an interpreter
fn setup_v3_sentinels(interp: &mut Interpreter) {
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
    for sym in &["startResponse", "writeChunk", "endResponse", "sseEvent"] {
        interp
            .env
            .define_force(sym, Value::str(format!("__net_builtin_{}", sym)));
    }
}

/// Read full HTTP response from a client stream
fn read_full_response(client: &mut std::net::TcpStream) -> String {
    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&response).to_string()
}

// NET3-2b: writeChunk sends chunked body
#[test]
fn test_v3_write_chunk_basic() {
    let port = v3_free_port();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_write_chunk_call("hello ")),
        Statement::Expr(make_write_chunk_call("world")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Should have chunked transfer encoding
    assert!(
        response_str.contains("Transfer-Encoding: chunked"),
        "chunked response should have Transfer-Encoding: chunked\n{}",
        response_str
    );
    // Should have HTTP/1.1 200 OK
    assert!(
        response_str.contains("HTTP/1.1 200 OK"),
        "chunked response should have 200 OK\n{}",
        response_str
    );
    // Should NOT have Content-Length
    assert!(
        !response_str.contains("Content-Length"),
        "chunked response should NOT have Content-Length\n{}",
        response_str
    );
    // Body should contain the chunked data
    assert!(
        response_str.contains("hello "),
        "response should contain 'hello '\n{}",
        response_str
    );
    assert!(
        response_str.contains("world"),
        "response should contain 'world'\n{}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-2c: endResponse sends chunked terminator
#[test]
fn test_v3_end_response_empty_body() {
    let port = v3_free_port();

    // endResponse without any writeChunk → empty chunked body
    let handler = make_streaming_handler(vec![Statement::Expr(make_end_response_call())]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    assert!(
        response_str.contains("Transfer-Encoding: chunked"),
        "empty chunked response should have Transfer-Encoding: chunked\n{}",
        response_str
    );
    assert!(
        response_str.contains("HTTP/1.1 200 OK"),
        "empty chunked response should have 200 OK\n{}",
        response_str
    );
    // Should end with chunked terminator (0\r\n\r\n)
    assert!(
        response_str.contains("0\r\n\r\n"),
        "empty chunked response should have terminator\n{}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-2c: endResponse is idempotent (double call)
#[test]
fn test_v3_end_response_idempotent() {
    let port = v3_free_port();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_write_chunk_call("data")),
        Statement::Expr(make_end_response_call()),
        Statement::Expr(make_end_response_call()), // second call is no-op
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    assert!(
        response_str.contains("Transfer-Encoding: chunked"),
        "response should be chunked\n{}",
        response_str
    );
    assert!(
        response_str.contains("data"),
        "response should contain 'data'\n{}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-2d: Bytes fast path (zero-copy)
#[test]
fn test_v3_write_chunk_bytes_fast_path() {
    let port = v3_free_port();

    let bytes_data = b"binary\x00payload\xff".to_vec();
    let bytes_data_clone = bytes_data.clone();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_write_chunk_bytes_call(bytes_data.clone())),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        // Pre-set the bytes data in the environment
        interp
            .env
            .define_force("__test_bytes_data", Value::bytes(bytes_data_clone));
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("Transfer-Encoding: chunked"),
        "bytes response should be chunked\n{}",
        response_str
    );
    // Binary payload should be present in raw bytes
    // The chunk format is: <hex_len>\r\n<payload>\r\n
    // Our payload is b"binary\x00payload\xff" = 15 bytes → hex "f"
    assert!(
        response.windows(b"binary".len()).any(|w| w == b"binary"),
        "response should contain binary data"
    );
    assert!(
        response.windows(b"payload".len()).any(|w| w == b"payload"),
        "response should contain 'payload' binary data"
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-2e: Str chunk without aggregate buffer
#[test]
fn test_v3_write_chunk_str_no_aggregate() {
    let port = v3_free_port();

    // Multiple string chunks to verify no aggregate buffer
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_write_chunk_call("chunk1")),
        Statement::Expr(make_write_chunk_call("chunk2")),
        Statement::Expr(make_write_chunk_call("chunk3")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    assert!(
        response_str.contains("Transfer-Encoding: chunked"),
        "str chunks response should be chunked\n{}",
        response_str
    );
    // All chunks should be present
    assert!(
        response_str.contains("chunk1"),
        "response should contain chunk1\n{}",
        response_str
    );
    assert!(
        response_str.contains("chunk2"),
        "response should contain chunk2\n{}",
        response_str
    );
    assert!(
        response_str.contains("chunk3"),
        "response should contain chunk3\n{}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-2f: startResponse + writeChunk + endResponse full flow
#[test]
fn test_v3_start_response_write_chunk_end_response() {
    let port = v3_free_port();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_start_response_call(
            201,
            vec![("X-Custom".into(), "streaming-test".into())],
        )),
        Statement::Expr(make_write_chunk_call("first")),
        Statement::Expr(make_write_chunk_call("second")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Custom status
    assert!(
        response_str.contains("HTTP/1.1 201 Created"),
        "response should have 201 Created\n{}",
        response_str
    );
    // Custom header
    assert!(
        response_str.contains("X-Custom: streaming-test"),
        "response should have custom header\n{}",
        response_str
    );
    // Chunked
    assert!(
        response_str.contains("Transfer-Encoding: chunked"),
        "response should be chunked\n{}",
        response_str
    );
    // Body chunks
    assert!(
        response_str.contains("first"),
        "response should contain 'first'\n{}",
        response_str
    );
    assert!(
        response_str.contains("second"),
        "response should contain 'second'\n{}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-2f: writeChunk empty data is no-op
#[test]
fn test_v3_write_chunk_empty_is_noop() {
    let port = v3_free_port();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_write_chunk_call("before")),
        Statement::Expr(make_write_chunk_call("")), // empty = no-op
        Statement::Expr(make_write_chunk_call("after")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    assert!(
        response_str.contains("before"),
        "response should contain 'before'\n{}",
        response_str
    );
    assert!(
        response_str.contains("after"),
        "response should contain 'after'\n{}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-2f: auto-end when handler returns without endResponse
#[test]
fn test_v3_auto_end_on_handler_return() {
    let port = v3_free_port();

    // Handler writes chunks but doesn't call endResponse → auto-end
    let handler = make_streaming_handler(vec![Statement::Expr(make_write_chunk_call(
        "auto-end-data",
    ))]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    assert!(
        response_str.contains("Transfer-Encoding: chunked"),
        "auto-end response should be chunked\n{}",
        response_str
    );
    assert!(
        response_str.contains("auto-end-data"),
        "response should contain body\n{}",
        response_str
    );
    // Should have terminator (auto-ended)
    assert!(
        response_str.contains("0\r\n\r\n"),
        "auto-ended response should have terminator\n{}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-2f: reserved header rejection in startResponse
#[test]
fn test_v3_reserved_header_rejection_in_handler() {
    let port = v3_free_port();

    // Handler calls startResponse with Content-Length → should error
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_start_response_call(
            200,
            vec![("Content-Length".into(), "42".into())],
        )),
        Statement::Expr(make_write_chunk_call("data")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Should get 500 error because startResponse rejected Content-Length
    assert!(
        response_str.contains("500"),
        "reserved header should cause 500 error\n{}",
        response_str
    );
    assert!(
        response_str.contains("Content-Length"),
        "error should mention Content-Length\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-2f: bodyless status rejects writeChunk
#[test]
fn test_v3_bodyless_status_rejects_write_chunk() {
    let port = v3_free_port();

    // Handler calls startResponse(204) then writeChunk → should error
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_start_response_call(204, vec![])),
        Statement::Expr(make_write_chunk_call("body-not-allowed")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Should get 500 error because 204 does not allow body
    assert!(
        response_str.contains("500"),
        "bodyless status should cause 500 error\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-2f: Verify chunked wire format correctness
#[test]
fn test_v3_chunked_wire_format() {
    let port = v3_free_port();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_write_chunk_call("AB")), // 2 bytes → hex "2"
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);
    // Verify exact chunked wire format:
    // "2\r\nAB\r\n0\r\n\r\n"
    assert!(
        response_str.contains("2\r\nAB\r\n"),
        "chunk should be '2\\r\\nAB\\r\\n', got:\n{}",
        response_str
    );
    assert!(
        response_str.ends_with("0\r\n\r\n"),
        "response should end with '0\\r\\n\\r\\n', got:\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-2f: implicit head commit on first writeChunk
#[test]
fn test_v3_implicit_head_commit() {
    let port = v3_free_port();

    // No startResponse → writeChunk should auto-commit 200/@[]
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_write_chunk_call("implicit-head")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Should default to 200 OK
    assert!(
        response_str.contains("HTTP/1.1 200 OK"),
        "implicit head should default to 200 OK\n{}",
        response_str
    );
    assert!(
        response_str.contains("Transfer-Encoding: chunked"),
        "implicit head should include chunked TE\n{}",
        response_str
    );
    assert!(
        response_str.contains("implicit-head"),
        "response should contain body\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// ── Phase 3: SSE tests ──────────────────────────────────────

// NET3-3a: sseEvent sends SSE wire format
#[test]
fn test_v3_sse_event_basic() {
    let port = v3_free_port();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_sse_event_call("message", "hello")),
        Statement::Expr(make_sse_event_call("message", "world")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Should have SSE content type
    assert!(
        response_str.contains("Content-Type: text/event-stream; charset=utf-8"),
        "SSE response should have text/event-stream Content-Type\n{}",
        response_str
    );
    // Should have Cache-Control: no-cache
    assert!(
        response_str.contains("Cache-Control: no-cache"),
        "SSE response should have Cache-Control: no-cache\n{}",
        response_str
    );
    // Should have Transfer-Encoding: chunked
    assert!(
        response_str.contains("Transfer-Encoding: chunked"),
        "SSE response should have chunked TE\n{}",
        response_str
    );
    // Should contain SSE wire format
    assert!(
        response_str.contains("event: message\ndata: hello\n\n"),
        "SSE response should contain first event wire format\n{}",
        response_str
    );
    assert!(
        response_str.contains("event: message\ndata: world\n\n"),
        "SSE response should contain second event wire format\n{}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-3b: Content-Type auto-setting
#[test]
fn test_v3_sse_auto_content_type() {
    let port = v3_free_port();

    // startResponse with no Content-Type header, then sseEvent → auto-set
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_start_response_call(200, vec![])),
        Statement::Expr(make_sse_event_call("ping", "pong")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    assert!(
        response_str.contains("Content-Type: text/event-stream; charset=utf-8"),
        "sseEvent should auto-set Content-Type\n{}",
        response_str
    );
    assert!(
        response_str.contains("Cache-Control: no-cache"),
        "sseEvent should auto-set Cache-Control\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-3b: Content-Type is NOT overwritten if user already set it
#[test]
fn test_v3_sse_respects_user_content_type() {
    let port = v3_free_port();

    // startResponse with explicit Content-Type → sseEvent should NOT override
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_start_response_call(
            200,
            vec![("Content-Type".to_string(), "text/plain".to_string())],
        )),
        Statement::Expr(make_sse_event_call("test", "data")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Should have the user-specified Content-Type, NOT the auto-set SSE one
    assert!(
        response_str.contains("Content-Type: text/plain"),
        "User Content-Type should be preserved\n{}",
        response_str
    );
    assert!(
        !response_str.contains("text/event-stream"),
        "Auto Content-Type should NOT override user-set Content-Type\n{}",
        response_str
    );
    // But Cache-Control should still be auto-set (user didn't set it)
    assert!(
        response_str.contains("Cache-Control: no-cache"),
        "Cache-Control should still be auto-set\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-3d: multiline data splitting
#[test]
fn test_v3_sse_multiline_data() {
    let port = v3_free_port();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_sse_event_call("update", "line1\nline2\nline3")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Multiline data should be split into multiple data: lines
    assert!(
        response_str.contains("event: update\ndata: line1\ndata: line2\ndata: line3\n\n"),
        "multiline data should be split into data: lines\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-3d: empty event name omits event: line
#[test]
fn test_v3_sse_empty_event_name() {
    let port = v3_free_port();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_sse_event_call("", "noname")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Should NOT contain "event:" line when event name is empty
    assert!(
        !response_str.contains("event:"),
        "empty event name should omit event: line\n{}",
        response_str
    );
    // Should contain "data: noname\n\n"
    assert!(
        response_str.contains("data: noname\n\n"),
        "data should still be sent\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-3f: sseEvent outside handler context
#[test]
fn test_v3_sse_event_outside_handler() {
    let mut interp = Interpreter::new();
    // Set sentinel but not inside handler
    interp
        .env
        .define_force("sseEvent", Value::str("__net_builtin_sseEvent".into()));
    let args: Vec<Expr> = vec![
        Expr::StringLit("writer".into(), dummy_span()),
        Expr::StringLit("message".into(), dummy_span()),
        Expr::StringLit("hello".into(), dummy_span()),
    ];
    let result = interp.try_net_func("sseEvent", &args);
    assert!(result.is_err(), "sseEvent should error outside handler");
    let msg = result.unwrap_err().message;
    assert!(
        msg.contains("2-argument httpServe handler"),
        "error should mention 2-arg handler: {}",
        msg
    );
}

// NET3-3f: sseEvent after endResponse
#[test]
fn test_v3_sse_event_after_end() {
    let port = v3_free_port();

    // endResponse first, then sseEvent → should error
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_end_response_call()),
        Statement::Expr(make_sse_event_call("message", "after-end")),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        // This will result in a runtime error from sseEvent after endResponse,
        // but httpServe catches handler errors and returns an error response.
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // The server should still respond (handler error is caught by httpServe).
    // The response might be a 500 error or the chunked response that was already sent.
    assert!(
        !response_str.is_empty(),
        "server should respond even when handler has SSE error"
    );

    let _ = server_handle.join();
}

// NET3-3f: SSE with implicit head commit (no startResponse)
#[test]
fn test_v3_sse_implicit_head() {
    let port = v3_free_port();

    // No startResponse → sseEvent should auto-commit 200 with SSE headers
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_sse_event_call("message", "auto-head")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Should have 200 OK
    assert!(
        response_str.contains("HTTP/1.1 200 OK"),
        "implicit head should default to 200 OK\n{}",
        response_str
    );
    // Should have SSE headers auto-set
    assert!(
        response_str.contains("Content-Type: text/event-stream; charset=utf-8"),
        "auto-head should include SSE Content-Type\n{}",
        response_str
    );
    assert!(
        response_str.contains("Cache-Control: no-cache"),
        "auto-head should include Cache-Control\n{}",
        response_str
    );
    // Should contain the SSE data
    assert!(
        response_str.contains("data: auto-head\n\n"),
        "response should contain SSE data\n{}",
        response_str
    );

    let result = server_handle.join().unwrap();
    match result {
        Signal::Value(Value::Async(a)) => {
            assert_eq!(a.status, AsyncStatus::Fulfilled);
        }
        _ => panic!("expected fulfilled Async"),
    }
}

// NET3-3f: SSE mixed with writeChunk
#[test]
fn test_v3_sse_mixed_with_write_chunk() {
    let port = v3_free_port();

    // Mix sseEvent and writeChunk — both should work on the same chunked stream
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_sse_event_call("greet", "hi")),
        Statement::Expr(make_write_chunk_call("raw-chunk")),
        Statement::Expr(make_sse_event_call("bye", "bye")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Should contain SSE event
    assert!(
        response_str.contains("event: greet\ndata: hi\n\n"),
        "first SSE event should be present\n{}",
        response_str
    );
    // Should contain raw chunk
    assert!(
        response_str.contains("raw-chunk"),
        "raw writeChunk data should be present\n{}",
        response_str
    );
    // Should contain second SSE event
    assert!(
        response_str.contains("event: bye\ndata: bye\n\n"),
        "second SSE event should be present\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-3f: writeChunk before sseEvent → error (head committed without SSE headers)
#[test]
fn test_v3_write_chunk_then_sse_event_errors() {
    let port = v3_free_port();

    // writeChunk commits head without SSE headers, then sseEvent should error
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_write_chunk_call("raw-first")),
        Statement::Expr(make_sse_event_call("message", "after-chunk")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // The handler should have errored on the sseEvent call — httpServe catches
    // runtime errors and returns a 500 or similar response.
    // The raw-first chunk was already sent before the error, so the response
    // will contain it. The SSE event should NOT be present.
    assert!(
        !response_str.contains("event: message"),
        "sseEvent should not succeed after writeChunk committed head\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-3f: startResponse with Content-Type only (no Cache-Control) + writeChunk + sseEvent → error
#[test]
fn test_v3_content_type_only_then_write_chunk_then_sse_event_errors() {
    let port = v3_free_port();

    // User sets Content-Type but NOT Cache-Control, then writeChunk commits head.
    // sseEvent should still error because Cache-Control: no-cache is also required.
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_start_response_call(
            200,
            vec![(
                "Content-Type".to_string(),
                "text/event-stream; charset=utf-8".to_string(),
            )],
        )),
        Statement::Expr(make_write_chunk_call("raw-first")),
        Statement::Expr(make_sse_event_call("message", "after-chunk")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // sseEvent should fail — Content-Type alone is not enough, Cache-Control is also required
    assert!(
        !response_str.contains("event: message"),
        "sseEvent should not succeed when only Content-Type is set (Cache-Control missing)\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-3f: startResponse with explicit SSE headers + writeChunk + sseEvent → OK
#[test]
fn test_v3_explicit_sse_headers_then_write_chunk_then_sse() {
    let port = v3_free_port();

    // User explicitly sets SSE headers via startResponse, then writeChunk, then sseEvent.
    // This should work because the user took responsibility for setting SSE headers.
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_start_response_call(
            200,
            vec![
                (
                    "Content-Type".to_string(),
                    "text/event-stream; charset=utf-8".to_string(),
                ),
                ("Cache-Control".to_string(), "no-cache".to_string()),
            ],
        )),
        Statement::Expr(make_write_chunk_call("preamble")),
        Statement::Expr(make_sse_event_call("update", "data-after-chunk")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // Verify SSE headers are present in the response head
    assert!(
        response_str.contains("text/event-stream"),
        "explicit SSE Content-Type should be in response\n{}",
        response_str
    );
    assert!(
        response_str.contains("no-cache"),
        "explicit Cache-Control should be in response\n{}",
        response_str
    );

    // The preamble chunk should be present
    assert!(
        response_str.contains("preamble"),
        "preamble writeChunk should be present\n{}",
        response_str
    );

    // The SSE event should be present (sse_mode was set by explicit headers check)
    assert!(
        response_str.contains("event: update\ndata: data-after-chunk\n\n"),
        "sseEvent should succeed when SSE headers were explicitly set\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-3f: SSE wire format exactness
#[test]
fn test_v3_sse_wire_format_exact() {
    let port = v3_free_port();

    let handler = make_streaming_handler(vec![
        Statement::Expr(make_sse_event_call("tick", "42")),
        Statement::Expr(make_end_response_call()),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let mut response = Vec::new();
    let _ = client.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    loop {
        let mut buf = [0u8; 4096];
        match std::io::Read::read(&mut client, &mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let response_str = String::from_utf8_lossy(&response);

    // SSE event is sent as one chunked frame via vectored I/O.
    // The payload "event: tick\ndata: 42\n\n" is 23 bytes → hex "17"
    let expected_sse = "event: tick\ndata: 42\n\n";
    let expected_hex = format!("{:x}", expected_sse.len());
    let expected_chunk = format!("{}\r\n{}\r\n", expected_hex, expected_sse);
    assert!(
        response_str.contains(&expected_chunk),
        "SSE chunk wire format mismatch.\nExpected chunk: {:?}\nGot:\n{}",
        expected_chunk,
        response_str
    );
    // Should end with chunked terminator
    assert!(
        response_str.ends_with("0\r\n\r\n"),
        "response should end with chunked terminator\n{}",
        response_str
    );

    let _ = server_handle.join();
}

// NET3-3f: SSE bodyless status rejection
#[test]
fn test_v3_sse_bodyless_status_rejected() {
    let port = v3_free_port();

    // startResponse with 204 (No Content), then sseEvent → should error
    let handler = make_streaming_handler(vec![
        Statement::Expr(make_start_response_call(204, vec![])),
        Statement::Expr(make_sse_event_call("message", "should-fail")),
    ]);

    let server_port = port;
    let server_handle = std::thread::spawn(move || {
        let mut interp = Interpreter::new();
        setup_v3_sentinels(&mut interp);
        interp.env.define_force("__handler", handler);
        let args = vec![
            Expr::IntLit(server_port as i64, dummy_span()),
            Expr::Ident("__handler".into(), dummy_span()),
            Expr::IntLit(1, dummy_span()),
            Expr::IntLit(5000, dummy_span()),
        ];
        interp.try_net_func("httpServe", &args).unwrap().unwrap()
    });

    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut client = connect_with_retry(port);
    std::io::Write::write_all(
        &mut client,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .unwrap();

    let response_str = read_full_response(&mut client);

    // The handler error should be caught by httpServe — we should get some response
    assert!(
        !response_str.is_empty(),
        "server should respond even with bodyless status SSE error"
    );

    let _ = server_handle.join();
}

// ── v5 Phase 1 tests ──

#[test]
fn test_net_symbols_includes_ws_close_code() {
    assert!(
        NET_SYMBOLS.contains(&"wsCloseCode"),
        "NET_SYMBOLS must include wsCloseCode (v5)"
    );
}

#[test]
fn test_ws_close_code_not_in_ws_state() {
    // wsCloseCode outside handler should fail.
    let mut interp = Interpreter::new();
    interp.env.define_force(
        "wsCloseCode",
        Value::str("__net_builtin_wsCloseCode".into()),
    );
    let args = vec![Expr::Ident("dummy".into(), dummy_span())];
    let result = interp.try_net_func("wsCloseCode", &args);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("wsCloseCode"));
}

#[test]
fn test_ws_close_dispatch_with_code_arg() {
    // wsClose with 2 args (ws, code) dispatches correctly.
    // Out of handler context, the handler-context guard rejects the call
    // before reaching close code validation. This only verifies dispatch
    // routing — actual close code validation is covered by parity tests
    // (test_net5_5a_ws_close_reserved_codes_interp, test_net5_5a_ws_close_out_of_range_interp).
    for code in &[1004i64, 1005, 1006, 1015, 0, 999, 5000, -1] {
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("wsClose", Value::str("__net_builtin_wsClose".into()));
        let args = vec![
            Expr::Ident("dummy".into(), dummy_span()),
            Expr::IntLit(*code, dummy_span()),
        ];
        let result = interp.try_net_func("wsClose", &args);
        assert!(
            result.is_err(),
            "wsClose({}) should fail outside handler context",
            code
        );
    }
}

#[test]
fn test_http_serve_tls_arg_empty_pack_accepted() {
    // httpServe with tls <= @() should be accepted (plaintext mode).
    // Define test_handler as a non-Function value to trigger the handler type check.
    // This proves the tls arg parsing (index 5) succeeds without interference.
    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));
    interp.env.define_force("test_handler", Value::Int(42));

    let args = vec![
        Expr::IntLit(0, dummy_span()),
        Expr::Ident("test_handler".into(), dummy_span()),
        Expr::IntLit(1, dummy_span()),
        Expr::IntLit(100, dummy_span()),
        Expr::IntLit(1, dummy_span()),
        Expr::BuchiPack(vec![], dummy_span()), // tls = @()
    ];

    let result = interp.try_net_func("httpServe", &args);
    assert!(result.is_err());
    let msg = result.unwrap_err().message;
    assert!(
        msg.contains("handler must be a Function"),
        "Should fail on handler, not on tls arg. Got: {}",
        msg
    );
}

#[test]
fn test_http_serve_tls_arg_non_pack_rejected() {
    // httpServe with tls as non-BuchiPack should be rejected.
    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));

    // Define a dummy handler function.
    interp.env.define_force(
        "test_handler",
        Value::Function(super::super::value::FuncValue {
            name: "test_handler".into(),
            params: vec![Param {
                name: "req".into(),
                default_value: None,
                type_annotation: None,
                span: dummy_span(),
            }],
            body: vec![],
            closure: std::sync::Arc::new(std::collections::HashMap::new()),
            return_type: None,
            module_type_defs: None,
            module_enum_defs: None,
        }),
    );

    let args = vec![
        Expr::IntLit(0, dummy_span()),
        Expr::Ident("test_handler".into(), dummy_span()),
        Expr::IntLit(1, dummy_span()),
        Expr::IntLit(100, dummy_span()),
        Expr::IntLit(1, dummy_span()),
        Expr::IntLit(42, dummy_span()), // tls = 42 (not a BuchiPack)
    ];

    let result = interp.try_net_func("httpServe", &args);
    assert!(result.is_err());
    let msg = result.unwrap_err().message;
    assert!(
        msg.contains("tls must be a BuchiPack"),
        "Should reject non-BuchiPack tls. Got: {}",
        msg
    );
}

#[test]
fn test_http_serve_tls_cert_key_returns_phase2_error() {
    // httpServe with tls = @(cert: "x", key: "y") should return TlsError (Phase 2 not ready).
    let mut interp = Interpreter::new();
    interp
        .env
        .define_force("httpServe", Value::str("__net_builtin_httpServe".into()));

    interp.env.define_force(
        "test_handler",
        Value::Function(super::super::value::FuncValue {
            name: "test_handler".into(),
            params: vec![Param {
                name: "req".into(),
                default_value: None,
                type_annotation: None,
                span: dummy_span(),
            }],
            body: vec![],
            closure: std::sync::Arc::new(std::collections::HashMap::new()),
            return_type: None,
            module_type_defs: None,
            module_enum_defs: None,
        }),
    );

    let args = vec![
        Expr::IntLit(0, dummy_span()),
        Expr::Ident("test_handler".into(), dummy_span()),
        Expr::IntLit(1, dummy_span()),
        Expr::IntLit(100, dummy_span()),
        Expr::IntLit(1, dummy_span()),
        Expr::BuchiPack(
            vec![
                BuchiField {
                    name: "cert".into(),
                    value: Expr::StringLit("cert.pem".into(), dummy_span()),
                    span: dummy_span(),
                },
                BuchiField {
                    name: "key".into(),
                    value: Expr::StringLit("key.pem".into(), dummy_span()),
                    span: dummy_span(),
                },
            ],
            dummy_span(),
        ),
    ];

    let result = interp.try_net_func("httpServe", &args);
    // Should succeed (return a Signal::Value with Async), but with a TlsError result.
    assert!(result.is_ok());
    let signal = result.unwrap();
    assert!(signal.is_some());
    match signal.unwrap() {
        Signal::Value(Value::Async(async_val)) => {
            // Should be a fulfilled Async with a Result failure.
            assert_eq!(async_val.status, AsyncStatus::Fulfilled);
            // The inner value should be a Result with TlsError.
            match &*async_val.value {
                Value::BuchiPack(fields) => {
                    let kind = fields
                        .iter()
                        .find(|(k, _)| k == "__value")
                        .and_then(|(_, v)| {
                            if let Value::BuchiPack(inner) = v {
                                inner
                                    .iter()
                                    .find(|(k, _)| k == "kind")
                                    .map(|(_, v)| v.clone())
                            } else {
                                None
                            }
                        });
                    assert_eq!(
                        kind,
                        Some(Value::str("TlsError".into())),
                        "Should have TlsError kind"
                    );
                }
                other => {
                    panic!("Expected BuchiPack (Result), got: {:?}", other);
                }
            }
        }
        other => {
            panic!("Expected Async value, got: {:?}", other);
        }
    }
}

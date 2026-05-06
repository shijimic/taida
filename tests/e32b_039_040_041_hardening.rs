//! E32B-039 / E32B-040 / E32B-041 — net hardening regressions.
//!
//! These tests pin static facts in the runtime sources so that someone
//! removing the overflow guards, the connection-abort helper, or the
//! grammar checks would have to update this file too — making the
//! regression visible in code review.

const NATIVE_NET: &str = include_str!("../src/codegen/native_runtime/net_h1_h2.c");
const INTERP_TYPES: &str = include_str!("../src/interpreter/net_eval/types.rs");
const INTERP_HELPERS: &str = include_str!("../src/interpreter/net_eval/helpers.rs");
const JS_NET: &str = include_str!("../src/js/runtime/net.rs");

// ── E32B-039 ────────────────────────────────────────────────────

#[test]
fn e32b_039_native_chunked_uses_builtin_overflow() {
    // The two helpers (taida_net_chunked_body_complete + the in-place
    // compactor) must use checked uint64_t arithmetic, not raw `* 16 + d`
    // on size_t — otherwise LP32 builds wrap above 8 hex digits.
    assert!(
        NATIVE_NET.contains("__builtin_mul_overflow(chunk_size_u64, (uint64_t)16, &mul)"),
        "native chunked parser must use __builtin_mul_overflow for chunk-size accumulation"
    );
    assert!(
        NATIVE_NET.contains("__builtin_add_overflow(mul, (uint64_t)digit, &add)"),
        "native chunked parser must use __builtin_add_overflow for chunk-size accumulation"
    );
    assert!(
        NATIVE_NET.contains("if (chunk_size_u64 > (uint64_t)SIZE_MAX) return -2;"),
        "native chunked parser must bound uint64_t accumulator to SIZE_MAX (body_complete)"
    );
    assert!(
        NATIVE_NET.contains("if (chunk_size_u64 > (uint64_t)SIZE_MAX) return -1;"),
        "native chunked parser must bound uint64_t accumulator to SIZE_MAX (in_place_compact)"
    );
}

#[test]
fn e32b_039_native_streaming_chunk_uses_strtoull_with_errno() {
    // The streaming readBodyChunk / readBodyAll path must use strtoull +
    // ERANGE detection, not strtoul. strtoul on LP32 is unsigned long ==
    // 32-bit and silently wraps to ULONG_MAX without errno being checked.
    let strtoul_count = NATIVE_NET.matches("strtoul(hex_buf").count();
    assert_eq!(
        strtoul_count, 0,
        "native streaming chunked path must not use strtoul (LP32 wraps silently)"
    );
    assert!(
        NATIVE_NET.contains("strtoull(hex_buf, &parse_end, 16)"),
        "native streaming chunked path must use strtoull"
    );
    assert!(
        NATIVE_NET.contains("errno == ERANGE"),
        "native streaming chunked path must check errno for ERANGE"
    );
    assert!(
        NATIVE_NET.contains("chunk_size_ull > (unsigned long long)SIZE_MAX"),
        "native streaming chunked path must bound chunk_size to SIZE_MAX"
    );
}

#[test]
fn e32b_039_interpreter_chunked_already_uses_checked_math() {
    // The interpreter is the reference implementation; this just
    // documents the invariant that backends must match.
    assert!(
        INTERP_HELPERS.contains("checked_mul") && INTERP_HELPERS.contains("checked_add"),
        "interpreter chunk-size accumulator must use checked_mul / checked_add"
    );
}

// ── E32B-040 ────────────────────────────────────────────────────

#[test]
fn e32b_040_native_has_connection_abort_helper() {
    assert!(
        NATIVE_NET.contains("static void taida_net4_abort_connection(const char *reason)"),
        "native runtime must define taida_net4_abort_connection"
    );
    assert!(
        NATIVE_NET.contains("shutdown(fd, SHUT_RDWR);"),
        "abort helper must shutdown the socket so further reads/writes fail fast"
    );
    // Net4BodyState carries the abort flag the accept loop reads.
    assert!(
        NATIVE_NET.contains("int aborted;"),
        "Net4BodyState must carry an aborted flag"
    );
    assert!(
        NATIVE_NET.contains("if (body_state.aborted) {"),
        "httpServe accept loop must drop keep-alive when the body state is aborted"
    );
}

fn slice_between<'a>(haystack: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
    let start = haystack
        .find(start_marker)
        .unwrap_or_else(|| panic!("missing start marker {:?}", start_marker));
    let after = &haystack[start..];
    let end = after
        .find(end_marker)
        .unwrap_or_else(|| panic!("missing end marker {:?}", end_marker));
    &after[..end]
}

#[test]
fn e32b_040_ws_receive_does_not_exit_on_attacker_input() {
    // wsReceive starts at its own banner and ends at the wsClose banner.
    let ws_receive = slice_between(
        NATIVE_NET,
        "// ── wsReceive(ws) → Lax[@(type, data)] (NET4-4d) ────────────",
        "// ── wsClose(ws, code) → Unit (NET4-4d, v5 revision) ────────────────",
    );

    // The function may keep `exit(1)` for programmer-error guards
    // (validate_ws_token, writer state) but must NOT exit(1) on any
    // *frame data* path — only the abort helper is acceptable.
    let exit_count = ws_receive.matches("exit(1)").count();
    assert!(
        exit_count <= 3,
        "wsReceive should only retain at most 3 programmer-error exits (state checks); found {}",
        exit_count
    );
    let abort_count = ws_receive.matches("taida_net4_abort_connection").count();
    assert!(
        abort_count >= 5,
        "wsReceive must use taida_net4_abort_connection for: invalid UTF-8 text frame, malformed close payload, invalid close code, invalid close reason UTF-8, frame protocol error — found {}",
        abort_count
    );
}

#[test]
fn e32b_040_chunked_body_does_not_exit_on_attacker_input() {
    let chunk = slice_between(
        NATIVE_NET,
        "// ── readBodyChunk(req) → Lax[Bytes] ─────────────────────────",
        "// ── readBodyAll(req) → Bytes ─────────────────────────────────",
    );
    let all = slice_between(
        NATIVE_NET,
        "// ── readBodyAll(req) → Bytes ─────────────────────────────────",
        "// ── WebSocket frame write (NET4-4c) ─────────────────────────",
    );

    // 4 programmer-error exits are tolerated in each per the API misuse
    // guards (arity, body-state, token, WS state). Anything more would
    // mean a wire-data path is still calling exit(1).
    let chunk_exits = chunk.matches("exit(1)").count();
    let all_exits = all.matches("exit(1)").count();
    assert!(
        chunk_exits <= 4,
        "readBodyChunk must only retain programmer-error exits, found {}",
        chunk_exits
    );
    assert!(
        all_exits <= 4,
        "readBodyAll must only retain programmer-error exits, found {}",
        all_exits
    );

    // And the chunked / Content-Length wire paths must abort the
    // connection rather than the process when they hit malformed input.
    assert!(
        chunk.contains("readBodyChunk: chunk-size overflow")
            && chunk.contains("readBodyChunk: invalid hex digit in chunk-size")
            && chunk.contains("readBodyChunk: truncated Content-Length body"),
        "readBodyChunk wire-data path must funnel malformed input through abort_connection"
    );
    assert!(
        all.contains("readBodyAll: chunk-size overflow")
            && all.contains("readBodyAll: invalid hex digit in chunk-size")
            && all.contains("readBodyAll: truncated Content-Length body"),
        "readBodyAll wire-data path must funnel malformed input through abort_connection"
    );
}

// ── E32B-041 ────────────────────────────────────────────────────

#[test]
fn e32b_041_interpreter_validator_carries_grammar_helpers() {
    assert!(
        INTERP_TYPES.contains("pub(crate) fn is_rfc7230_token_byte(b: u8) -> bool"),
        "interpreter must export the RFC 7230 token grammar helper"
    );
    assert!(
        INTERP_TYPES.contains("pub(crate) fn is_rfc7230_field_value_byte(b: u8) -> bool"),
        "interpreter must export the RFC 7230 field-value grammar helper"
    );
}

#[test]
fn e32b_041_eager_path_shares_grammar_with_streaming() {
    // The eager path (httpEncodeResponse) must call into the same
    // grammar helpers as the streaming path; otherwise the 7 attacker
    // bypass cases fall back to the old CR/LF-only check.
    assert!(
        INTERP_HELPERS.contains("is_rfc7230_token_byte")
            && INTERP_HELPERS.contains("is_rfc7230_field_value_byte"),
        "interpreter eager path (httpEncodeResponse) must share grammar with streaming"
    );
    assert!(
        NATIVE_NET.contains("static int taida_net3_is_rfc7230_token_byte(unsigned char b);")
            || NATIVE_NET.contains("static int taida_net3_is_rfc7230_token_byte(unsigned char b)"),
        "native must declare token grammar helper before httpEncodeResponse"
    );
    assert!(
        JS_NET.contains("__taida_net_isRfc7230TokenByte")
            && JS_NET.contains("__taida_net_isRfc7230FieldValueByte"),
        "JS must define grammar helpers reused by both validators"
    );
}

#[test]
fn e32b_039_native_chunked_data_length_check_does_not_wrap() {
    // The naive `rp + chunk_size + 2 > data_len` wraps on LP32 once
    // `chunk_size` approaches SIZE_MAX, even after the upstream uint64_t
    // guard. Native must use the difference form so the comparison stays
    // monotonic.
    assert!(
        NATIVE_NET.contains("if (chunk_size > data_len - rp) return -1;"),
        "native chunked parser must use difference-form length check"
    );
    assert!(
        NATIVE_NET.contains("if (data_len - after_data < 2) return -1;"),
        "native chunked parser must check trailing CRLF without wrapping"
    );
}

#[test]
fn e32b_040_streaming_writers_use_abort_connection() {
    // Peer disconnect (RST / EPIPE) is attacker-reachable. The streaming
    // commit / send paths must funnel write failures through
    // taida_net4_abort_connection rather than exit(1).
    assert!(
        NATIVE_NET.contains(
            "taida_net4_abort_connection(\"writeChunk: failed to commit response head\")"
        ) && NATIVE_NET
            .contains("taida_net4_abort_connection(\"writeChunk: failed to send chunk data\")"),
        "writeChunk wire-error paths must call taida_net4_abort_connection"
    );
    assert!(
        NATIVE_NET.contains(
            "taida_net4_abort_connection(\"endResponse: failed to commit response head\")"
        ),
        "endResponse wire-error path must call taida_net4_abort_connection"
    );
    assert!(
        NATIVE_NET
            .contains("taida_net4_abort_connection(\"sseEvent: failed to commit response head\")")
            && NATIVE_NET.contains(
                "taida_net4_abort_connection(\"sseEvent: failed to send SSE chunk data\")"
            ),
        "sseEvent wire-error paths must call taida_net4_abort_connection"
    );
}

#[test]
fn e32b_041_eager_path_rejects_set_cookie() {
    // Set-Cookie reservation must be enforced in the eager path
    // (httpEncodeResponse) across all three backends — not just the
    // streaming validator.
    assert!(
        INTERP_HELPERS.contains("'Set-Cookie' is reserved by the runtime"),
        "interpreter eager path must reject Set-Cookie in httpEncodeResponse"
    );
    assert!(
        NATIVE_NET.contains("'Set-Cookie' is reserved by the runtime"),
        "native eager path must reject Set-Cookie in httpEncodeResponse"
    );
    assert!(
        JS_NET.contains("'Set-Cookie' is reserved by the runtime"),
        "JS eager path must reject Set-Cookie in httpEncodeResponse"
    );
}

#[test]
fn e32b_041_scatter_path_uses_grammar_helpers() {
    // The httpServe handler-return scatter path (which does not flow
    // through httpEncodeResponse) must enforce the same RFC 7230 grammar
    // and reservations. Otherwise an attacker-influenced header from the
    // handler bypasses the validator on the production wire.
    let scatter = NATIVE_NET
        .split("static int taida_net_send_response_scatter")
        .nth(1)
        .expect("scatter function must exist");
    let scatter_end = scatter
        .find("\n}\n")
        .expect("scatter function must terminate");
    let scatter_body = &scatter[..scatter_end];
    assert!(
        scatter_body.contains("taida_net3_is_rfc7230_token_byte"),
        "native scatter path must call the RFC 7230 token grammar helper"
    );
    assert!(
        scatter_body.contains("taida_net3_is_rfc7230_field_value_byte"),
        "native scatter path must call the RFC 7230 field-value grammar helper"
    );
    assert!(
        scatter_body.contains("\"set-cookie\""),
        "native scatter path must reserve set-cookie"
    );
    assert!(
        scatter_body.contains("\"transfer-encoding\""),
        "native scatter path must reserve transfer-encoding"
    );

    // JS scatter shares helpers with the eager validator.
    let js_scatter = JS_NET
        .split("function __taida_net_encodeResponseScatter")
        .nth(1)
        .expect("JS scatter must exist");
    let js_scatter_end = js_scatter.find("\n}\n").expect("JS scatter must terminate");
    let js_scatter_body = &js_scatter[..js_scatter_end];
    assert!(
        js_scatter_body.contains("__taida_net_isRfc7230TokenByte"),
        "JS scatter path must call the RFC 7230 token helper"
    );
    assert!(
        js_scatter_body.contains("__taida_net_isRfc7230FieldValueByte"),
        "JS scatter path must call the RFC 7230 field-value helper"
    );
    assert!(
        js_scatter_body.contains("'set-cookie'") || js_scatter_body.contains("\"set-cookie\""),
        "JS scatter path must reserve set-cookie"
    );
    assert!(
        js_scatter_body.contains("'transfer-encoding'")
            || js_scatter_body.contains("\"transfer-encoding\""),
        "JS scatter path must reserve transfer-encoding"
    );
}

#[test]
fn e32b_041_seven_bypass_cases_pinned() {
    // Each of the seven cases the reviewer demonstrated must show up
    // in the validator messages so a unit test can assert against them.
    let cases = [
        // (1) ':' in name → token grammar
        (INTERP_TYPES, "RFC 7230 token grammar"),
        // (2) NUL in name → token grammar (NUL is not a token byte)
        (INTERP_TYPES, "RFC 7230 token grammar"),
        // (3) space/tab in name → token grammar
        (INTERP_TYPES, "RFC 7230 token grammar"),
        // (4) tab/control bytes in value → field-value grammar
        (INTERP_TYPES, "RFC 7230 field-value grammar"),
        // (5) control bytes in value → field-value grammar
        (INTERP_TYPES, "RFC 7230 field-value grammar"),
        // (6) underscore in name (CL.CL bypass)
        (
            INTERP_TYPES,
            "'_' which reverse proxies normalise inconsistently",
        ),
        // (7) Set-Cookie reserved
        (INTERP_TYPES, "'Set-Cookie' is reserved by the runtime"),
    ];
    for (haystack, needle) in cases {
        assert!(
            haystack.contains(needle),
            "validator must mention {:?} so the regression test can assert against it",
            needle
        );
    }
}

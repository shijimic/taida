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

// ── E32B-079 (E32B-040 follow-up) ───────────────────────────────

#[test]
fn e32b_079_native_runtime_has_no_remaining_handler_exit() {
    // After the supply-chain follow-up, every handler-callable API in
    // net_h1_h2.c routes attacker-reachable failures through
    // taida_net4_abort_connection. The only `exit(1)` tokens that are
    // allowed to remain in the source are the explanatory references
    // inside doc-comments. Strip C `//` line comments first so a
    // future "exit(1)" rewritten as `exit (1);` (whitespace-injected)
    // cannot slip through, and so doc-comment mentions don't trip the
    // assertion.
    let mut code = String::with_capacity(NATIVE_NET.len());
    for line in NATIVE_NET.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        code.push_str(line);
        code.push('\n');
    }
    let mut chars = code.chars().peekable();
    let mut tokens: Vec<char> = Vec::new();
    while let Some(c) = chars.next() {
        // Strip /* ... */ block comments too.
        if c == '/' && matches!(chars.peek(), Some(&'*')) {
            chars.next(); // consume '*'
            let mut prev = ' ';
            while let Some(cc) = chars.next() {
                if prev == '*' && cc == '/' {
                    break;
                }
                prev = cc;
            }
            continue;
        }
        tokens.push(c);
    }
    let stripped: String = tokens.into_iter().collect();
    let collapsed = stripped.replace([' ', '\t', '\n', '\r'], "");
    let exit_count = collapsed.matches("exit(1);").count();
    assert_eq!(
        exit_count, 0,
        "net_h1_h2.c must not call exit(1) on any handler-context path; \
         every attacker-reachable failure must funnel through \
         taida_net4_abort_connection"
    );
}

#[test]
fn e32b_079_validate_writer_returns_int_not_void() {
    assert!(
        NATIVE_NET.contains(
            "static int taida_net3_validate_writer(taida_val writer, const char *api_name)"
        ),
        "validate_writer must return int so callers can check 0/-1 instead of relying on exit(1)"
    );
    assert!(
        NATIVE_NET.contains("if (taida_net3_validate_writer(writer, \"startResponse\") < 0)"),
        "startResponse must early-return on validate_writer failure"
    );
    assert!(
        NATIVE_NET.contains("if (taida_net3_validate_writer(writer, \"writeChunk\") < 0)"),
        "writeChunk must early-return on validate_writer failure"
    );
    assert!(
        NATIVE_NET.contains("if (taida_net3_validate_writer(writer, \"endResponse\") < 0)"),
        "endResponse must early-return on validate_writer failure"
    );
    assert!(
        NATIVE_NET.contains("if (taida_net3_validate_writer(writer, \"sseEvent\") < 0)"),
        "sseEvent must early-return on validate_writer failure"
    );
    assert!(
        NATIVE_NET.contains("if (taida_net3_validate_writer(writer, \"wsUpgrade\") < 0)"),
        "wsUpgrade must early-return on validate_writer failure"
    );
}

#[test]
fn e32b_079_handler_apis_have_post_abort_noop_guard() {
    // After abort_connection runs, the handler keeps executing on Lax
    // sentinels until it returns naturally. Each handler-callable API
    // must short-circuit at its head when bs->aborted is set, so a
    // post-abort writeChunk / wsSend / sseEvent / readBodyChunk does
    // not retrigger I/O on the dead socket.
    let apis = [
        (
            "taida_net_start_response",
            "if (tl_net4_body && tl_net4_body->aborted) return 0;",
        ),
        (
            "taida_net_write_chunk",
            "if (tl_net4_body && tl_net4_body->aborted) return 0;",
        ),
        (
            "taida_net_end_response",
            "if (tl_net4_body && tl_net4_body->aborted) return 0;",
        ),
        (
            "taida_net_sse_event",
            "if (tl_net4_body && tl_net4_body->aborted) return 0;",
        ),
        (
            "taida_net_read_body_chunk",
            "return taida_net4_make_lax_bytes_empty();",
        ),
        (
            "taida_net_read_body_all",
            "return taida_bytes_new_filled(0, 0);",
        ),
        (
            "taida_net_ws_upgrade",
            "return taida_net4_make_lax_ws_empty();",
        ),
        (
            "taida_net_ws_send",
            "if (tl_net4_body && tl_net4_body->aborted) return 0;",
        ),
        (
            "taida_net_ws_receive",
            "return taida_net4_make_lax_ws_frame_empty();",
        ),
        (
            "taida_net_ws_close",
            "if (tl_net4_body && tl_net4_body->aborted) return 0;",
        ),
        (
            "taida_net_ws_close_code",
            "if (tl_net4_body && tl_net4_body->aborted) return 0;",
        ),
    ];
    for (api, sentinel) in apis {
        // Find the function *body* (signature line ending with `) {`),
        // not the forward declaration (signature line ending with `);`).
        // Walk every line, track when we cross a line that starts with
        // `taida_val <api>(` AND ends with `) {`.
        let signature_prefix = format!("taida_val {}(", api);
        let mut head_offset = None;
        let mut byte_pos = 0usize;
        for line in NATIVE_NET.lines() {
            if line.starts_with(&signature_prefix) && line.trim_end().ends_with(") {") {
                head_offset = Some(byte_pos + line.len() + 1); // +1 for '\n'
                break;
            }
            byte_pos += line.len() + 1; // +1 for '\n'
        }
        let head_offset = head_offset.unwrap_or_else(|| {
            panic!(
                "API {} body (signature line ending with `) {{`) must be defined",
                api
            )
        });
        let head_window_end = std::cmp::min(head_offset + 600, NATIVE_NET.len());
        let head_window = &NATIVE_NET[head_offset..head_window_end];
        assert!(
            head_window.contains("tl_net4_body && tl_net4_body->aborted"),
            "{} must guard its body with a tl_net4_body->aborted check at the head",
            api
        );
        assert!(
            head_window.contains(sentinel),
            "{} must return its post-abort sentinel `{}`",
            api,
            sentinel
        );
    }
}

#[test]
fn e32b_079_end_response_and_ws_upgrade_check_send_all_return() {
    // Codex follow-up: the chunked terminator in endResponse and the
    // 101 Switching Protocols response in wsUpgrade used to discard
    // the taida_net_send_all return value, leaving a peer-disconnect
    // path silently fail. Both now check the return and abort the
    // connection on a write failure.
    assert!(
        NATIVE_NET.contains("if (taida_net_send_all(fd, \"0\\r\\n\\r\\n\", 5) != 0) {"),
        "endResponse must check taida_net_send_all for the chunked terminator"
    );
    assert!(
        NATIVE_NET.contains(
            "taida_net4_abort_connection(\"endResponse: failed to send chunked terminator\")"
        ),
        "endResponse must abort on a chunked-terminator write failure"
    );
    assert!(
        NATIVE_NET.contains("if (taida_net_send_all(fd, response, (size_t)rlen) != 0) {"),
        "wsUpgrade must check taida_net_send_all for the 101 response"
    );
    assert!(
        NATIVE_NET.contains(
            "taida_net4_abort_connection(\"wsUpgrade: failed to send 101 Switching Protocols response\")"
        ),
        "wsUpgrade must abort on a 101-response write failure"
    );
    assert!(
        NATIVE_NET.contains("int pong_rc = taida_net4_write_ws_frame(fd, WS_OPCODE_PONG,"),
        "wsReceive auto-pong path must capture the write return value"
    );
    assert!(
        NATIVE_NET
            .contains("taida_net4_abort_connection(\"wsReceive: failed to send auto-pong frame\")"),
        "wsReceive must abort on auto-pong write failure"
    );
}

#[test]
fn e32b_079_ws_send_checks_write_frame_return() {
    // wsSend used to drop the taida_net4_write_ws_frame return value,
    // letting peer disconnect (RST / EPIPE) silently fail. The fix
    // checks `!= 0` and aborts the connection so the listener pool
    // keeps serving siblings.
    let ws_send = slice_between(
        NATIVE_NET,
        "// ── wsSend(ws, data) → Unit (NET4-4d) ───────────────────────",
        "// ── wsReceive(ws) → Lax[@(type, data)] (NET4-4d) ────────────",
    );
    assert!(
        ws_send.contains("taida_net4_write_ws_frame(fd, opcode, payload, payload_len) != 0"),
        "wsSend must check the WS frame write return value and abort on failure"
    );
    assert!(
        ws_send.contains("taida_net4_abort_connection(\"wsSend: failed to send WebSocket frame\")"),
        "wsSend must call abort_connection on a write failure"
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
fn e32b_041_native_eager_no_double_content_length() {
    // The Native eager httpEncodeResponse must coalesce a handler-supplied
    // Content-Length with its own auto-append, otherwise the response
    // emits two Content-Length lines and re-introduces CL.CL smuggling.
    // The control flow we depend on is: detect "content-length", drop it
    // for bodyless statuses (`continue`), and otherwise set
    // `has_content_length = 1` so the auto-append below is suppressed.
    // Skip the forward declaration (line ~7) and grab the function body
    // proper, which starts at the `{` after the parameter list.
    let eager = NATIVE_NET
        .split("taida_val taida_net_http_encode_response(taida_val response) {")
        .nth(1)
        .expect("Native httpEncodeResponse function body must exist");
    let eager_end = eager
        .find("// ── net_send_all")
        .or_else(|| eager.find("// ── readBody"))
        .or_else(|| eager.find("static int taida_net_send_response_scatter"))
        .expect("Native httpEncodeResponse must terminate before the next section");
    let eager_body = &eager[..eager_end];

    assert!(
        eager_body.contains(r#"taida_net3_header_name_eq_ci(hname_s, hn_len, "content-length")"#),
        "native eager path must detect content-length in handler headers"
    );
    assert!(
        eager_body.contains("has_content_length = 1"),
        "native eager path must mark has_content_length so auto-append is suppressed"
    );
    // Bodyless statuses (204 / 304 / 1xx) must drop the user CL.
    assert!(
        eager_body.contains("if (no_body) continue;"),
        "native eager path must skip user content-length for no-body statuses"
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

// ── E32B-082 (static-string NUL preservation) ─────────────────────────

#[test]
fn e32b_082_native_runtime_uses_heap_header_byte_length_for_body() {
    // E32B-082 follow-up (Codex REJECT): the eager `httpEncodeResponse`
    // and the `httpServe` scatter path used to size Str bodies via
    // `taida_read_cstr_len_safe` (a NUL scan), so an embedded-NUL body
    // would silently truncate before reaching the wire. The fix routes
    // both paths through `taida_str_byte_len`, which prefers the
    // heap-style header length and falls back to NUL scan only for
    // raw C-string callers (FFI). This test pins the source-level
    // invariant so a future revert is caught by the regression suite.

    // Eager path callsite (within the `body` Str branch).
    assert!(
        NATIVE_NET.contains("if (taida_str_byte_len((const char*)body_ptr, &slen)) {")
            && NATIVE_NET.contains("\"httpEncodeResponse: body exceeds 10485760 bytes (got %zu)\""),
        "eager httpEncodeResponse Str-body sizer must use taida_str_byte_len + post-bound check"
    );

    // Scatter path callsite (httpServe).
    let scatter_body = NATIVE_NET
        .split("static int taida_net_send_response_scatter")
        .nth(1)
        .expect("scatter function must exist");
    let scatter_body_end = scatter_body
        .find("\n}\n")
        .expect("scatter function must terminate");
    let scatter_body = &scatter_body[..scatter_body_end];
    assert!(
        scatter_body.contains("if (taida_str_byte_len((const char*)body_ptr, &slen)) {"),
        "scatter Str-body sizer must use taida_str_byte_len"
    );
    let raw_cstr_in_scatter_body = scatter_body
        .matches("taida_read_cstr_len_safe((const char*)body_ptr")
        .count();
    assert_eq!(
        raw_cstr_in_scatter_body, 0,
        "scatter Str-body sizer must not call taida_read_cstr_len_safe (it truncates at NUL)"
    );
}

#[test]
fn e32b_082_native_runtime_str_byte_length_uses_heap_header() {
    // `taida_str_byte_length` (the public C entrypoint exported via
    // header) and `taida_str_byte_slice` (used by the polymorphic
    // slice mold) must reach for the heap-header byte length first
    // so embedded-NUL static strings are not truncated.
    let core: &str = include_str!("../src/codegen/native_runtime/core.c");
    let length_body = core
        .split("taida_val taida_str_byte_length(const char* s) {")
        .nth(1)
        .expect("taida_str_byte_length body must exist");
    let length_body_end = length_body
        .find("\n}")
        .expect("taida_str_byte_length must terminate");
    let length_body = &length_body[..length_body_end];
    assert!(
        length_body.contains("taida_str_byte_len(s, &out_len)"),
        "taida_str_byte_length must prefer the heap-header byte length"
    );

    let slice_body = core
        .split("taida_val taida_str_byte_slice(const char* s, taida_val start, taida_val end) {")
        .nth(1)
        .expect("taida_str_byte_slice body must exist");
    let slice_body_end = slice_body
        .find("\n}")
        .expect("taida_str_byte_slice must terminate");
    let slice_body = &slice_body[..slice_body_end];
    assert!(
        slice_body.contains("taida_str_byte_len(s, &byte_len)"),
        "taida_str_byte_slice must prefer the heap-header byte length"
    );
}

// ── E32B-041 ────────────────────────────────────────────────────

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

// ── E32B-085 ────────────────────────────────────────────────────

#[test]
fn e32b_085_native_runtime_checks_every_ws_frame_write_return() {
    // Whole-file invariant: every `taida_net4_write_ws_frame(...)` call
    // outside the function definition itself must capture its return
    // value. The forms in use are
    //   - `if (taida_net4_write_ws_frame(...) != 0) { ... }`  (wsSend / wsClose)
    //   - `int wrc = taida_net4_write_ws_frame(...);`         (close replies)
    //   - `wrc = taida_net4_write_ws_frame(...);`             (re-use of wrc)
    //   - `int pong_rc = taida_net4_write_ws_frame(...);`     (auto-pong)
    // Any new call site that drops the return becomes a peer-disconnect
    // hole that is invisible to the abort-connection path.
    let mut total_calls = 0usize;
    let mut unchecked_lines: Vec<String> = Vec::new();
    for line in NATIVE_NET.lines() {
        if !line.contains("taida_net4_write_ws_frame(") {
            continue;
        }
        // Skip the function definition.
        if line.contains("static int taida_net4_write_ws_frame(") {
            continue;
        }
        total_calls += 1;
        let captured = line.contains("if (taida_net4_write_ws_frame(")
            || line.contains("wrc = taida_net4_write_ws_frame(")
            || line.contains("pong_rc = taida_net4_write_ws_frame(");
        if !captured {
            unchecked_lines.push(line.trim().to_string());
        }
    }
    assert!(
        total_calls >= 10,
        "net_h1_h2.c must still emit at least 10 WS frame writes (wsSend, wsReceive 8 sites, wsClose, handler-return auto-close); got {}",
        total_calls
    );
    assert!(
        unchecked_lines.is_empty(),
        "every taida_net4_write_ws_frame call site must capture its return value; unchecked: {:?}",
        unchecked_lines
    );
}

#[test]
fn e32b_085_ws_receive_checks_every_write_frame_return() {
    // E32B-079 hardened wsSend / wsClose / endResponse / wsUpgrade so the
    // peer-disconnect-during-reply path no longer escapes diagnosis. The
    // wsReceive frame-handling switch was missed in that sweep: invalid
    // UTF-8 text replies, malformed-close replies, invalid-close-code
    // replies, invalid-reason replies, valid-close echoes, the empty
    // close reply, and the WS_FRAME_ERROR fallback all dropped the
    // taida_net4_write_ws_frame return, leaving "we couldn't actually
    // send the close frame" indistinguishable from "we did."
    //
    // This test pins that every `taida_net4_write_ws_frame(...)` inside
    // the wsReceive function captures the return value (either as
    // `int wrc = ...`, `wrc = ...`, or the pre-existing `int pong_rc`),
    // matching the E32B-079 wsSend pattern.
    let ws_receive = slice_between(
        NATIVE_NET,
        "// ── wsReceive(ws) → Lax[@(type, data)] (NET4-4d) ────────────",
        "// ── wsClose(ws, code) → Unit (NET4-4d, v5 revision) ────────────────",
    );

    let total_calls = ws_receive.matches("taida_net4_write_ws_frame(").count();
    // Every call site must be preceded by either `wrc =` or `pong_rc =`
    // (with optional `int` declaration earlier on the same token). We
    // count by lines so substring overlap between `int wrc =` and
    // `wrc =` does not double-count.
    let mut captured_calls = 0usize;
    let mut unchecked_lines: Vec<&str> = Vec::new();
    for line in ws_receive.lines() {
        if !line.contains("taida_net4_write_ws_frame(") {
            continue;
        }
        // Skip the function definition itself.
        if line.contains("static int taida_net4_write_ws_frame(") {
            continue;
        }
        let captured = line.contains("wrc = taida_net4_write_ws_frame(")
            || line.contains("pong_rc = taida_net4_write_ws_frame(");
        if captured {
            captured_calls += 1;
        } else {
            unchecked_lines.push(line.trim());
        }
    }

    assert!(
        total_calls >= 7,
        "wsReceive must still emit at least 7 close/auto-pong write sites; got {}",
        total_calls
    );
    assert!(
        unchecked_lines.is_empty(),
        "every taida_net4_write_ws_frame inside wsReceive must capture its return value (wrc / pong_rc); unchecked sites: {:?}",
        unchecked_lines
    );
    assert!(
        captured_calls >= 7,
        "wsReceive must capture at least 7 write returns; got {}",
        captured_calls
    );

    // Every captured wrc / pong_rc must feed an abort-on-failure path
    // (either a ternary in the abort message or an explicit
    // `if (wrc != 0) abort`).
    let abort_paths = ws_receive.matches("wrc != 0").count()
        + ws_receive.matches("pong_rc != 0").count();
    assert!(
        abort_paths >= captured_calls,
        "every wsReceive write must reach an abort_connection branch on failure; abort_paths={} captured_calls={}",
        abort_paths,
        captured_calls
    );
}

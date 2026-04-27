//! D29B-011 (Phase 5 / Track-ζ): h3 QPACK headers span shape parity.
//!
//! Symmetric to D29B-001 (`tests/d29b_001_h2_headers_span_parity.rs`).
//! Pre-fix Native h3 / Interpreter h3 built `req.headers[i].name`,
//! `req.headers[i].value`, `req.method`, `req.path`, `req.query` as
//! `Value::Str` instead of `@(start, len)` span packs into `req.raw`,
//! and `req.raw` itself was just the body buffer (head excluded). QPACK
//! has the same dynamic-table reallocation problem as HPACK, so the only
//! way to give Span* mold a stable backing buffer is to copy decoded
//! headers into a per-request arena alongside the body (Strategy V1-A in
//! `.dev/D29_SESSION_PLANS/Phase-5_2026-04-27-1100_track-zeta_sub-Lock.md`)
//! and surface every Str-shaped pseudo / regular header field as
//! `@(start, len)` span packs into that arena.
//!
//! This file is the structural regression guard for the h3 path. It pins
//! the arena shape inside both `src/interpreter/net_eval/h3.rs::serve_h3`
//! (interp h3) and
//! `src/codegen/native_runtime/net_h3_quic.c::h3_build_request_pack`
//! (native h3) by string-matching the arena builder pattern.
//!
//! Backend coverage:
//! - Interpreter h3: arena fast path
//! - Native h3: arena fast path with OOM-tolerant Str-pack fallback
//! - JS: h3 server not implemented (D29B-002 limited the `wasm-full`
//!   zero-copy claim to Bytes I/O surface, JS is similarly NET-h3
//!   scope-out)
//! - wasm-wasi: NET surface scope-out (POST-STABLE-002, FUTURE_BLOCKERS.md)
//!
//! `cargo test --release --test d29b_011_h3_headers_span_parity` is
//! cheap (no quinn / TLS / fork) and runs as a CI hard-fail gate.

fn read_interp_h3_source() -> String {
    std::fs::read_to_string("src/interpreter/net_eval/h3.rs")
        .expect("read src/interpreter/net_eval/h3.rs")
}

fn read_native_runtime_source() -> String {
    use std::path::PathBuf;
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest_dir.join("src/codegen/native_runtime");
    let fragments = ["core.c", "os.c", "tls.c", "net_h1_h2.c", "net_h3_quic.c"];
    let mut out = String::new();
    for name in fragments {
        let path = dir.join(name);
        let part = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
        out.push_str(&part);
    }
    out
}

#[test]
fn interpreter_h3_handler_uses_per_request_arena_with_span_headers() {
    let src = read_interp_h3_source();

    // D29B-011 banner: identifies the post-fix arena builder. A revert to
    // the legacy Str-pack form would drop this banner and flip the
    // assertion.
    assert!(
        src.contains("D29B-011 (Track-ζ Lock-H, 2026-04-27)"),
        "Interpreter h3 must keep the D29B-011 arena builder banner. \
         A revert would drop this comment along with the arena."
    );
    assert!(
        src.contains("let mut arena: Vec<u8> = Vec::with_capacity(arena_cap)"),
        "Interpreter h3 must allocate a per-request arena sized exactly \
         for body + method + path + query + headers (Strategy V1-A, \
         identical to h2)."
    );

    // body first (offset 0)
    assert!(
        src.contains("arena.extend_from_slice(&req.body);"),
        "Interpreter h3 arena layout must place body at offset 0 (so \
         `body = make_span(0, body_len)` and `bodyOffset = 0` keep \
         pointing at body)."
    );
    assert!(
        src.contains("arena.extend_from_slice(req.method.as_bytes());"),
        "Interpreter h3 arena layout must follow body with method bytes."
    );

    // Header (start, len) pairs captured for span pack list.
    assert!(
        src.contains("header_spans.push((n_start, n_len, v_start, v_len));"),
        "Interpreter h3 must capture (name_start, name_len, value_start, \
         value_len) for each header so the headers list can be rebuilt as \
         span packs into req.raw."
    );

    // Final headers list uses make_span (not Value::str)
    let headers_section: String = src
        .lines()
        .skip_while(|l| !l.contains("for (n_start, n_len, v_start, v_len) in &header_spans"))
        .take(8)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        headers_section.contains("make_span(*n_start, *n_len)")
            && headers_section.contains("make_span(*v_start, *v_len)"),
        "Interpreter h3 headers list must use make_span(...) for both \
         name and value (post-D29B-011 contract); pre-fix used \
         Value::str(name.clone()) / Value::str(value.clone()), which \
         broke SpanEquals[req.headers(0).name, req.raw, ...]() on h3."
    );

    // Pseudo headers (method/path/query) as span packs
    assert!(
        src.contains("(\"method\".into(), make_span(method_start, method_len))"),
        "Interpreter h3 `method` field must be a span pack matching \
         h1/h2 reference shape."
    );
    assert!(
        src.contains("(\"path\".into(), make_span(path_start, path_len))"),
        "Interpreter h3 `path` field must be a span pack."
    );
    assert!(
        src.contains("(\"query\".into(), make_span(query_start, query_len))"),
        "Interpreter h3 `query` field must be a span pack."
    );

    // raw = arena
    assert!(
        src.contains("(\"raw\".into(), Value::bytes(arena))"),
        "Interpreter h3 `raw` field must be Value::bytes(arena), giving \
         req.raw the contiguous body+headers buffer that span packs index \
         into."
    );
}

#[test]
fn native_h3_request_pack_uses_arena_with_span_headers() {
    let src = read_native_runtime_source();

    // h3_build_request_pack lives in fragment 5 (net_h3_quic.c). Slice a
    // generous window to pin multiple invariants without matching outside
    // the function.
    let h3_section: String = src
        .lines()
        .skip_while(|l| !l.contains("h3_build_request_pack"))
        .take(220)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        h3_section.contains("D29B-011 (Track-ζ Lock-H, 2026-04-27)"),
        "Native h3_build_request_pack must keep the D29B-011 arena banner."
    );

    // Arena allocation
    assert!(
        h3_section.contains("TAIDA_MALLOC(arena_size > 0 ? arena_size : 1, \"h3_arena\")"),
        "Native h3 must allocate a per-request `h3_arena` buffer sized \
         exactly for body + method + path + query + headers (Strategy \
         V1-A, symmetric to Native h2's `h2_arena`)."
    );

    // Body first, then method
    assert!(
        h3_section.contains("memcpy(arena, body, body_len)"),
        "Native h3 arena layout must place body at offset 0."
    );

    // taida_bytes_from_raw + free(arena) for materialization
    assert!(
        h3_section.contains("taida_bytes_from_raw(arena, (taida_val)arena_size)")
            && h3_section.contains("free(arena);"),
        "Native h3 must materialize req.raw via taida_bytes_from_raw(arena, \
         arena_size) and free the staging arena."
    );

    // header_starts indexed pin — name/value materialized via make_span
    assert!(
        h3_section.contains("(taida_val)header_starts[i][0], (taida_val)header_lens[i][0]")
            && h3_section.contains("(taida_val)header_starts[i][1], (taida_val)header_lens[i][1]"),
        "Native h3 headers list entries must use taida_net_make_span(...) \
         indexed by (header_starts[i][0], header_lens[i][0]) for name and \
         (header_starts[i][1], header_lens[i][1]) for value (post-D29B-011 \
         contract); pre-fix used taida_str_new_copy(...) / TAIDA_TAG_STR \
         which broke SpanEquals under h3."
    );

    // Pseudo headers (method/path/query) span packs on the arena fast path
    assert!(
        h3_section.contains("SET_FIELD_H3(\"method\", taida_net_make_span("),
        "Native h3 `method` field must be a span pack into req.raw on the \
         arena fast path."
    );
    assert!(
        h3_section.contains("SET_FIELD_H3(\"path\",   taida_net_make_span("),
        "Native h3 `path` field must be a span pack."
    );
    assert!(
        h3_section.contains("SET_FIELD_H3(\"query\",  taida_net_make_span("),
        "Native h3 `query` field must be a span pack."
    );

    // body field span (not Bytes ref)
    assert!(
        h3_section
            .contains("SET_FIELD_H3(\"body\",        taida_net_make_span(0, (taida_val)body_len)")
            || h3_section
                .contains("SET_FIELD_H3(\"body\", taida_net_make_span(0, (taida_val)body_len)"),
        "Native h3 `body` field must be a span pack `make_span(0, body_len)` \
         into req.raw at offset 0 — matches h1/h2 reference shape."
    );

    // OOM-tolerant fallback present
    assert!(
        h3_section.contains("OOM: degrade to legacy form"),
        "Native h3 must keep an OOM-tolerant fallback path that retains \
         the legacy Str-pack form when the staging arena allocation fails."
    );
}

#[test]
fn native_h3_request_pack_drops_double_retain_now_that_body_is_a_span() {
    // D29B-011: post-fix the `body` field is a span pack into req.raw,
    // not a second reference to `raw_bytes`. The previous
    // `taida_retain(raw_bytes)` covered the dual-field shape; with
    // `body` now a span the extra retain would leak. Pin the
    // explanatory comment we left so a future "well-meaning" leak fix
    // that re-adds the retain shows up in code review.
    let src = read_native_runtime_source();
    let h3_section: String = src
        .lines()
        .skip_while(|l| !l.contains("h3_build_request_pack"))
        .take(220)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        h3_section.contains("body field is now a span pack")
            || h3_section.contains("body field is a span pack"),
        "Native h3_build_request_pack must keep the explanatory comment \
         documenting why the legacy `taida_retain(raw_bytes)` was removed \
         once `body` became a span pack (post-D29B-011)."
    );
}

#[test]
fn h2_and_h3_request_pack_arenas_are_structurally_symmetric() {
    // Lock-H requires the two arena builders to be byte-similar so that
    // `SpanEquals[req.method, req.raw, "GET"]()` returns the same Bool
    // under h1/h2/h3. We pin two structural symmetries:
    //   1. Both interp h2 and h3 use the same Strategy V1-A banner
    //      ("D29B-001" / "D29B-011 (Track-ζ Lock-H").
    //   2. Both Native h2 and h3 use the same arena allocation idiom
    //      (TAIDA_MALLOC(arena_size > 0 ? arena_size : 1, ...)) with
    //      backend-specific labels ("h2_arena" / "h3_arena").
    // If a future track refactors only one of the four sites, this test
    // will catch the divergence before it lands.
    let h2_interp = std::fs::read_to_string("src/interpreter/net_eval/h2.rs")
        .expect("read src/interpreter/net_eval/h2.rs");
    let h3_interp = read_interp_h3_source();
    assert!(h2_interp.contains("Strategy V1-A"));
    assert!(h3_interp.contains("Strategy V1-A"));

    let native = read_native_runtime_source();
    assert!(native.contains("\"h2_arena\""));
    assert!(native.contains("\"h3_arena\""));
    assert!(
        native
            .matches("TAIDA_MALLOC(arena_size > 0 ? arena_size : 1")
            .count()
            >= 2,
        "Both Native h2 and Native h3 must use the same arena allocation \
         idiom (TAIDA_MALLOC(arena_size > 0 ? arena_size : 1, ...)) so a \
         single grep can audit both. Pre-D29B-001/011 there was no arena \
         at all; post-fix exactly two sites."
    );
}

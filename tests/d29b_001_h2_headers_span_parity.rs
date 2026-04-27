//! D29B-001 (Phase 5 / Track-ζ): h2 HPACK headers span shape parity.
//!
//! Pre-fix Native h2 / Interpreter h2 built `req.headers[i].name` and
//! `req.headers[i].value` as `Value::Str(Arc<StrValue>)` instead of as
//! `@(start: Int, len: Int)` span packs into `req.raw`. The HPACK dynamic
//! table's reallocation invariant means the decoded name / value bytes
//! cannot point into the wire buffer, so the only way to give Span* mold a
//! stable backing buffer is to copy decoded headers into a per-request
//! arena alongside the body (Strategy V1-A in
//! `.dev/D29_SESSION_PLANS/Phase-5_2026-04-27-1100_track-zeta_sub-Lock.md`)
//! and surface every Str-shaped pseudo / regular header field as
//! `@(start, len)` span packs into that arena. The arena becomes
//! `req.raw`.
//!
//! This file is the **structural** regression guard: it pins the arena
//! shape inside both `src/interpreter/net_eval/h2.rs::serve_h2` (interp
//! backend) and `src/codegen/native_runtime/net_h1_h2.c::h2_build_request_pack`
//! (native backend) by string-matching the exact arena builder pattern.
//! The wire-level parity is exercised in `tests/parity.rs` under the
//! `// D29B-001/011 region` marker (a Taida fixture that drives
//! `SpanEquals[req.headers(0).name, req.raw, "host"]()` on h1 and h2 and
//! asserts the same Bool — h1 reference shape vs post-fix h2 — and the
//! existing `test_net6_3b_native_h2_d28b002_*` suite covers the round-trip
//! handler / wire shape.
//!
//! `cargo test --release --test d29b_001_h2_headers_span_parity` is
//! cheap (no curl / TLS / fork) and runs as a CI hard-fail gate to catch
//! a future revert that would silently re-introduce the protocol-divergent
//! Str-pack form.

/// Read the full `src/interpreter/net_eval/h2.rs` source to verify the
/// arena builder lives where we expect.
fn read_interp_h2_source() -> String {
    std::fs::read_to_string("src/interpreter/net_eval/h2.rs")
        .expect("read src/interpreter/net_eval/h2.rs")
}

/// Read the assembled native runtime so we can string-search for the
/// arena fast path inside `h2_build_request_pack`. We read the fragments
/// from disk in the same C13-4 order as the in-crate `NATIVE_RUNTIME_C`
/// LazyLock concatenation, matching the convention used by `tests/parity.rs`
/// (so source-audit pin tests pick up edits without recompiling).
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
fn interpreter_h2_serves_pre_request_arena_with_span_headers() {
    let src = read_interp_h2_source();

    // The arena allocation is identifiable by the D29B-001 banner comment
    // plus the `Vec::with_capacity(arena_cap)` line that sizes the buffer
    // exactly for body + pseudo + headers. We pin both so a future revert
    // that drops the arena (going back to direct Str pack) flips the
    // assertion.
    assert!(
        src.contains("D29B-001 (Track-ζ Lock-H, 2026-04-27)"),
        "Interpreter h2 must keep the D29B-001 arena builder banner. \
         A revert would drop this comment along with the arena."
    );
    assert!(
        src.contains("let mut arena: Vec<u8> = Vec::with_capacity(arena_cap)"),
        "Interpreter h2 must allocate a per-request arena sized exactly \
         for body + method + path + query + headers (Strategy V1-A)."
    );

    // The arena layout is body first (so the existing body span and
    // bodyOffset = 0 invariants are preserved) followed by method.
    assert!(
        src.contains("arena.extend_from_slice(&body);")
            && src.contains("arena.extend_from_slice(method.as_bytes());"),
        "Interpreter h2 arena layout must place body at offset 0 followed \
         by the method bytes (so make_span(0, body_len) keeps pointing at \
         body)."
    );

    // Headers must be appended into the arena and their (start, len) pairs
    // captured for the span pack list.
    assert!(
        src.contains("header_spans.push((n_start, n_len, v_start, v_len));"),
        "Interpreter h2 must capture (name_start, name_len, value_start, \
         value_len) for each header so the headers list can be rebuilt as \
         span packs into req.raw."
    );

    // The final `headers` list must be span packs, not Str packs.
    let headers_section: String = src
        .lines()
        .skip_while(|l| !l.contains("for (n_start, n_len, v_start, v_len) in &header_spans"))
        .take(8)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        headers_section.contains("make_span(*n_start, *n_len)")
            && headers_section.contains("make_span(*v_start, *v_len)"),
        "Interpreter h2 headers list must use make_span(...) for both \
         name and value (post-D29B-001 contract); pre-fix used \
         Value::str(name.clone()) / Value::str(value.clone()), which \
         broke SpanEquals[req.headers(0).name, req.raw, ...]() on h2."
    );

    // method / path / query must be span packs too (pseudo-header parity
    // with h1).
    assert!(
        src.contains("(\"method\".into(), make_span(method_start, method_len))"),
        "Interpreter h2 `method` field must be a span pack matching h1 \
         reference shape."
    );
    assert!(
        src.contains("(\"path\".into(), make_span(path_start, path_len))"),
        "Interpreter h2 `path` field must be a span pack matching h1."
    );
    assert!(
        src.contains("(\"query\".into(), make_span(query_start, query_len))"),
        "Interpreter h2 `query` field must be a span pack matching h1."
    );

    // raw must be the assembled arena (Vec<u8> moved into Value::bytes).
    assert!(
        src.contains("(\"raw\".into(), Value::bytes(arena))"),
        "Interpreter h2 `raw` field must be Value::bytes(arena), giving \
         req.raw the contiguous body+headers buffer that span packs index \
         into."
    );
}

#[test]
fn native_h2_request_pack_uses_arena_with_span_headers() {
    let src = read_native_runtime_source();

    // The native arena builder lives inside `h2_build_request_pack`.
    // Take a generous slice so we can pin multiple invariants without
    // matching outside the function.
    let h2_section: String = src
        .lines()
        .skip_while(|l| !l.contains("h2_build_request_pack"))
        .take(220)
        .collect::<Vec<_>>()
        .join("\n");

    // D29B-001 banner: same as interpreter pin, makes a future revert
    // immediately visible.
    assert!(
        h2_section.contains("D29B-001 (Track-ζ Lock-H, 2026-04-27)"),
        "Native h2_build_request_pack must keep the D29B-001 arena banner."
    );

    // Arena allocation: `TAIDA_MALLOC(arena_size > 0 ? arena_size : 1, \"h2_arena\")`
    assert!(
        h2_section.contains("TAIDA_MALLOC(arena_size > 0 ? arena_size : 1, \"h2_arena\")"),
        "Native h2 must allocate a per-request `h2_arena` buffer sized \
         exactly for body + method + path + query + headers (Strategy V1-A)."
    );

    // Body first (offset 0)
    assert!(
        h2_section.contains("memcpy(arena, body, body_len)"),
        "Native h2 arena layout must place body at offset 0."
    );

    // D29B-015 (Track-β-2, 2026-04-27): producer flip. Native h2 now
    // materializes req.raw via taida_bytes_contig_new (CONTIG header +
    // inline payload, single allocation) so the writev hot path on the
    // h2 response side reflects the inline payload directly into iov[1]
    // — no taida_val[] 8x expansion. The staging arena is still freed
    // immediately because taida_bytes_contig_new memcpys the payload.
    assert!(
        h2_section.contains("taida_bytes_contig_new(arena, (taida_val)arena_size)")
            && h2_section.contains("free(arena);"),
        "Native h2 must materialize req.raw via taida_bytes_contig_new(arena, \
         arena_size) and free the staging arena (D29B-015 producer flip; the \
         CONTIG constructor copies into a single inline payload behind the \
         CONTIG header, so the staging arena can be freed immediately)."
    );

    // Headers list must use span packs (taida_net_make_span) into the
    // arena, with TAIDA_TAG_PACK tags. This is the heart of the fix.
    assert!(
        h2_section.contains("taida_net_make_span(\n                (taida_val)header_starts[i][0]")
            || h2_section
                .contains("taida_net_make_span(\n                (taida_val)header_starts[i][0]",)
            || h2_section.contains("(taida_val)header_starts[i][0], (taida_val)header_lens[i][0]"),
        "Native h2 headers list entries must use taida_net_make_span(...) \
         for both name and value (post-D29B-001 contract); pre-fix used \
         taida_str_new_copy(...) / TAIDA_TAG_STR which broke SpanEquals \
         under h2."
    );

    // Pseudo headers (method/path/query) must be span packs on the arena
    // fast path. The OOM fallback retains the legacy Str-pack form so the
    // server never crashes on a transient per-request OOM.
    assert!(
        h2_section.contains("SET_FIELD(\"method\", taida_net_make_span("),
        "Native h2 `method` field must be a span pack into req.raw on the \
         arena fast path."
    );
    assert!(
        h2_section.contains("SET_FIELD(\"path\",   taida_net_make_span("),
        "Native h2 `path` field must be a span pack into req.raw."
    );
    assert!(
        h2_section.contains("SET_FIELD(\"query\",  taida_net_make_span("),
        "Native h2 `query` field must be a span pack into req.raw."
    );

    // Body field is a span (not a Bytes ref)
    assert!(
        h2_section
            .contains("SET_FIELD(\"body\",        taida_net_make_span(0, (taida_val)body_len)")
            || h2_section
                .contains("SET_FIELD(\"body\", taida_net_make_span(0, (taida_val)body_len)"),
        "Native h2 `body` field must be a span pack `make_span(0, body_len)` \
         into req.raw at offset 0 — matches h1 reference shape."
    );

    // OOM fallback: legacy Str-pack form is retained for graceful
    // degradation. SpanEquals will silently miss but wire correctness is
    // preserved.
    assert!(
        h2_section.contains("// OOM: degrade to legacy form")
            || h2_section.contains("OOM-tolerant fallback"),
        "Native h2 must keep an OOM-tolerant fallback path that retains \
         the legacy Str-pack form when the staging arena allocation fails."
    );
}

#[test]
fn native_h2_request_pack_drops_double_retain_now_that_body_is_a_span() {
    // D29B-001: post-fix the `body` field of the h2 request pack is a
    // span pack `make_span(0, body_len)`, not a second reference to
    // `raw_bytes`. The previous NB6-27 invariant required
    // `taida_retain(raw_bytes)` because both `body` and `raw` held the
    // same Bytes ref; with `body` now a span the second retain would
    // leak. The pin lives inside the rewritten `h2_build_request_pack`
    // body — assert the comment block we left documenting why retain
    // was removed is present, so a future "well-meaning" leak fix that
    // re-adds the retain shows up in code review.
    let src = read_native_runtime_source();
    let h2_section: String = src
        .lines()
        .skip_while(|l| !l.contains("h2_build_request_pack"))
        .take(220)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        h2_section.contains("body field is now a span pack")
            || h2_section.contains("body field is a span pack"),
        "Native h2_build_request_pack must keep the explanatory comment \
         documenting why the legacy `taida_retain(raw_bytes)` was removed \
         once `body` became a span pack (post-D29B-001). Re-introducing \
         the retain would leak raw_bytes by one ref/request."
    );
}

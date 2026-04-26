// D29B-003 (Track-β, 2026-04-27): Native writev zero-copy regression
// fixture.
//
// Companion to `tests/d29b_003_native_writev_zero_copy.rs`. The Rust
// harness builds this fixture for the native backend, launches it as
// a subprocess with `TAIDA_NET_ANNOUNCE_PORT=1`, parses the announce
// line from stdout to discover the kernel-assigned port, and fires a
// single HTTP/1.1 request whose body it expects to see echoed back.
//
// Why an echo handler matters: the response.body field is built from
// the request body via `readBody`, which today emits the legacy
// TAIDA_BYTES_MAGIC representation (the contig-promotion of readBody
// is intentionally deferred — see the rationale in
// `src/codegen/native_runtime/net_h1_h2.c` near the
// `taida_net_read_body` / `taida_net_read_body_all` end-of-function
// blocks). The writev hot path in
// `taida_net_send_response_scatter` therefore sees a legacy Bytes
// body and falls through to the byte-loop materialize branch we kept
// alongside the new contig fast path. A wire-correct echo confirms
// that the legacy branch is intact even with the new
// TAIDA_BYTES_CONTIG / body_is_contig branches added in front of it.
//
// Phase 4 (Track-ε `Value::BytesView`) and the polymorphic-Bytes-
// dispatcher follow-up sub-Lock will allow us to flip readBody +
// readBodyAll over to taida_bytes_contig_new, at which point this
// fixture also exercises the contig fast path end-to-end. For Phase 3
// we only require the legacy branch to remain correct.

>>> taida-lang/net => @(httpServe, readBody)

handler req =
  // Echo the request body back as the response body. readBody returns
  // a Bytes value (legacy taida_val[] form today), which flows through
  // taida_net_send_response_scatter's body_is_bytes branch. The newly
  // added body_is_contig fast path is by-passed here because legacy
  // readBody output is not tagged TAIDA_BYTES_CONTIG; the byte-loop
  // fallback handles it.
  body <= readBody(req)
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "application/octet-stream")], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe(0, handler, 50)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)

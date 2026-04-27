// D29B-015 (Track-β-2 TIER 4, 2026-04-27): bench fixture for the
// `cargo bench --bench d29b_015_chunked_te_writev` 1MB body echo
// round-trip latency microbench.
//
// Identical handler to `d29b_003_writev_zero_copy/server.td`, but
// configured with a much higher max-requests cap so criterion's
// sample loop (warm-up + measurement) has enough headroom. The
// bench harness times one full request; criterion does ~100-200
// samples in a 5-10s measurement window.
//
// Why a separate fixture: bumping the d29b_003 fixture's max
// requests would change its behaviour for the dedicated regression
// test that uses it; a sibling fixture preserves both surfaces.

>>> taida-lang/net => @(httpServe, readBody)

handler req =
  // Echo the request body back as the response body. With D29B-015
  // landed, readBody emits a TAIDA_BYTES_CONTIG payload directly;
  // the writev hot path in taida_net_send_response_scatter then
  // reflects taida_bytes_contig_data() into iov[1].iov_base, eliding
  // the per-byte materialize loop that dominated 1MB body latency
  // pre-flip.
  body <= readBody(req)
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "application/octet-stream")], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe(0, handler, 1000000)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)

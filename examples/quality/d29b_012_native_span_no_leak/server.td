// D29B-012 (Track-η Phase 6, 2026-04-27) — Native HTTP/1 server fixture
// driving taida_net_SpanEquals / SpanStartsWith / SpanContains on req.raw
// for valgrind leak-0 verification.
//
// The handler exercises every Span* helper to ensure the Track-η resolver
// rewrite (taida_net_raw_as_bytes Option D, release-handle ABI) closes
// the per-request own_buf / own_n leak across all three call sites.
//
// httpServe limit = 4 requests so the smoke runs <2s under valgrind on
// CI 2C and the per-request reset codepath fires multiple times.
//
// Expected stdout: "4" (httpServe returns request count on clean drain).

>>> taida-lang/net => @(httpServe)

handler req =
  isGet     <= SpanEquals[req.method, req.raw, "GET"]()
  startsApi <= SpanStartsWith[req.path, req.raw, "/api"]()
  hasFoo    <= SpanContains[req.path, req.raw, "foo"]()
  marker    <= "g=" + isGet.toString() + ";s=" + startsApi.toString() + ";c=" + hasFoo.toString()
  @(status <= 200, headers <= @[], body <= marker)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

// Bind ephemeral port (0) so the test wrapper can scrape the assigned
// port via TAIDA_NET_ANNOUNCE_PORT=1. handler limit 4 keeps the smoke
// duration bounded under valgrind (10× slowdown) on CI 2C runners.
asyncResult <= httpServe(0, handler, 4, 5000, 4)
asyncResult ]=> result
result ]=> r
stdout(r.requests.toString())

// D28B-012 (Round 2 wF): NET runtime path leak regression fixture.
//
// Companion to `tests/d28b_012_net_runtime_leak.rs`. The Rust harness
// builds this fixture for the native backend, launches it as a
// subprocess, fires a fixed number of HTTP/1.1 requests against
// 127.0.0.1:18091 via /dev/tcp keep-alive loops, and samples the
// server's RSS / fd count from /proc/<pid>/status before vs after.
//
// The fixture produces a 512 B Str body per request to exercise the
// scatter-gather (writev) response path under the same shape as the
// `c26_soak_fixture` used by `scripts/soak/fast-soak-proxy.sh`.
//
// Acceptance: with the `taida_arena_request_reset` per-request
// boundary applied (D28B-012 fix), the worker thread's bump arena
// must rewind on every request so RSS plateaus near the cold-start
// value. Pre-fix the same workload drove RSS to ~340 MiB in 5 minutes
// (matching the 4 GB / 4.7 GiB/h drift recorded in
// `.dev/D28_BLOCKERS.md::D28B-012`).

>>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= Repeat["x", 512]())
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

// 50,000 request cap keeps the harness self-terminating; the harness
// itself stops sending after its sample window so we never reach the
// cap in the regression test.
asyncResult <= httpServe(18091, handler, 50000)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)

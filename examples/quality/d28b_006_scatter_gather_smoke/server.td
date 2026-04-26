// D28B-006 (Round 2 wI): scatter-gather smoke regression fixture.
//
// Companion to `tests/d28b_006_scatter_gather_smoke.rs`. The harness
// builds this fixture for the native backend, launches it as a
// subprocess on 127.0.0.1:18092, fires a small batch of HTTP/1.1
// requests (~1,000) over keep-alive TCP loops, and asserts that the
// scatter-gather (writev) response path produces well-formed 512 B
// responses without RSS / fd drift.
//
// Why a separate fixture from d28b_012?
//
//   * d28b_012 owns the *bump-arena rewind* invariant for the NET
//     runtime path (RSS bound at the request boundary). Its hard cap
//     (5 MiB / 1k req) matches that pin.
//   * d28b_006 owns the *scatter-gather is default send path*
//     invariant. It picks a different port (18092 vs 18091) so both
//     tests can run in parallel without bind contention, and uses a
//     smaller request budget (smoke regression, not RSS-cap pin).
//
// The fixture body is intentionally identical in shape to the soak
// fixture inlined by `scripts/soak/fast-soak-proxy.sh` (a 512 B body
// served by a single-arg handler) so the wF baseline (5,556 KiB
// plateau, 0.89 MiB/h drift recorded under `D28B-012` Acceptance) is
// directly comparable to anything this test catches.
//
// See `.dev/D28_BLOCKERS.md::D28B-006` and
// `.dev/D28_SOAK_RUNBOOK.md` for the full acceptance.

>>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= Repeat["x", 512]())
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

// 50,000 request cap keeps the harness self-terminating; the harness
// itself stops sending after its smoke window so we never reach the
// cap in the regression test.
asyncResult <= httpServe(18092, handler, 50000)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)

// 24-hour HTTP soak fixture for the long-running net runtime.
//
// Identical in shape to the 30-minute proxy fixture inlined by
// `scripts/soak/fast-soak-proxy.sh` and to
// `examples/quality/d28b_006_scatter_gather_smoke/server.td`, with
// two intentional differences:
//
//   * Port 18100 (vs 18092 for the smoke regression test, 18091 for
//     the bump-arena regression test) so all three can run in
//     parallel from three terminals or three CI shards without bind
//     contention.
//
//   * Request cap raised from 50,000 to 2,000,000,000 (effectively
//     unlimited for the duty cycle this fixture expects). A 24-hour
//     run at the documented steady-state rate (~1k-10k req/s) reaches
//     ~10^8 to 10^9 requests; the 50k cap used by the smoke fixtures
//     would trip well before the soak window closes and crash the
//     run. 2 G is the next round number the i32 request counter can
//     reach without overflow concerns under the documented load.
//
// Body shape is the same 512 B Repeat-fill the smoke fixture uses so
// the long-running net baseline (~5,556 KiB plateau, ~0.89 MiB/h
// proxy rate, 60-sample bit-identical post-warmup distribution) is
// directly comparable.

>>> taida-lang/net => @(httpServe)

handler req: Request =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= Repeat["x", 512]())
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe(18100, handler, 2000000000)
asyncResult >=> result
result >=> r
stdout(r.ok)
stdout(r.requests)

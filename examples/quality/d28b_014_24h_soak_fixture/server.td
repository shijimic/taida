// D28B-014 (Round 2 wI): 24h soak fixture for `.dev/D28_SOAK_RUNBOOK.md`.
//
// Identical in shape to the 30-min proxy fixture inlined by
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
//     unlimited for the duty cycle the runbook expects). A 24h run at
//     wF baseline rates (~1k-10k req/s steady-state) reaches ~10^8
//     to 10^9 requests; the 50k cap used by the smoke fixtures would
//     trip well before the soak window closes and crash the run.
//     2 G is the next round number the i32 request counter can reach
//     without overflow concerns under the documented load.
//
// Body shape is the same 512 B Repeat-fill the smoke fixture uses so
// the wF baseline (5,556 KiB plateau, 0.89 MiB/h proxy rate, 60-sample
// bit-identical post-warmup distribution) is directly comparable.
//
// See `.dev/D28_SOAK_RUNBOOK.md` for the full operating procedure,
// `.dev/D28_BLOCKERS.md::D28B-014` for the acceptance contract.

>>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= Repeat["x", 512]())
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe(18100, handler, 2000000000)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)

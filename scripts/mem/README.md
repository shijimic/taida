# scripts/mem/

Memory regression gate helpers for `@c.26` (C26B-010). Owned by
`.github/workflows/memory.yml`.

## Files

| File | Purpose |
|------|---------|
| `run_valgrind_smoke.sh` | Hard-fail leak gate. Runs valgrind memcheck against every `.td` fixture in `examples/quality/c26_mem_smoke/` and fails the job on any `definitely lost` byte. |

## Policy (C26B-010 hard-fail scope)

1. **Definitely-lost bytes are a regression**. Any fixture producing
   non-zero `definitely lost` via valgrind's `--leak-check=full
   --show-leak-kinds=definite --errors-for-leak-kinds=definite` fails
   the job with `--error-exitcode=1`.
2. **Indirectly-lost / possibly-lost bytes are surfaced, not gated.**
   These are commonly dominated by one-shot globals (tokio runtime
   thread pool, OnceLock panic-handler, etc.) that the OS reclaims on
   exit and are not regression signals for the interpreter's own
   Value drop paths. They appear in the `$GITHUB_STEP_SUMMARY` table
   so trends can still be eyeballed.
3. **Fixture crash / non-zero exit fails the gate too.** The script
   treats any valgrind wrapper rc != 0 as a failure regardless of the
   leak category attribution.

## Fixture contract (`examples/quality/c26_mem_smoke/`)

- Each fixture must complete in well under 10s under valgrind's ~10x
  slowdown (so <1s on a raw interpreter). The smoke set intentionally
  covers only the smallest representative paths:
  - `hello_smoke.td` — stdout + Int arithmetic (parser / type-checker /
    interpreter teardown).
  - `list_smoke.td` — `@[...]` + `fold` (List / BuchiPack Value drop,
    Cluster 4 territory — C26B-012 / C26B-024).
  - `string_smoke.td` — Str `.length()` (char-index cache +
    byte-level primitive, C26B-018 Option A+B+C).
- Additions are allowed only if they keep the single-fixture valgrind
  wall-clock under 10s on the GitHub Actions ubuntu-latest runner.

## Local reproduction

```bash
sudo apt-get install -y valgrind
cargo build --release --bin taida
scripts/mem/run_valgrind_smoke.sh target/release/taida
```

The log directory defaults to `target/mem-smoke/`; override with
`VALGRIND_LOG_DIR=/tmp/vg scripts/mem/run_valgrind_smoke.sh ...` if
needed.

## Why not heaptrack / peak-RSS on every run?

Heaptrack gives rich per-allocation timelines but is considerably
heavier than memcheck (both wall-clock and artifact size). `memory.yml`
runs it only on the weekly `schedule:` trigger to keep PR feedback
cycle tight. Peak-RSS gating lives with the perf gate
(`scripts/bench/` + `.github/workflows/bench.yml`, C26B-004) because it
shares the sample-count + EWMA baseline machinery and piggybacks on
the same runner profile.

## Escalation

If a fixture starts leaking on main, `.github/workflows/memory.yml`
fails hard. The hot-path investigation order is:

1. Check `target/mem-smoke/<fixture>.valgrind.log` for the originating
   stack. The top frame is almost always in `src/interpreter/value.rs`
   (Value drop), `src/interpreter/mold_eval.rs` (mold cache), or
   `src/interpreter/net/eval/` (span lifetime).
2. Bisect against the last green main commit using
   `target/release/taida` + `scripts/mem/run_valgrind_smoke.sh`.
3. File a C26B-xxx follow-up if the leak crosses into Cluster 1 (NET)
   or Cluster 4 (Runtime) abstraction territory; otherwise fix
   locally under the existing blocker.

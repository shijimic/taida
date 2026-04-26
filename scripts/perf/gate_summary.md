# D28 perf gate runbook (D28B-005 + D28B-013)

This runbook describes the four hard-fail gates that ship with the
`@d.X` stable initial release and the precise hand-off between PR
runs, main-push runs, and weekly cron runs.

The gates are **independent** (each owns its own workflow file) and
share only the regression-comparison engine
(`scripts/bench/compare_baseline.py`) so the `+10%` tolerance +
30-sample-gating-threshold + 10-sample-alpha-window EWMA logic is
identical across throughput and peak RSS.

> **D28B-027 terminology note (Round 2 wH follow-up)**: 30 is the
> `min_samples_required` field — the count at which the gate
> switches from WARN to hard-fail. 10 is the `--max-alpha-window`
> argument used by `scripts/bench/update_baseline.py` (`alpha = 1 /
> min(sample_count + 1, window)`), which controls how quickly the
> EWMA reflects new samples. The earlier "30-sample EWMA window"
> phrasing in this file conflated the two; the precise term is
> "30-sample gating threshold + 10-sample alpha-window".

## Gate matrix

| Gate | Workflow | Trigger | Hard-fail policy | Owns |
|------|----------|---------|-----------------|------|
| Throughput regression | `bench.yml` | PR + main-push + nightly cron + manual | +10% slow-down vs 30-sample-gating-threshold + 10-sample-alpha-window EWMA | D28B-005 |
| Peak RSS regression | `bench.yml` | PR + main-push + nightly cron + manual | +10% RSS growth vs 30-sample-gating-threshold + 10-sample-alpha-window EWMA | D28B-013 #2 |
| Valgrind definitely-lost | `memory.yml` | PR + push + manual | any `definitely lost` byte | D28B-013 #1 |
| Coverage threshold | `coverage.yml` | weekly cron + manual | line ≥ 80% / branch ≥ 70% on `src/interpreter/` | D28B-013 #3 |

Each row is described in detail below.

---

## 1. Throughput regression gate (D28B-005)

- **Source file**: `.github/workflows/bench.yml` (job `bench`).
- **Engine**: `scripts/bench/compare_baseline.py` against
  `.github/bench-baselines/perf_baseline.json`.
- **Inputs**: criterion bencher output from
  `cargo bench --bench perf_baseline -- --output-format bencher`,
  plus the three NET6-3b h2 throughput tests parsed by
  `scripts/bench/parse_net_throughput.py`.
- **Acceptance** (D28B-005):
  - baseline 30 samples or more accumulated per bench (auto via
    main-push `update-baseline` job; 30 is the gating threshold,
    not the alpha window — see top-of-file note).
  - +10% slowdown causes hard-fail (D28B-027 trade-off note: the
    `ns_median: 0.0` short-circuit path in `compare_baseline.py`
    means a real regression in the bootstrap window — before any
    samples accumulate — surfaces as `WARN` rather than `FAIL`.
    This is by design: failing PRs on a pristine baseline would
    block all merges. The trade-off is acceptable because the
    Phase 12 GATE evidence requires a green bench.yml run on the
    actual `feat/d28` HEAD, so the bootstrap window cannot hide a
    regression past the stable tag).
  - `test_net6_3b_native_h2_32_request_throughput_benchmark`,
    `test_net6_3b_native_h2_64kib_data_benchmark`, and
    `test_net6_3b_native_h2_32_stream_multiplex_benchmark` are part
    of the gate set.
  - `docs/STABILITY.md §5.1` throughput line is FIXED at `@d.X`.

### Bootstrapping the throughput baseline

The committed baseline as of `@d.X` carries `sample_count: 0` for
every bench. The first 30 main-push runs after the tag is cut
populate `ns_median` via the EWMA in `update_baseline.py`. During
this window, individual benches emit `WARN (baseline only has N
samples, < min 30)` instead of `FAIL`. Once all entries cross the
threshold, regression detection is fully active without any manual
intervention.

If a bench is intentionally rewritten (algorithmic change, new
fixture), reset its `sample_count` to 0 and document the reason in
`notes`. Do not edit `ns_median` manually except for the same
reset cycle.

### Runbook: investigating a throughput failure

1. Inspect the failing PR's `Compare against throughput baseline`
   step. The failed line shows `name: ns_base -> ns_current
   (+pct%)`.
2. Pull the artefact `perf-baseline-<sha>` (retained 30 days). It
   contains `bench-results.txt`, `bench-results-net.txt`,
   `cargo-test-net.log`, and the criterion `estimates.json` files.
3. Reproduce locally with the same `cargo bench` invocation and
   `--sample-size 20`. Local 16T noise is high; CI 2C 3-run median
   is the source of truth (per CLAUDE.md and the C24/C25/C26
   educational record).
4. Either land a fix that recovers the baseline, or — if the slow-
   down is intentional — bump the baseline JSON for that bench
   only, document the reason in `notes`, and update CHANGELOG.

---

## 2. Peak RSS regression gate (D28B-013 #2)

- **Source file**: `.github/workflows/bench.yml` (steps appended
  to job `bench` at `@d.X`).
- **Engine**: same `scripts/bench/compare_baseline.py` invoked
  against `scripts/perf/peak_rss_baseline.json`.
- **Inputs**: `scripts/perf/measure_peak_rss.sh` running
  `/usr/bin/time -v` against each
  `examples/quality/d28_perf_smoke/*.td` fixture, emitting
  bencher-format lines where the `ns/iter` slot stores peak RSS
  in KiB (units cancel under relative %).
- **Acceptance** (D28B-013 #2):
  - 30-sample gating threshold + 10-sample alpha-window per
    fixture (auto via main-push).
  - +10% RSS growth causes hard-fail (same bootstrap trade-off as
    the throughput gate — see §1).
  - `docs/STABILITY.md §5.5` Memory line is FIXED at `@d.X`.

### Adding a fixture to the RSS gate

1. Add a `.td` fixture under `examples/quality/d28_perf_smoke/`.
   Keep it under 1 second of wallclock so the gate adds < 5 s
   total to the bench job.
2. Add a `rss_<basename>` entry to
   `scripts/perf/peak_rss_baseline.json` with
   `ns_median: 0.0, sample_count: 0`.
3. Wait 30 main-push cycles for the EWMA to populate, or seed
   `ns_median` with a known-good measurement and record the seed
   value in `notes`.
4. The script auto-discovers the fixture; no workflow edit is
   needed.

### Runbook: investigating a peak-RSS failure

1. Inspect the failing PR's `Compare against peak-RSS baseline`
   step. The failed line names the fixture.
2. Pull the artefact `perf-baseline-<sha>`; the
   `target/perf-rss/<fixture>.time.log` contains the full
   `/usr/bin/time -v` output (page faults, RSS curves, etc).
3. Reproduce locally with `scripts/perf/measure_peak_rss.sh
   ./target/release/taida examples/quality/d28_perf_smoke/<fix>.td`.
4. Common causes: COW path regression (Cluster 4 abstraction),
   excessive Vec growth, leaked Arc clones in the value pool.
5. Either land a fix or — if the RSS growth is justified —
   reset `sample_count` to 0 and document the reason in `notes`.

---

## 3. Valgrind definitely-lost gate (D28B-013 #1)

- **Source file**: `.github/workflows/memory.yml` (job
  `valgrind-smoke`).
- **Engine**: `scripts/mem/run_valgrind_smoke.sh`.
- **Inputs**: `examples/quality/c26_mem_smoke/*.td`.
- **Acceptance** (D28B-013 #1): zero `definitely lost` bytes for
  every fixture. `indirectly lost` / `possibly lost` surface in the
  step summary but do not gate merge — those are commonly
  dominated by one-shot global allocations like the tokio runtime
  thread-pool, which are not regression signals.

The weekly heaptrack run on Monday 04:30 UTC is visibility-only
(`continue-on-error: true`) and exists to surface long-tail
allocation patterns; it does not gate merge.

### Runbook: investigating a valgrind failure

1. The failing job dumps captured stdout and the valgrind log
   inline. The leak summary lists the call stack of every
   `definitely lost` allocation.
2. Reproduce locally with `valgrind --leak-check=full
   --show-leak-kinds=definite --errors-for-leak-kinds=definite
   ./target/release/taida <fixture>`.
3. Common causes: missing `Drop` impl on a new Value variant,
   forgotten `Arc::strong_count` decrement, FFI layer leaks (rare
   — those usually surface as `possibly lost` first).

---

## 4. Coverage threshold gate (D28B-013 #3)

- **Source file**: `.github/workflows/coverage.yml`.
- **Engine**: `cargo-llvm-cov --lib --lcov` + a Python summariser
  that computes per-module line/branch percentages from the
  emitted `lcov.info`.
- **Inputs**: `cargo llvm-cov --lib`. Only library coverage is
  tracked; integration / e2e tests would inflate coverage with
  paths that are not the contract surface.
- **Triggers**: weekly cron (Mondays 04:00 UTC) +
  `workflow_dispatch`. Intentionally **not** PR-triggered because
  the instrumented build is ~3x slower; PR latency would double.
  The trade-off is recorded in `docs/STABILITY.md §5.5`.
- **Acceptance** (D28B-013 #3):
  - `src/interpreter/`: line ≥ 80%, branch ≥ 70%.
  - JS / native / wasm backends remain visibility-only at this
    generation. Promotion is post-stable scope.

### Runbook: recovering from a coverage failure

1. Pull the artefact `coverage-html-<sha>`. Open
   `target/llvm-cov/html/index.html` and drill into
   `src/interpreter/` to find files with low line / branch %.
2. Add unit tests under `src/interpreter/<module>/tests.rs` or
   integration tests targeting the missing branches. Avoid adding
   `#[cfg(test)]` panics that "cover" defensive paths but never
   exercise the production semantics.
3. Re-run the coverage workflow via `workflow_dispatch` to verify
   the gate passes before the next weekly cron.

If the threshold cannot be recovered (e.g. an entire submodule
becomes dead code that should be deleted instead of tested), the
threshold can only be lowered through the gen-bump policy at
`docs/STABILITY.md §6` — i.e. the `@d.X` → `@e.0` boundary or
later. Within `@d.*`, the threshold is contractual.

---

## Phase 12 GATE evidence

The `@d.X` stable initial release Phase 12 GATE requires the
following evidence to be linked from `docs/STABILITY.md §5.1` /
`§5.5` once collected:

- a green `bench.yml` run on `feat/d28` HEAD (both throughput +
  peak-RSS gates active, even if individual entries are still in
  WARN due to baseline collection — the policy is the gate, not
  the per-entry verdict);
- a green `memory.yml` run on `feat/d28` HEAD (definitely-lost = 0
  for all three smoke fixtures);
- a manually triggered `coverage.yml` run on `feat/d28` HEAD that
  meets the line ≥ 80 % / branch ≥ 70 % threshold for
  `src/interpreter/`.

The `tests/d28b_013_perf_gate_invariants.rs` invariant test (run
via `cargo test --release`) pins the structural shape of the four
gates so a workflow-side regression (e.g. `continue-on-error` re-
introduced, threshold lowered) is caught at the test layer
independently of CI.

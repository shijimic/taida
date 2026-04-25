# scripts/perf/

D28 perf hard-fail gate runbook (D28B-005, D28B-013).

This directory consolidates the perf observability + hard-fail
machinery promoted at the `@d.X` stable initial release. It
complements the existing `scripts/bench/` (regression gate harness)
and `scripts/mem/` (valgrind smoke gate) directories — all three
co-own the perf-cluster acceptance pinned by `docs/STABILITY.md
§5.1` (throughput) and `§5.5` (memory + coverage).

## Files

| File | Purpose |
|------|---------|
| `measure_peak_rss.sh` | Measures peak RSS of a `taida` invocation against a fixture, emits bencher-format output, supports `--check-against-baseline` for hard-fail. |
| `peak_rss_baseline.json` | Committed peak-RSS baseline keyed by fixture. Same schema convention as `.github/bench-baselines/perf_baseline.json` (EWMA + sample_count). |
| `gate_summary.md` | Human-readable runbook of the three D28 perf hard-fail gates and how they hand off between PR / main / weekly cron. |

## D28 perf gate matrix

| Gate | Trigger | Workflow | Hard-fail policy | Acceptance |
|------|---------|----------|-----------------|------------|
| Throughput regression | PR + main + nightly cron | `bench.yml` | +10% slowdown vs 30-sample EWMA baseline (per-bench WARN until 30 samples) | D28B-005 |
| Peak RSS regression | PR + main + nightly cron | `bench.yml` (extension) | +10% RSS growth vs 30-sample EWMA baseline (per-fixture WARN until 30 samples) | D28B-013 acceptance 2 |
| Valgrind definitely-lost | PR + push + manual | `memory.yml` | any `definitely lost` byte | D28B-013 acceptance 1 |
| Coverage threshold | weekly + workflow_dispatch | `coverage.yml` | line ≥ 80% / branch ≥ 70% on `src/interpreter/` | D28B-013 acceptance 3 |

The three "PR + main + nightly cron" gates run in parallel and never
share a baseline file — each gate owns its own JSON in
`.github/bench-baselines/` (throughput) or `scripts/perf/` (peak RSS).

## Peak RSS gate state machine

The peak RSS gate is structurally identical to the throughput gate:

1. **PR runs** call `measure_peak_rss.sh --check-against-baseline`.
   Fixtures with `sample_count < min_samples_required` (default 30)
   emit `WARN` and do not fail. Fixtures that cleared the threshold
   fail the job when they regress beyond `+10%`.
2. **main push runs** also append to the baseline JSON via the
   `update-baseline` job in `bench.yml` (EWMA median + increment
   `sample_count`).
3. **Scheduled / dispatched runs** behave like a PR run (gate-only).

The first 30 main-push runs after `@d.X` is tagged populate the
baseline. During that window, the gate is structurally hard-fail
but suppressed per-fixture by `min_samples_required`. This is
documented in `gate_summary.md`.

## Coverage gate budget note

The coverage gate is **not** PR-triggered as of `@d.X`. The
instrumented build is ~3x slower than the regular release build and
would double PR latency. The compromise pinned at the 2026-04-26
Phase 0 Design Lock:

- `coverage.yml` runs on weekly cron (Mondays 04:00 UTC) +
  `workflow_dispatch`.
- The threshold (line ≥ 80% / branch ≥ 70% on `src/interpreter/`)
  is **hard-fail** when the workflow runs.
- A regression of `src/interpreter/` coverage below the threshold
  on any weekly run blocks the next `@d.X` follow-up release.

This decision is recorded in `docs/STABILITY.md §5.5` so that a
future generation can revisit the trade-off without losing the
rationale.

## Adding a new fixture to the peak-RSS gate

1. Add the fixture path under `examples/quality/d28_perf_smoke/`.
2. Add a `<fixture>: { ns_median: 0.0, sample_count: 0, notes: "..." }`
   entry to `peak_rss_baseline.json` (zero-initialised).
3. Wait 30 main-push cycles for the baseline to populate, or
   manually seed a `ns_median` value if you have a known-good
   measurement. Manual seeds must be documented in `notes`.

Same convention as `.github/bench-baselines/perf_baseline.json`.

## Hand-off to other dirs

- `scripts/bench/` — throughput regression gate (pre-existing,
  `compare_baseline.py` etc.). Owned by C26B-004 → carried into D28
  unchanged.
- `scripts/mem/` — valgrind definitely-lost gate (pre-existing,
  `run_valgrind_smoke.sh`). Owned by C26B-010 → carried into D28
  unchanged.
- `scripts/soak/` — long-run NET soak runbook. Owned by C26B-005 +
  D28B-006 / D28B-014 (Round 2 wI). Out of scope for `scripts/perf/`.

## Related blockers

- **D28B-005**: Throughput regression gate hard-fail promote
  (C25B-004 body) — `bench.yml` `continue-on-error` removed at C26
  (C26B-004), so the D28 task is to confirm the gate state and
  flip `STABILITY.md §5.1` throughput line to FIXED.
- **D28B-013**: Memory / perf / runtime observability hard-fail
  gates — three-pillar acceptance (leak / RSS / coverage) all
  promoted to hard-fail at `@d.X`.

## Manual one-off measurement

```bash
# Build the release binary first.
cargo build --release --bin taida

# Measure a single fixture (does not hard-fail, prints bencher line).
scripts/perf/measure_peak_rss.sh target/release/taida \
    examples/quality/d28_perf_smoke/peak_rss_string.td

# Compare current measurements against the committed baseline
# (used by bench.yml job; here for local diagnosis).
scripts/perf/measure_peak_rss.sh target/release/taida \
    --check-against-baseline scripts/perf/peak_rss_baseline.json
```

# Perf-Router Baseline Derivation — C26B-024 Step 1 (wη Round 11, 2026-04-24)

This file documents how the thresholds in
`.github/bench-baselines/perf_router.json` were derived for the
`perf-router.yml` CI regression gate.

## Purpose

`C26B-024` (Native backend list / BuchiPack access 12-19× slower than JS)
was closed on @c.26 wε Round 10 (`78a70f4` / `baff13d`) after a Step 4
runtime refactor (Arena + Native char-cache + zero-copy slice view +
string freelist). Step 4 acceptance required:

- `Native real / JS real <= 2.0×`
- `Native sys / Native real <= 30%`

Both were satisfied on local 16T (Linux x86_64, gcc):

| Backend | real  | sys   | sys/real | vs JS |
|---------|------:|------:|---------:|------:|
| Native  | 0.34s | 0.03s | 9%       | 2.0×  |
| JS      | 0.17s | 0.01s | 6%       | 1.0×  |

Step 1 of C26B-024 is the **CI workflow wiring**: we must catch future
regressions that would push the Native path back toward the wT Round 8
baseline (Native/JS = 12.1×, sys/real = 81%). That is the role of
`perf-router.yml` + the ignored `c26b_024_router_perf_gate` test.

## Environment difference: local 16T → CI 2C

The wε Round 10 measurements are on a 16-thread developer workstation.
GitHub Actions `ubuntu-latest` is a 2-vCPU shared runner. Three axes
diverge:

1. **Less parallelism**: the router fixture itself is single-threaded,
   but concurrent system load on the runner (kernel workqueue, cgroup
   accounting, journald, other runners sharing the machine) is higher
   per available core. This adds wall-time variance to *both* backends.
2. **Smaller CPU cache / worse prefetch**: 2C Azure/Standard_D2s_v3-class
   cores have less L3 than the dev box; hot-loop list scans see more
   L3 misses, amplifying the Native/JS ratio modestly.
3. **Shorter absolute times**: the router fixture runs in 0.17-0.34s.
   On CI, clock resolution and per-run noise can shift a 0.17s JS run
   by ±0.03s (18%) and a 0.34s Native run by ±0.05s (15%). Ratios are
   more robust than absolute timings but still inherit that noise.

## Threshold derivation

### `native_js_ratio_max = 3.0` (local reference = 2.0)

- Start from the acceptance bar: 2.0.
- Add 1.5× headroom to absorb CI noise + 2C allocator contention.
- A Native/JS ratio of 3.0 is still dramatically better than the wT
  Round 8 baseline (12.1×) and the `@c.25.rc7` baseline (12-19×), so
  a regression to even 4.0 would be caught.
- If 5+ CI samples land consistently below 2.5, tighten to 2.5 (note
  added to `perf_router.json`).

### `sys_real_ratio_max = 0.40` (local reference = 0.09)

- The published C26B-024 Step 4 acceptance bar is 0.30.
- CI syscall overhead is higher than local (runner tracing, cgroup v2
  accounting, glibc arena contention). 0.40 gives 1.33× headroom over
  the 0.30 acceptance bar.
- This still catches the wT Round 8 baseline (0.81) comfortably and
  any regression approaching 0.50 (where the Arena fix was clearly
  not working).

## Sample-count gate state machine

The `perf_router.json` state block mirrors the `perf_baseline.json`
pattern from C26B-004:

- `min_samples_required = 5` (vs 30 for `perf_baseline`; we tolerate
  fewer samples because the ratios are already conservative and the
  fixture runs in <1s so variance from startup dominates).
- While `sample_count < 5`: violations emit `WARN:` lines but the
  workflow job **does not** hard-fail. This prevents noise-driven
  false positives during the first week of runs.
- Once `sample_count >= 5`: violations hard-fail.
- Manual override: `state.strict = true` forces hard-fail immediately;
  `state.strict = false` keeps WARN-only indefinitely (useful during
  known noisy infrastructure events).

## What is NOT gated here

- **Interpreter perf**: the interpreter is 2-3 orders of magnitude
  slower than Native by design (reference implementation, not a perf
  path). It is not timed by this gate.
- **Criterion micro-benches**: those live in `bench.yml` +
  `perf_baseline.json`. This gate owns only the router-scale E2E
  ratio.
- **NET throughput**: `net6_3b_native_h2_*` lives in `bench.yml`.
- **Memory / peak RSS**: C26B-010 tracks those separately.

## Updating the baseline

On successful main-branch runs the workflow appends the measured
ratios into `perf_router.json::state`:

- `sample_count += 1`
- `median_native_js_ratio` and `median_native_sys_ratio` EWMA-updated
- `last_run_sha` / `last_run_at` refreshed

PR runs **never** update the baseline; they only read it.

## Acceptance checklist for wη Round 11

- [x] `.github/workflows/perf-router.yml` runs on PR + push(main) + schedule + workflow_dispatch
- [x] `tests/c26b_024_router_bench_parity.rs::c26b_024_router_perf_gate` emits `PERF_ROUTER_*` lines
- [x] `.github/bench-baselines/perf_router.json` pins thresholds with documented headroom
- [x] First 5 runs are WARN-only (sampling phase); then hard-fail
- [x] Existing parity tests in `c26b_024_router_bench_parity.rs` are unchanged
- [x] D27 escalation checklist: 3/3 NO (no surface / no error-string / no existing-assertion edits)

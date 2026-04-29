# Taida Lang Stability Policy

> Target: **`@e.X`** (gen-E CLI surface stable candidate)
> Status: **E31 draft** — updated for the E31 top-level CLI hierarchy.
> The final stable tag number is chosen at release gate time; `@e.X`
> is used until then.

Related references:

- `PHILOSOPHY.md` — the four philosophies the language is bound to.
- `.dev/E31_BLOCKERS.md` — E31 CLI surface blockers and stable gate
  status.
- `.dev/E31_PROGRESS.md` — E31 phase map.
- `docs/reference/addon_manifest.md` — addon manifest schema.
- `docs/reference/operators.md`, `docs/reference/class_like_types.md`,
  `docs/reference/standard_library.md`, `docs/reference/standard_methods.md`
  — the surface whose compatibility is pinned by this document.

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

This is the constraint that underwrites the whole policy: humans watch
the shape of Taida, AI writes and reads it. That in turn demands that
the **shape** (surface) be predictable across generations. Internal
implementation details may move freely; the surface does not.

---

## 1. Versioning Scheme

Taida does **not** use semver. Versions are:

```
<gen>.<num>[.<label>]
```

| Component    | Meaning                                                                        |
| ------------ | ------------------------------------------------------------------------------ |
| `<gen>`      | Generation. Increments **only** for explicit breaking changes to the surface.  |
| `<num>`      | Iteration within a generation. Increments for additive / fix-only releases.    |
| `<label>`    | Optional pre-release tag (e.g. `rc1`, `rc7`). Absent on stable releases.       |

Examples:

- `@c.25.rc7` — 25th generation, 7th release-candidate iteration.
- `@c.25` — **skipped**; see §1.3.
- `@c.26.rcM` — gen-C fix-only RC cycle (this track).
- `@c.26` — gen-C stable (label absent) — the second candidate for
  gen-C stable, pursued by C26.
- `@d.28.rc1` — next breaking-change generation (see §1.2 below).

**Agents / automation must not write semver-shaped numbers (`0.1.0`,
`1.2.3`) into release artifacts, tag names, or manifest versions.**
Doing so is an immediate reject condition — see
`MEMORY/feedback_taida_versioning.md`.

**Build numbers are one-way.** `@c.26.rcM` does **not** auto-promote
to `@c.26`; the stable tag is a separate build with its own number.

### 1.1. Generation bump (= breaking change) policy

A breaking change may only land at a `<gen>` bump. Any of the following
are treated as breaking:

1. Removing or renaming an operator (the ten operators listed in §2.1).
2. Removing or renaming a prelude function, mold, or type.
3. Changing the observable semantics of an existing prelude function,
   mold, or operator in a way that makes a previously-legal program
   either stop compiling or compute a different value.
4. Tightening a type signature in the surface (widening is additive).
5. Removing or renaming a diagnostic code `E1xxx` used in tooling.
6. Incompatible changes to the addon manifest schema (§4).
7. Incompatible changes to the CLI command hierarchy or flag grammar
   documented in `docs/reference/cli.md`, including `taida build`,
   `taida way`, `taida graph`, `taida doc`, `taida ingot`,
   `taida init`, `taida lsp`, `taida auth`, `taida community`,
   and `taida upgrade`.

Additions that keep every previously-legal program working unchanged
do **not** constitute a breaking change. They land at a `<num>` bump.

### 1.2. D28 (breaking-change phase)

Generation D28 (originally planned as D26, renamed to D27 on
2026-04-24, then renamed to D28 on 2026-04-25 alongside the opening
of the C27 fix-only RC cycle — see
`MEMORY/project_d28_breaking_change_phase.md`) is reserved for the
breaking changes deliberately deferred from gen-C. The principal
motivators are (non-exhaustive):

- **Function name capitalisation cleanup** — `Str` / `lower` /
  `toString` etc. have drifted between `PascalCase` / `camelCase` /
  `lowercase`. D28 will pick one convention and migrate en masse.
- **WASM backend extension for addons** — gen-C locks `AddonBackend`
  to `Native | Interpreter` and rejects `Js`. D28 introduces a
  WASM backend, potentially requiring manifest schema changes
  (`targets` field, see §4.3).
- **Addon ABI v2** — host-side callbacks (`on_panic_cleanup`,
  termios-restore hook) that require manifest + loader coordination.
- **Diagnostic renumbering** — any cleanups that require renaming or
  renumbering `E1xxx` codes.

See `.dev/D28_BLOCKERS.md` and `MEMORY/project_d28_breaking_change_phase.md`
for the live worklist. Anything in that list is out-of-scope for
C25, C26, and C27 (the fix-only RC cycle series) even if it is
otherwise attractive.

> **Error-string note.** Through the gen-C generation, the
> diagnostic emitted by `src/addon/backend_policy.rs` (see §4.2)
> carried the legacy substring `wasm planned for D26`. **D28B-010
> (@d.X)** rewrites the diagnostic at the gen-D boundary in
> lock-step with the wasm-full addon backend widening: the new
> token list is `(supported: interpreter, native, wasm-full)` and
> tooling that matches on the substring `"supported: interpreter,
> native"` continues to work transparently (the prefix is preserved
> as the stable matchable token across the gen-C → gen-D boundary).
> The legacy `D26` reference has been removed.

### 1.3. `@c.25` label skip

The label-less `@c.25` tag is **skipped** (no re-issue condition).
`@c.25.rc7` was the final RC iteration of C25; the stable-candidate
effort continues as the C26 fix-only RC cycle with the label-less
`@c.26` as its target. Agents and release tooling must not attempt
to tag `@c.25` retroactively.

---

## 2. Stable Surface — what is guaranteed

Across a single generation (e.g. every release tagged `@c.25.*`), the
following is guaranteed to remain compatible:

### 2.1. Operators (exactly ten)

```
=     =>    <=    ]=>   <=[    |==    |     |>    >>>    <<<
```

Semantics and grammar positions are pinned by `docs/reference/operators.md`.
The single-direction constraint (`=>` and `<=` may not mix in one
statement; `]=>` and `<=[` may not mix) is permanent and will not be
relaxed.

### 2.2. Prelude functions, molds, and types

The prelude is enumerated by `docs/reference/standard_library.md` and
`docs/reference/standard_methods.md`. Every entry listed there is
pinned for the lifetime of the generation: name, arity, parameter
ordering, unmolding signature, and observable output.

Specifically covered:

- All ten operators (see §2.1).
- The `Lax[T]` / `Result[T, E]` / `Gorillax[T]` mold family
  (`docs/reference/class_like_types.md`).
- `Str[...]()` constructor (C22 / C23).
- Collection primitives (`List`, `HashMap`, `Set`, `Stream`) and
  their method surface.
- The `Async[T]` mold and `]=>` / `<=[` await semantics
  (modulo the async-redesign caveat in §5.3).
- Introspection pack shape (`docs/guide/12_introspection.md`).
- The four error-handling primitives: `Lax`, `throw`, `|==`,
  Gorillax ceiling.

Additive methods / overloads may land at any `<num>` bump; removal
or incompatible overloads may not.

### 2.3. Diagnostic codes `E1xxx`

Each `E1xxx` code listed in `docs/reference/diagnostic_codes.md` is a
stable name that tooling may match on. The human-readable message
text is **not** part of the contract (it may be clarified between
releases). Adding new `E1xxx` codes is additive. Renaming,
renumbering, or retiring codes is a breaking change.

### 2.4. CLI surface

The public CLI surface is:

- `taida` / `taida <FILE>`
- `taida build [target] <PATH>`
- `taida way <PATH>` and `taida way check|lint|verify|todo`
- `taida graph` and `taida graph summary`
- `taida doc`, `taida ingot`, `taida init`, `taida lsp`,
  `taida auth`, `taida community`, `taida upgrade`

The exact flag grammar is documented in `docs/reference/cli.md`.
Adding flags is additive. Changing the meaning of an existing flag,
tightening its argument grammar, retiring a flag, or moving a command
between top-level and hub scope is a breaking change.

### 2.5. File layout contracts

- `addon.toml` schema (see §4).
- `packages.tdm` resolution rules.
- `.taida/` workspace layout as consumed by `taida build`.
- The mapping from `.td` source files to addon facade nesting (the
  relative `>>> ./x.td` rules pinned in C25B-030 Phase 1E).

---

## 3. Non-Stable Surface — what may change

The following are **not** part of the stable surface. Consumers who
depend on them do so at their own risk; they may change at any
`<num>` bump.

- Internals of `src/interpreter/`, `src/codegen/`, `src/parser/`,
  `src/types/`, `src/js/`, `src/addon/`. These are implementation
  detail.
- The on-disk format of compiled native binaries and WASM artifacts
  produced by `taida build`. No guarantee is given that a binary
  produced at `@c.25.rc7` will run identically at `@c.25.rc8`; the
  reproducibility guarantee is at the **source** level, not the
  binary level.
- The exact wording of diagnostic messages (see §2.3; the code is
  stable, the text is not).
- The wallclock performance of any particular workload. Performance
  is tracked via the perf-gate harness (C25B-004) but not contractual.
- Host resource exhaustion, including out-of-memory termination. The
  native runtime may print a fatal allocation message and exit with
  status 1, while interpreter / JS / WASM surfaces remain
  host-runtime-dependent. Backend parity tests do not assert OOM
  message or recovery behaviour.
- Addon ABI major version: §4.4 permits ABI minor additions at a
  `<num>` bump, but major ABI revisions require a `<gen>` bump.
- Any file under `.dev/`. This directory is explicitly development
  scratch and not distributed.

---

## 4. Addon surface

Addons are a particularly sensitive part of the surface because they
are authored by third parties and distributed out-of-band.

### 4.1. Manifest (`addon.toml`)

The schema is frozen by `docs/reference/addon_manifest.md`. Adding
new optional fields is additive; removing or renaming existing fields
is a breaking change. Tightening validation on an existing field
(e.g. rejecting an input that was previously accepted) is a breaking
change.

### 4.2. Backend policy

Across the whole gen-C generation (`@c.25.*` and `@c.26.*`):

- `Native` — supported.
- `Interpreter` — supported (first-class, not a degraded fallback).
- `WasmFull` — **supported at @d.X** (D28B-010 widening, §6.2
  addition). The wasm-full backend reuses the same registry / facade
  path as Native and Interpreter; manifest authors opt in by adding
  `"wasm-full"` to the top-level `targets` array.
- `Js` — deterministically rejected; no dispatcher exists.
- `WasmMin` / `WasmWasi` / `WasmEdge` — deterministically rejected;
  no addon dispatcher in the stable contract at @d.X.

The error message `"(supported: interpreter, native, wasm-full).
Run 'taida build native' or use the interpreter; for wasm
targets, only 'wasm-full' supports addons."` is part of the stable
surface — tooling is permitted to match on the substring
`"supported: interpreter, native"` to detect the current policy.
That prefix is preserved verbatim across the gen-C → gen-D boundary
to keep existing matchers working; the trailing list grew to include
`wasm-full` as a §6.2 widening.

### 4.3. `targets` field (forward-compat pin)

`addon.toml` across the gen-C generation has **no** `targets` field.
The label-less `@c.26` stable release will ship the same schema.

When `targets` is introduced at a later generation (tentatively D28,
coupled with the WASM backend), the migration rule is **pinned now**
so that existing gen-C addons remain valid:

> An `addon.toml` with no `targets` field is interpreted as
> `targets = ["native"]`.

That is: the absence of `targets` means **native only**, matching the
gen-C reality. Addon authors who want multi-target support at D28+
opt in explicitly; addon authors who do nothing remain valid
native-only addons.

This rule is part of the stable surface and will not be revisited.
See `.dev/FUTURE_PROGRESS.md` Post-stable item 3 for the broader
multi-target roadmap.

### 4.4. ABI

The addon ABI version (`TaidaHostV1`, exported symbols, calling
convention) is frozen within a generation. Additive slots (new
callbacks at the end of the vtable, new optional exported symbols)
may land at a `<num>` bump. Reordering, renaming, or changing the
signature of an existing slot is a breaking change.

D28 is expected to introduce ABI v2 (adds `on_panic_cleanup` etc.
host callbacks). The gen-C generation (`@c.25.*` / `@c.26.*` /
`@c.27.*`) keeps ABI v1 intact for the full generation.

### 4.5. Publishing workflow

The `taida ingot publish` / `taida init --target rust-addon` workflow
(C25B-007 / RC2.6) is part of the stable surface. The release
workflow template (`crates/addon-rs/templates/release.yml.template`),
tag-push semantics, and `--dry-run` / `--force-version` flag behaviour
are pinned by `tests/init_release_workflow_symmetry.rs` and the
`tests/publish_*` suites.

Core-bundled addons (`taida-lang/os`, `taida-lang/net`,
`taida-lang/crypto`, `taida-lang/pool`, `taida-lang/js`) do **not**
pass through `taida ingot publish`; they are bundled through the
`CoreBundledProvider` path. The only externally-publishable official
addon at `@c.25.rc7` is `taida-lang/terminal`.

---

## 5. Deferred / Caveats

The following items are **not** covered by the current stability
contract. They are the reason `@c.26` is still an RC-track target
rather than a landed stable tag. All items below are owned by the
C26 fix-only RC cycle (see `.dev/C26_BLOCKERS.md`); none are
deferred past C26.

### 5.1. NET stable viewpoint

The following NET-adjacent items are owned by the **C26 fix-only
RC cycle** (`.dev/C26_BLOCKERS.md::C26B-001〜C26B-006` + C26B-026).
They block the label-less `@c.26` tag until FIXED; the severity
assignments below are pinned by the 2026-04-24 Phase 0 Design Lock:

- **HTTP/2 parity across interpreter / native / JS / wasm-wasi** —
  **FIXED at `@d.X`** (D28B-002 4-backend pin + D28B-012 + D28B-002
  paired arena-leak fix, Round 2 / wF + wG 2026-04-26; D28B-025
  Round 2 review follow-up sealed RFC 9113 §8.1.1 no-body
  content-length conformance). `tests/parity.rs` now pins 11 h2
  4-backend parity cases (`test_net6_3b_native_h2_d28b002_1..11_*`)
  on top of the 10 C26B-001 cases inherited from gen-C; the JS
  branch rejects with `H2Unsupported` and the wasm-wasi branch
  reuses the JS rejection path. The h2 server response path's
  per-stream and per-connection arena boundaries are sealed by
  `taida_arena_request_reset` calls in
  `taida_net_h2_serve_connection` (paired twin of the h1 fix
  D28B-012, ~1,000× RSS-growth improvement) and the no-body
  status response path strips `content-length` /
  `transfer-encoding` before HPACK encode (D28B-025), so the
  combination of 4-backend test pin + arena-leak fix + RFC 9113
  conformance closes the historical Cluster 1 gating.
- **Native h2 HPACK custom-header preservation** —
  **FIXED (2026-04-24, Round 2 / wC)**. C26B-026 (discovered as a
  sub-finding of C26B-001 Session 2 on 2026-04-24) was a Native h2
  response path where HPACK encoding dropped every custom response
  header (`set-cookie`, `content-type`, `x-request-id`, …) because
  `h2_extract_response_fields` in
  `src/codegen/native_runtime/net_h1_h2.c` re-wrapped
  `taida_list_get` results as Lax packs and then looked up `name`
  / `value` on the wrapper instead of the inner pack. Fixed to
  mirror the h1 encode path; the header cap was raised to match
  `H2_MAX_HEADERS = 128`. Regression pinned by
  `test_net6_c26b026_h2_multiple_custom_headers_3backend_parity`
  (3 custom headers + content-type; interpreter / native dumps
  byte-equal; JS H2Unsupported branch excluded).
- **TLS construction** — cert chains, ALPN, and verification modes
  that the current `taida-lang/net` facade covers. **3-backend
  construction-matrix pin FIXED at Round 11 wι review
  follow-up (2026-04-25)**: five new `test_net6_1c_c26b002_*`
  cases in `tests/parity.rs` pin symmetric behaviour across
  interpreter / JS / native for the missing-cert, key-only,
  plaintext-fallback (`tls = @()`), invalid-PEM-content, and
  unknown-protocol-token permutations. Live cert-rotation and
  full ALPN negotiation remain runtime-dependent and are
  observed through the C26B-005 soak runbook.
- **Port-bind race eradication** — **FIXED (2026-04-24, C26 Phase 3)**.
  C26B-003 landed the root-cause fix for the H2 parity flaky-bind
  timeout inherited from C25B-002. 100 consecutive CI-equivalent
  runs of the former flaky fixtures pass with no retry shim firing
  (the shim itself is retired by C26B-006). The MEMORY note
  `project_flaky_h2_parity.md` is archived. Listed here for
  audit continuity; the gating item for §5.1 is no longer C26B-003.
- **Throughput regression guard hard-fail promotion** —
  **FIXED (2026-04-24, Round 2 / wB; carried into `@d.X` by
  D28B-005, Round 2 wH 2026-04-26)**. C26B-004 promoted the
  `benches/perf_baseline.rs` harness from `continue-on-error` to
  hard-fail on 10 % regression against a 30-sample baseline. At
  `@d.X` (D28B-005), the same harness is reaffirmed: `bench.yml`
  carries no `continue-on-error` flag, the regression engine
  (`scripts/bench/compare_baseline.py`) gates with
  `--tolerance-pct 10.0 --min-samples 30`, and the contract is
  pinned by the `tests/d28b_013_perf_gate_invariants.rs` invariant
  test so a future workflow regression is caught at the test
  layer. The committed throughput baseline at
  `.github/bench-baselines/perf_baseline.json` accumulates samples
  via the `update-baseline` job on every main-push; entries with
  `sample_count < 30` emit per-bench `WARN` instead of `FAIL`
  during the bootstrap window. Per-bench gates include
  `test_net6_3b_native_h2_32_request_throughput_benchmark`,
  `test_net6_3b_native_h2_64kib_data_benchmark`, and
  `test_net6_3b_native_h2_32_stream_multiplex_benchmark`.
- **Scatter-gather long-run** — the `httpServe` path is verified
  under a 24-hour soak test via a manual runbook
  (`.dev/C26_SOAK_RUNBOOK.md`, C26B-005, Must Fix). Runbook
  **landed**; the 24 h run itself is the gating artefact.
- **HTTP parity retry-shim retirement** — C26B-006 removes the
  remaining retry shim once C26B-003 is FIXED at the root
  (Must Fix; landing is staged for the `wJ` NET-rest worktree).

The scope is pinned to the **3-backend** matrix (interpreter / JS /
native); the wasm targets are out of gen-C scope except for
C26B-020 pillar 3 (a widening addition, §6.2).

### 5.2. Addon WASM backend

**Gen-D widens the addon backend set to include `WasmFull`**
(D28B-010, §6.2 addition). The widening is structurally a §6.2
addition — the set of accepted backends grows; no existing addon
is reinterpreted — so it does not require a generation bump beyond
the gen-C → gen-D transition that is already happening at @d.X.
`AddonBackend::WasmFull` joins `Native` and `Interpreter` as a
first-class addon backend; manifest authors opt in by listing
`"wasm-full"` in the top-level `targets` array. Addons that omit
`targets` continue to default to `["native"]` (D28B-021 contract
preserved), so no existing addon is reinterpreted by the widening.

`WasmMin`, `WasmWasi`, and `WasmEdge` remain unsupported at @d.X.
Adding any of them to the supported set is a future widening and
must be made in lock-step with `AddonBackend::supports_addons` in
`src/addon/backend_policy.rs`, the manifest allowlist
`SUPPORTED_ADDON_TARGETS`, and the `addon_manifest.md` reference.

cdylib loading on the wasm-full backend at @d.X reuses the host's
native loader (the wasm-full target compiles to a wasm module that
calls back into the host runtime for addon dispatch). A wasm-side
dispatcher (cdylib loaded inside the wasm module sandbox) is
post-stable scope and tracked separately as a future improvement.

### 5.3. Async redesign

C25B-016 tracks an audit of async lambda closure lifetime across
suspend points. Until that audit lands, the `Async[T]` surface is
stable in **syntax and type shape** (pinned by §2.2) but the exact
behaviour of a lambda whose closure outlives its defining frame
through a `]=>` suspend is not contractual. Programs that depend on
this edge case should assume it will be redesigned at D28+.

### 5.4. Terminal addon async FIFO

`PENDING_BYTES` FIFO ordering across concurrent `ReadEvent()` calls
is owned by **C26B-012** (formerly tracked under C25B-019, promoted
to Must Fix at the 2026-04-24 Phase 0 Design Lock and coupled with
the BuchiPack interior Arc migration). The terminal addon's
behaviour under concurrent event-read becomes contractual at
`@c.26`; until then the ordering is not guaranteed.

### 5.5. Performance

**FIXED at `@d.X`** (D28B-005 throughput, D28B-013 memory + perf
+ coverage hard-fail gates, Round 2 wH 2026-04-26). The "FIXED"
designation here pins the **gate policy contract** — workflow
structure, hard-fail flags, tolerance / min-samples literals,
fixture set — not the empirical baseline collection (D28B-027
clarification, Round 2 review follow-up). The committed baselines
ship at `sample_count: 0` for the peak-RSS gate; per-bench
`update-baseline` jobs accumulate samples on every main-push and
the gate runs in WARN-only mode while `sample_count <
min_samples_required`. Empirical baseline stabilisation is the
**post-tag 30 main-push window**; until then a real regression in
that window is observable via the WARN line in CI logs but does
not hard-fail the PR. The four gates that ship with the stable
initial release are:

| Gate | Workflow | Trigger | Hard-fail policy |
|------|----------|---------|-----------------|
| Throughput regression | `bench.yml` | PR + main-push + nightly cron | +10% slow-down vs 30-sample-gating-threshold + 10-sample-alpha-window EWMA baseline |
| Peak RSS regression | `bench.yml` | PR + main-push + nightly cron | +10% RSS growth vs 30-sample-gating-threshold + 10-sample-alpha-window EWMA baseline |
| Valgrind definitely-lost | `memory.yml` | PR + push | any `definitely lost` byte |
| Coverage threshold | `coverage.yml` | weekly cron + manual | line ≥ 80% / branch ≥ 70% on `src/interpreter/` |

The "30-sample-gating-threshold + 10-sample-alpha-window" phrase
above is precise: 30 is the `min_samples_required` field — the
number of accumulated bench samples the baseline must hold before
the gate switches from WARN to hard-fail (D28B-027 terminology
clarification; the older "30-sample EWMA window" phrasing
conflated the two). 10 is the `--max-alpha-window` argument used
by `scripts/bench/update_baseline.py`, which determines how
quickly the EWMA reflects new samples (`alpha = 1 / min(sc + 1,
window)`).

The perf-gate harness (`benches/perf_baseline.rs`, inherited
from C25B-004 → C26B-004 hard-fail) is reaffirmed without policy
change. The peak-RSS gate is new at `@d.X` (D28B-013 acceptance
#2): the regression engine is the same
`scripts/bench/compare_baseline.py` invoked against
`scripts/perf/peak_rss_baseline.json`, with
`/usr/bin/time -v` capturing peak RSS in KiB across the
`examples/quality/d28_perf_smoke/*.td` fixtures (see
`scripts/perf/README.md` and `scripts/perf/gate_summary.md` for
the full runbook). The coverage gate is removed from
`continue-on-error` and ships with hard-fail thresholds for the
Source-of-Truth interpreter backend; JS / native / wasm
backends remain visibility-only at this generation by design
(promotion is post-stable scope).

The coverage gate is intentionally **not** PR-triggered at
`@d.X`. The instrumented build is ~3x slower than a regular
release build and would double PR latency. The trade-off,
documented at the 2026-04-26 Phase 0 Design Lock: the gate
runs on weekly cron + `workflow_dispatch` only, but is hard-
fail when run, and a regression below the threshold blocks the
next stable follow-up release.

The structural shape of all four gates (no
`continue-on-error: true`, the exact tolerance / min-samples /
threshold literals, the schema parity between the throughput
and peak-RSS baselines, and the existence of the
`d28_perf_smoke` fixtures) is pinned by the
`tests/d28b_013_perf_gate_invariants.rs` invariant test so a
future workflow-side regression is caught independently of CI
itself. Related runtime-perf work items
(`C26B-010` / `C26B-012` / `C26B-018` / `C26B-020` /
`C26B-024`) land alongside the gate promotion so the baseline
is measured against the post-fix runtime; the leak-fix half
that survived into D28 is owned by D28B-012 (NET runtime path
leak) under the Round 2 wF worktree and the 24-hour soak
verification by D28B-014 (Round 2 wI).

**Bytes I/O addendum (C26B-020 all three pillars, 2026-04-24):**
The `readBytesAt(path: Str, offset: Int, len: Int) -> Bytes` API
is landed across 3-backend (interpreter / JS / native) **and**
lowered for the `wasm-wasi` / `wasm-full` targets (Round 3 / wI:
new `src/codegen/runtime_wasi_io.c` WASI preview1
`path_open` + `fd_read` path, 64 MB runtime-configurable ceiling
preserved). The previous 64 MB ceiling of `readBytes` is
runtime-configurable on every target.

**Pillar 2 landed at Round 5 / wO** (commit `f15c145`). The
`Value::Bytes` variant now wraps `Arc<Vec<u8>>` internally
(`src/interpreter/value.rs`), so each `BytesCursorTake(size)` call
performs an `Arc::clone` (O(1) refcount bump) instead of
copying the entire byte buffer. `parse_bytes_cursor` returns
`(Arc<Vec<u8>>, usize)` to preserve the zero-copy path through
`make_bytes_cursor_arc`; destructive consumers use the new
`Value::bytes_take` helper (try-unwrap fast path, clone fallback)
so uniquely-owned buffers move in place. Acceptance pins:
256 MB × 16 chunks < 500 ms baseline and (with `TAIDA_BIG_BYTES=1`)
1 GB × 64 chunks < 2 s, with `Arc::ptr_eq` invariants asserted
in `tests/c26b_020_bytes_cursor_zero_copy.rs`.

The bytes I/O surface is therefore now **fully** contractual: the
`readBytesAt` signature is pinned across all four targets
(interpreter / JS / native / wasm-wasi+full), and the zero-copy
guarantee for `BytesCursorTake` has landed against the locked
Cluster 4 Arc + try_unwrap COW family abstraction
(`.dev/C26_CLUSTER4_ABSTRACTION.md`).

### 5.6. C26 + C27 fix-track progress snapshot (informational)

This subsection is **informational** and updated as fix-track
blockers land. It is not part of the stable surface contract and
may be removed once `@c.27` (or its successor) is tagged. Canonical
worklists:

- `.dev/C27_BLOCKERS.md` — current cycle (gen-C stable, **third
  candidate**, opened 2026-04-25 after C26 verdict).
- `.dev/C26_BLOCKERS.md` — predecessor cycle (gen-C stable, second
  candidate, `feat/c26` merged at `6c4fa5f`; the label-less `@c.26`
  tag was deferred to C27 per the Phase 14 GATE verdict).

The §5.6.0 subsection below holds the **`@c.27` GATE snapshot**
(current target). The §5.6.1 subsection holds the historical
**`@c.26` GATE status** retained as the inheritance basis for
which C26 blockers carry over to C27 versus close out
(`CLOSED (not required)`). The §5.6 main body below the snapshots
is the FIXED / OPEN catalogue carried forward from C26 with C27
overlays.

#### 5.6.0. `@c.27` GATE snapshot (informational, 2026-04-25)

The label-less `@c.27` tag is the **third candidate** for gen-C
stable. The label-less `@c.25` tag was skipped (see §1.3); the
label-less `@c.26` tag was deferred to C27 (see §5.6.1 below).
C27 is a fix-only RC cycle inheriting C26 residuals; no breaking
changes land here (see `MEMORY/project_c27_fix_cycle_track.md`).

Phase 0 Design Lock verdict (2026-04-25):

- C26 Phase 14 GATE inputs are absorbed: 7 of the 13 originally
  tentative C27B-001..013 blockers were closed out by C26 work
  and flipped to `CLOSED (not required)` (C27B-002 / 004 / 007 /
  008 / 009 / 011 / 013); 6 remain `confirmed Must Fix`
  (C27B-001 / 003 / 005 / 006 / 010 / 012). Six new blockers
  (C27B-022..027) were opened from C26 residuals
  (C26B-015 / 016 PARTIAL / 017 / 021 / 022 / 023) per the
  DEFERRED-zero policy.
- Final scope: **19 OPEN (confirmed)** + **7 CLOSED
  (not required)** + **1 FIXED (historical)** + **0 D28
  ESCALATED** + **0 tentative**. Critical 1 (C27B-003) +
  Must Fix 18.
- Phase configuration: Phase 2 / 4 / 7 / 8 / 9 / 11 / 13 are
  empty (their owning blockers were CLOSED at Phase 0).
  New sub-phases 5b (NET surface integrity, C27B-022) and 9b
  (Interpreter eval semantics, C27B-024) added. wasm widening
  (C27B-020 / 021) handled as an independent stream under
  §6.2 (addition only).

Round 1 + Round 1 review fix verdict (2026-04-25, on `feat/c27`):

- C27B-014 (port-bind announcement, opt-in env-var) — **FIXED**
  after fA review fix (`d79a884` → `dc4b985`).
- C27B-015 (proxy multi-backend dispatch) — **FIXED** after
  fA review fix.
- C27B-017 (CI smoke 1-min) — **FIXED** after fA review fix.
- C27B-019 (`docs/reference/` hygiene sweep) — **FIXED**
  (`666b938`); `docs/reference/README.md` writing guide pinned;
  generalised sweep regex
  (`[A-Z][0-9]+B-[0-9]+` / `@[a-z]\.[0-9]+`) holds the reference
  body at 0 hits modulo the documented version-syntax exceptions.
- C27B-020 / C27B-021 (wasm widening, addition only) —
  **FIXED** after fD review fix (`d6ca943` → `8fbdab2`); MUSL
  `fmod` for `taida_mod_mold_f` restores 4-backend numeric parity.
- C27B-022 (path traversal `..` 3-backend parity) — **FIXED**
  after fJ review fix (`29a9ea3` → `d2e5615`); 15 cases
  (5 × 3 backends) green; canonical reject error string
  unified across Interpreter / JS / Native; documented in
  `docs/reference/os_api.md §7`.
- C27B-024 (closure capture regression guard) — **FIXED**
  guard (`55f3ad7` → `d3ba552` empirical); 5/5 RED reproduces
  the HI-005-original symptom when the
  `eval.rs:820-821` 4-line guard is reverted, proving the
  guard is load-bearing.
- C27B-018 (native arena leak Option A) + C27B-028
  (Async/Str RC corruption Critical) — paired-fix scheduled
  for wH (Round 2). The Option A 1-line guard removal is
  **not** landed yet — `4 GB plateau` is not an acceptable
  stable basis per the C27B-018 acceptance.

Round 2 worktree map (2026-04-25, in flight):

- wE (NET tls-h2): C27B-001 / 003 / 006 / 027 NET portion.
- wH (Runtime perf): C27B-010 / 018 / 025 / 026 / 028.
- wI (Docs final amendment, this commit): C27B-012 / 023 /
  C27B-027 docs portion / C27B-022 docs amendment portion +
  Round 1 review M-1 / L-1 follow-up.

| GATE evidence row | Status (Round 2 boundary) |
| --- | --- |
| Blocker closure (`OPEN` / `tentative` / `PARTIAL` = 0) | **HOLD** — 13 OPEN remain across wE / wH / GATE prep (C27B-001 / 003 / 005 / 006 / 010 / 012 / 018 / 023 / 025 / 026 / 027 / 028 / Phase 14 itself); 6 FIXED on `feat/c27` Round 1 (C27B-014 / 015 / 017 / 019 / 020 / 021 / 022 / 024). |
| 3-backend parity (Interpreter / JS / Native) | **PENDING** — `cargo test --release --lib` baseline ≥ 2535 holds (1 known C27B-003 port-bind flaky on local 16T); `cargo test --release --test parity` baseline ≥ 662 inherited from C26 Round 10 / wε; new C27B-022 / 024 fixtures additive. |
| Backend matrix (interp / JS / native + wasm-min / wasi / edge / full) | **PENDING** — wasm widening (C27B-020 / 021) added without regression to existing wasm profiles per Round 1 fD review. |
| NET soak (24 h soak runbook + fast-soak proxy 30-min smoke 3-backend) | **HOLD** — agent-side proxy infra (014 / 015 / 017) FIXED; user-side 24 h PASS record (C27B-005) outstanding. |
| Security (audit / deny / Sigstore / SLSA / install-side verify) | **GREEN inherited** from C26 Round 11 wι (C26B-007 / 008 / 030 all FIXED); C27 introduces no security workflow change. |
| Perf / memory (RSS / FD / thread / throughput hard-fail; SLOW = 0; parallel ≥ 80%) | **HOLD** — C27B-018 native arena leak gate; 4 GB plateau not acceptable; Option A trial revealed C27B-028 silent corruption requiring paired fix in wH. |
| Docs hygiene (`grep -nE "Round [0-9]+\|[A-Z][0-9]+B-[0-9]+\|@[a-z]\.[0-9]+\|FIXED" docs/reference/ --exclude=README.md` 0 件) | **GREEN** modulo justified version-syntax exceptions (documented in `docs/reference/README.md §3`). `@c.27` snapshot landed in this §5.6.0; `CHANGELOG.md @c.27` section landed; M-1 Stream historical context restored to C25B-001 entry; L-1 sweep regex generalised. |
| PHILOSOPHY consistency (no new syntax / operator / breaking rename) | **GREEN** — all Round 1 fixes are surface-additive or internal refactors; C27B-019 / 023 docs amendments do not introduce new operators or break operator-10-only / single-direction constraints. |
| `@c.27.rcM` operation discipline (rcM not stable input; agent does not cut tags) | **GREEN** — no `@c.27*` tag has been cut on `feat/c27`; all RC tags are user-cut after GATE verdict. |

This subsection is informational (as is the rest of §5.6) and is
not part of the stable-surface contract. It will be removed
once `@c.27` is tagged. D28 escalation checklist for the wI
docs amendment that landed this snapshot: 3/3 NO — no public
mold signature / pinned error string / existing parity
assertion altered; reference body remains ID-free / tag-free /
date-free; CHANGELOG narrative restoration is additive.

#### 5.6. (continued) C26 inheritance catalogue (informational)

The remainder of §5.6 below holds the C26 FIXED / OPEN
catalogue carried forward as the inheritance basis for §5.6.0.

FIXED on `feat/c26` (Round 1 + Round 2 + Round 3 + Round 4 + Round 5 + Round 6 + Round 7 + Round 8 (wY + wZ + wT + wU + wX2) + wδ rolling amendment + Round 9 (wα + wβ + wδ) + Round 10 (wε) + wθ rolling amendment):

- **C26B-001** (Must Fix) — h2 3-backend parity pin reached 10
  cases (baseline GET / POST + C26B-001-{1..7}) at Round 3 / wE,
  meeting the 2026-04-24 Phase 0 acceptance threshold. The `§5.1
  → FIXED` flip remains held on the rest of Cluster 1.
- **C26B-003** (Critical) — port-bind race root cause.
- **C26B-004** — throughput regression gate promoted to hard-fail
  (Round 2 / wB).
- **C26B-005** runbook — `.dev/C26_SOAK_RUNBOOK.md` landed
  (Round 2 / wA); the 24 h run itself is still pending.
- **C26B-007** sub-phase 7.1 / 7.2 / 7.3 — SEC-002〜010 localised
  fixes, `cargo-audit` / `cargo-deny` promoted to hard-fail,
  C static analysis (`cppcheck` + `gcc -Wall -Wextra
  -Wformat-security`) wired into `.github/workflows/security.yml`
  with a pinned warning baseline.
- **C26B-007** sub-phase 7.4 — **SEC-011** Sigstore cosign keyless
  signing + SLSA provenance attestation wired into the
  `taida ingot publish` workflow (Round 2 / wB).
- **C26B-009** — parser state-machine transition graph
  (`.dev/C26_PARSER_FSM.md`) + arm-body throw propagation.
- **C26B-011** — Float parity (NaN / ±Inf / denormal) + Div /
  Mul divergence resolved across 3-backend. The signed-zero
  `-0.0` rendering path was split between rounds: interpreter +
  native landed at Round 6 / wS (`547972c`, `signbit`-aware
  `taida_float_to_str`); JS codegen landed at Round 7 / wV-a
  (`d00e896`), closing the last 3-backend divergence.
  `src/js/runtime/core.rs::__taida_float_render` probes
  `Object.is(v, -0)` before `toFixed(1)`, and `__taida_mul` holds
  the Number-path for `-0` operands so the BigInt integer
  fast-path does not collapse the sign on `-1.0 * 0.0`.
  `src/js/codegen.rs::Expr::FloatLit` adds defensive emission for
  `-0.0` / NaN / ±Infinity (Rust `f64::to_string` surfaces
  `"inf"` / `"-inf"` tokens that are invalid JS `Number`
  literals). Regression guard:
  `tests/c26b_011_signed_zero_parity.rs`.
- **C26B-014** — core-bundled packages (`taida-lang/os`, `net`,
  `crypto`, `pool`, `js`) resolvable without an explicit
  `packages.tdm` entry (Option B pinned, widening).
- **C26B-015** — native-backend path traversal no longer rejects
  project-root-internal `..` imports; root-escape still rejected.
- **C26B-016** — span-aware comparison mold family (`SpanEquals`
  / `SpanStartsWith` / `SpanContains` / `SpanSlice`) landed across
  3-backend (Round 2 / wD); `StrOf(span, raw) -> Str` function-form
  landed as the family's cold-path materialiser at Round 3 / wH
  via pure IR composition (no new C runtime helpers).
  **Option B+ complete**; Option A (auto-`Str` promotion of
  `req.method`) remains D28-deferred.
- **C26B-017** — Interpreter partial-application closure-capture
  bug fixed (Round 3 / wH); `makeAdder(10)(7) == 17` 3-backend.
- **C26B-019** — multi-line `TypeDef(field <= v, ...)`
  constructor parse + `taida way check` vs `taida build` parser
  divergence eliminated (widening, §6.2).
- **C26B-020** pillar 1 — `readBytesAt(path, offset, len)`
  3-backend API (see §5.5 addendum).
- **C26B-020** pillar 3 — `wasm-wasi` / `wasm-full` lowering of
  `readBytesAt` via `src/codegen/runtime_wasi_io.c`
  (WASI preview1 `path_open` + `fd_read`) landed at Round 3 / wI.
- **C26B-020** pillar 2 — `Value::Bytes` migrated to
  `Arc<Vec<u8>>` at Round 5 / wO (commit `f15c145`);
  `parse_bytes_cursor` returns `(Arc<Vec<u8>>, usize)` and
  `BytesCursorTake(size)` is now an `Arc::clone` (O(1)) rather
  than a full-buffer memcpy. Regression guards:
  `tests/c26b_020_bytes_cursor_zero_copy.rs` (256 MB × 16 < 500 ms
  baseline; `TAIDA_BIG_BYTES=1` scales to 1 GB × 64 < 2 s;
  `Arc::ptr_eq` proves the refcount-only path). All three pillars
  of C26B-020 are now FIXED; the downstream `bonsai-wasm` Phase 6
  unblock is material (acceptance-smoke still pending for the
  stable gate).
- **C26B-018** (B) + (C) — byte-level primitive paths +
  `StringRepeatJoin` mold landed at Round 4 / wK (commit
  `3e4c667`) across 3-backend
  (`src/interpreter/mold_eval.rs` / `src/js/runtime/core.rs` /
  `src/codegen/native_runtime/core.c`). Regression guards:
  `tests/c26b_018_byte_primitive.rs` +
  `tests/c26b_018_repeat_join.rs`.
- **C26B-018** (A) `Value::Str` Arc+COW foundation landed at
  Round 6 / wP (commit `6cf6648`). `Value::Str` migrated to
  `Arc<String>`; `Value::clone()` on a string is now an
  `Arc::clone` (one atomic increment) rather than an O(len)
  buffer copy. `Value::str()` / `Value::str_take()` helpers added,
  all call sites updated. Regression guard:
  `tests/c26b_018_str_arc_ptr_eq.rs` pins `Arc::ptr_eq` after
  `value.clone()` and after pass-through assignment.
- **C26B-018** (A) char-index cache layer landed at Round 8 / wU
  (commit `9e69f96`), flipping the Str super-linear hot path
  from O(n) to O(1). `Arc<String>` is wrapped in a new
  `StrValue { data: String, char_offsets: OnceLock<Vec<usize>> }`
  carrier — `Value::Str(Arc<StrValue>)` — so every pattern-match
  site continues binding `s: Arc<StrValue>` which derefs
  transparently to `&String` / `&str` through full trait
  forwarding (`PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Display`,
  `Hash`, `Default`, `AsRef<str>`, `AsRef<OsStr>`, `Borrow<str>`,
  `From<String>`, `From<&str>`). Display / ordering / hashing
  semantics and the addon ABI surface (`s.as_ptr()` / `s.len()`)
  are preserved bit-for-bit. Lazy `char_offsets` of length
  `char_count + 1` (final entry is the total byte length, used
  as a UTF-8 range sentinel) gives O(1)
  `Slice[str]()` / `CharAt[str, idx]()` / `Str.length()` /
  `Str.get(idx)` after first touch, and
  `Str.indexOf(sub)` / `.lastIndexOf(sub)` become O(log n) via
  `binary_search` over the cache (byte offsets returned by
  `str::find` / `str::rfind` round-trip exactly at char
  boundaries; non-boundary offsets yield `None`). Lock-free
  `OnceLock` matches the immutable-first model; no mutable
  interior state escapes. The negative-index Lax cast
  (`-1i64 as usize == usize::MAX`) is guarded by
  `idx.saturating_add(1)` so out-of-bounds `CharAt` continues
  to return `None`. 13 unit tests in
  `src/interpreter/value.rs::tests` pin ASCII + UTF-8 char
  counting (`aあ🙂b` = 9 B / 4 chars), `cached_char_at` /
  `cached_char_slice` / `cached_byte_to_char_index` round-trip
  + non-boundary rejection, `Arc` sharing of the cache across
  clones, and the `Value::str_take` unique + shared paths.
  Option (D) `StringBuilder` remains **discarded** (not
  deferred, not revisited at D28). D28 escalation checklist:
  3/3 NO — `Value::Str(Arc<StrValue>)` is an internal layout
  change, `StrValue` Deref is transparent, and no mold
  signature / pinned error string / existing assertion is
  altered.
- **C26B-012** BuchiPack interior Arc migration landed at
  Round 6 / wQ (commit `6f72f7c`). `Value::BuchiPack` migrated to
  `Arc<Vec<(String, Value)>>`; pack `Value::clone()` is now an
  `Arc::clone` vs field-by-field deep clone. New helpers
  `Value::pack()` / `Value::pack_take()`; write paths use
  `Arc::make_mut` or `Arc::try_unwrap` COW. Regression guard:
  `tests/c26b_012_buchipack_arc_ptr_eq.rs` pins `Arc::ptr_eq`
  invariants. The PENDING_BYTES FIFO (terminal addon concurrent
  `ReadEvent()`) portion of C26B-012 remains OPEN and is tracked
  separately.
- **C26B-024** `[FIXED]` — Native list / `BuchiPack` clone-heavy
  paths. **Step 2 + Step 3 Option A first pass** landed at
  Round 8 / wT (commit `81c4fc1`): a bounded per-thread freelist
  for 4-field Packs (`taida_pack4_freelist_{pop,push}` in
  `src/codegen/native_runtime/core.c`, `__thread` storage,
  32-entry cap, cross-thread release or cap overflow falls through
  to `free()`). Profiling identified `taida_lax_new` on every
  `list.get(i) ]=> x` as the hottest Native path (112 B Pack alloc
  dominating `bench_router.td` N=1000 / M=5000 at sys/real = 80%).
  Freelist dispatch re-initialises every slot on reuse — no stale
  child leaks — and the bounded per-thread cap prevents unbounded
  RSS growth. 3-run median of an internal 200k-wrapper micro-bench:
  baseline real 0.510 s / sys 0.412 s → freelist real 0.295 s /
  sys 0.240 s (delta real -42% / sys -42% / user -38%). Five
  3-backend parity tests in
  `tests/c26b_024_pack_freelist_parity.rs`
  (`lax_churn_int` / `lax_churn_str` / `lax_oob_empty` /
  `freelist_bound` / `mixed_type_lax`) are GREEN. **Step 4 full
  acceptance** landed at Round 10 / wε (`78a70f4` perf →
  `baff13d` merge): the `bench_router.td` hard-gate
  `Native ≤ JS × 2` with `sys/real ≤ 30%` is met contractually,
  with Native dropping from Round 8 / wT baseline
  real 2.05 s / sys 1.66 s / sys-ratio 81% / 12.1× JS to
  real 0.34 s / sys 0.03 s / sys-ratio 9% / 2.0× JS at
  N=200 × M=500 (3-backend parity, Linux x86_64, gcc). The
  six-change runtime refactor lives in
  `src/codegen/native_runtime/core.c` F1 and is superset-additive
  over wT: (1) thread-local bump arena (`TAIDA_ARENA_*`, 2 MiB
  chunks, per-thread chain, 16 B aligned; allocations ≤ 1024 B
  take the arena path, larger fall back to `malloc`; arena chunks
  reclaimed at process exit — the Native codegen does not emit
  `taida_release` for short-lived bindings, so the bump allocator
  is semantically equivalent to `malloc` for this workload),
  dropping `malloc` calls 2.97 M → ≈ 300 (**-99.99 %**);
  (2) tier-1 thread-local freelists for the residual
  `taida_release` paths (cap=16 List freelist +
  3-bucket small-string freelist ≤ 32 B / ≤ 64 B / ≤ 128 B),
  hooked from `taida_list_new` / `taida_str_alloc` (alloc side)
  and `taida_release` / `taida_str_release` (release side);
  arena-backed slabs bypass the freelist since `free()` on an
  arena pointer is UB; (3) heap-range tracker
  (`taida_safe_malloc` updates `[taida_heap_min, taida_heap_max)`
  on every `malloc`; `ptr_is_readable` fast-paths via O(1) range
  membership instead of syscalling `mincore`); (4) 64-entry
  `mincore`-page cache (`taida_mincore_cache`) wired into
  `taida_ptr_is_readable` / `taida_is_string_value` /
  `taida_read_cstr_len_safe`, dropping `mincore` syscalls
  9.45 M → 20 (**-99.9998 %**); (5) arena-aware `list_push`
  migration (`realloc()` on an arena-backed slab is UB, so when
  an arena-origin list grows past cap=16 the runtime `malloc`s
  the new capacity and `memcpy`s header + elements; the abandoned
  arena slot is reclaimed at process exit); (6) arena-skip guards
  in `taida_release` / `taida_str_release` so `free()` is never
  called on an arena pointer. Three new Round 10 parity tests
  (`tests/c26b_024_router_bench_parity.rs`:
  `router_bench_smoke_parity` / `list_push_arena_migration_parity`
  / `small_string_churn_parity`) confirm bit-for-bit 3-backend
  output under arena allocation, `list_push` arena-to-malloc
  migration, and small-string churn; the five Round 8 / wT
  pack4 freelist tests remain green (arena-skip guard inserted).
  Native `core.c` grows from 998,598 B to **1,012,971 B**
  (`F1_LEN` 251,878 → **266,252**; F2 / other fragments / interp
  / JS / all four wasm profiles unchanged — F1 absorbs the full
  +14,373-byte delta). Step 1 (CI perf regression gate wiring
  against `bench_router.td`) is additive infrastructure tracked
  separately under C26B-024 on `.dev/C26_BLOCKERS.md`; the
  stable-gate acceptance numbers above are the contractual
  baseline it will enforce once wired. D28 escalation checklist:
  3/3 NO — public mold surface untouched, error contract
  unchanged, all 185 parity tests + 880+ total tests green
  (including C25 / C24 / C23 / C21 regression guards and all
  four wasm profiles; wasm profiles are untouched by Round 10).
- **C26B-006** `[FIXED]` — HTTP parity retry shim retired at
  Round 4 / wJ (commit `c3805ff`). C26B-003 root-cause fix made
  the shim safe to remove; `tests/parity.rs` now binds to
  `0.0.0.0:0` and reads the concrete port via `getsockname()`
  with no retry wrapper. No existing assertion was rewritten
  (the shim's previous body was a no-op after C26B-003 landed).
- **C26B-022** authority (256 byte) — wire-parser enforcement
  landed at Round 4 / wJ (commit `c3805ff`) across h1 / h2 / h3
  (`src/interpreter/net_eval/{h1,h2,h3}.rs`); over-limit
  authorities return `400 Bad Request` symmetrically with the
  method / path limits from Round 3 / wE. The
  `-Wformat-truncation` warning-as-error CI gate promotion
  remains tracked as an OPEN residual below.
- **C26B-010** `[FIXED]` — Valgrind smoke on every push + weekly
  heaptrack run wired into `.github/workflows/memory.yml`
  (commit `e444f81`, Round 4 / wM). Smoke fixtures under
  `examples/quality/c26_mem_smoke/` (hello / list / string) pin
  the baseline; helper scripts at `scripts/mem/` automate the
  local reproduction. Peak-RSS drift rejects are contractual
  against this snapshot for the `@c.26` gate; the 24 h soak
  (C26B-005) is orthogonal and still pending.
- **C26B-021** — native `stdout` / `stderr` line-buffered at the
  C entry point via `setvbuf(_IOLBF, 0)` (Option B pinned).
- **C26B-022** Step 2 — interpreter-side h1 wire-parser
  enforcement of method (16 byte) + path (2048 byte) ceilings
  landed at Round 3 / wE (rejecting over-limit requests with
  `400 Bad Request`). The authority (256 byte) companion landed
  at Round 4 / wJ (see entry above); together the Step 2 scope
  is complete for interpreter-side h1 / h2 / h3. Native
  parser-side symmetry and the `-Wformat-truncation` CI
  promotion remain in the OPEN residuals below.
- **C26B-023** docs-path — `docs/reference/net_api.md` 2-arg
  handler `req.body` empty-span caveat + `readBody` /
  `readBodyChunk` / `readBodyAll` usage matrix landed at Round 3
  / wH. The runtime diagnostic (warn on direct `req.body` slice
  in 2-arg handlers) is part of the code-path completion tracked
  separately.
- **C26B-025** — `taida ingot publish` rejects stale `packages.tdm`
  self-identity before tag push.
- **C26B-026** — Native h2 HPACK custom-header preservation fix
  (Round 2 / wC). See §5.1.

Design decisions locked without code (informational):

- **Cluster 4 common abstraction LOCKED (wG Round 3, 2026-04-24)**:
  all Phase 10 blockers (C26B-010 / 012 / 018 / 020 pillar 2 /
  024) adopt the **Arc + try_unwrap COW family**
  (`.dev/C26_CLUSTER4_ABSTRACTION.md`). Zero-copy slice views are
  subsumed as a specialisation; the arena option is D28-deferred.
  No code landed in the wG session — the decision is a gating
  artefact for every Phase 10 follow-up session.

`@c.26` GATE-preparatory infrastructure sweeps (informational,
no behaviour change):

- **Round 7 / wX** (`eba5200`) — Rust 1.93 stricter
  `clippy::collapsible_if` + `cargo fmt` sweep. Two pre-existing
  `if … if …` nests folded into `let`-chain form
  (`src/pkg/provider.rs:250` CoreBundledProvider write-needed
  branch; `src/pkg/publish.rs:463` manifest-version label
  comparison). Four unrelated files re-formatted. `-D warnings`
  remains green on the updated toolchain.
- **Round 8 / wY** (`af5c443`) — test-doc clippy cleanup ahead of
  the `@c.26` GATE. Three lint categories confined to newly-added
  C26 test files: `doc_list_item_overindented` (rustdoc),
  `ptr_arg` (`&PathBuf` → `&Path` in test helpers where the
  buffer is never mutated), and a `zombie_processes` false-positive
  `#[allow]` on `spawn_and_wait_ready` in
  `c26b_022_native_authority.rs` (every caller pairs spawn with
  `drain_and_cleanup`, the pairing is just split across helpers
  so the lint cannot see it). Test files only — no `src/`
  changes, no `EXPECTED_TOTAL_LEN` impact, no parity fixture
  touched. D28 escalation checklist: 3/3 NO.

Round 8 merge narrative (all on `feat/c26`, authored in this
chronological order; merge commits `4c59078` (wZ) / `53e7040`
(wT) / `1cd42d2` (wU) land wY + wZ + wT + wU + wX2 onto the
same integration tip):

- **Round 8 / wY** (`af5c443`) — see the entry above.
- **Round 8 / wZ** (`ba720d3`, C26B-013 rolling docs amendment)
  — Round 6 + 7 catch-up. Promotes C26B-011 to full `[FIXED]`
  and adds Round 7 + Round 8 sections to the CHANGELOG. Because
  wZ was authored before wT / wU / wX2 committed, its Round 8
  subsection pre-announced only wY + itself; the wδ rolling
  amendment extends that narrative without contradiction.
- **Round 8 / wT** (`81c4fc1`, C26B-024 Step 2 + Step 3 Option A
  first pass — thread-local 4-field Pack freelist) — see the
  C26B-024 entry above. Drives the `EXPECTED_TOTAL_LEN` delta
  994,500 → 998,598 (`F1_LEN` 247,780 → 251,878).
- **Round 8 / wU** (`9e69f96`, C26B-018 (A) char-index cache
  layer) — see the C26B-018 (A) entry above. Internal `Value::Str`
  layout change, no `EXPECTED_TOTAL_LEN` impact (pure Rust refactor).
- **Round 8 / wX2** (`62fd54d`, C26B-008 CLOSED — advisory not
  required) — see the C26B-008 entry below. Removes the Round 6
  / wR advisory scaffold; rewrites `.github/SECURITY.md`,
  `CHANGELOG.md`, and this §5.6 snapshot. No `src/` change.

Round 9 merge narrative (all on `feat/c26`; merge commits
`e3aacd9` (wα) / `83b5f8a` (wβ) / `46210b5` (wδ)):

- **Round 9 / wα** (`7fd1500` fix → `e3aacd9` merge,
  **C26B-027 FIXED**) — `c25b_008_doc_examples_parse` baseline
  restored; the `docs/reference/net_api.md:258` / `:273` code
  fences were updated to the intended form. Docs-only path;
  no `src/` change, no parity fixture touched. Closes the
  first of the two OPEN blockers that Round 8 surfaced.
- **Round 9 / wβ** (`8fe8d49` fix → `83b5f8a` merge,
  **C26B-028 FIXED**) — release-workflow symmetry test now
  absorbs the two SEC-011 jobs (Sigstore `sign` +
  SLSA `provenance`) that landed at Round 2 / wB, and the
  **SEC-011 invariants are pinned into the symmetry test**:
  `sign` depends on `build-release`, `provenance` depends on
  `sign`, the verify-on-install step is present, and the
  keyless-signing OIDC audience matches the `taida ingot publish`
  workflow. Future release-workflow edits that break any of
  those invariants hard-fail the test rather than silently
  drifting. Closes the second of the two OPEN blockers from
  Round 8.
- **Round 9 / wδ** (`e3cefc0` → `46210b5` merge, C26B-013
  rolling docs amendment) — docs-only catch-up for the
  Round 8 merge order. Wraps up the rolling narrative for
  Round 8.

Round 9 closes both new OPEN blockers (C26B-027 / C26B-028).
`EXPECTED_TOTAL_LEN` is unchanged from Round 8 (998,598 B) —
docs / tests only, no native-runtime C delta.

Round 10 merge narrative (all on `feat/c26`; merge commit
`baff13d` (wε) on top of `78a70f4` perf):

- **Round 10 / wε** (`78a70f4` perf → `baff13d` merge,
  **C26B-024 Step 4 full acceptance FIXED**) — promotes
  C26B-024 from `[PARTIAL FIXED]` to `[FIXED]`. The
  `bench_router.td` hard-gate `Native ≤ JS × 2` with
  `sys/real ≤ 30 %` is met contractually at N=200 × M=500
  (Native real 0.34 s / sys 0.03 s / sys-ratio 9 % / 2.0× JS;
  down from Round 8 wT baseline real 2.05 s / sys 1.66 s /
  81 % / 12.1× JS). Six-change runtime refactor in F1 of
  `src/codegen/native_runtime/core.c` drives `malloc` calls
  2.97 M → ≈ 300 (**-99.99 %**) and `mincore` syscalls
  9.45 M → 20 (**-99.9998 %**). See the C26B-024 entry above
  for the full technical narrative. `EXPECTED_TOTAL_LEN`
  998,598 → **1,012,971** (`F1_LEN` 251,878 → **266,252**);
  F2 / other fragments / interp / JS / all four wasm
  profiles unchanged.

Round 10 `EXPECTED_TOTAL_LEN` rolls to **1,012,971 B**; F1
absorbs the full +14,373-byte delta.

wθ (this commit) is the C26B-013 rolling docs amendment
that absorbs the Round 9 + Round 10 merge narrative into
both the CHANGELOG `@c.26` section and this §5.6 snapshot,
and adds the `@c.26` GATE-READY status marker at §5.6.1.
No code change. D28 escalation checklist: 3/3 NO.

OPEN (owned by C26):

- **C26B-002** — TLS construction across 3-backend. Round 4 / wJ
  landed a TLS-observability surface tranche (interpreter
  `net_eval/h1.rs` + `h3.rs`); the full 3-backend TLS
  construction pin is still OPEN.
- **C26B-008** — **CLOSED (not required at `@c.26` stable)**.
  Taida Lang has no confirmed install base as of `@c.26` cycle, so
  there are no affected parties to notify. The underlying fix
  (`src/upgrade.rs::canonical_release_source_is_taida_lang_org`
  regression pin) has shipped since `@c.15.rc3`. GHSA / CVE
  publication was staged under Round 6 / wR but removed in Round 8
  alongside the `docs/advisory/` scaffold; the draft is recoverable
  from git history if an install base later emerges and the
  pre-`@c.15.rc3` window is confirmed exploitable against real users.
- **C26B-012** residual — `PENDING_BYTES` FIFO (terminal addon
  concurrent `ReadEvent()`). The BuchiPack Arc migration half of
  C26B-012 landed at Round 6 / wQ (agent-side DONE). The FIFO
  half lives in the `terminal` submodule and is **user-side**
  (downstream addon author action); the language agent does not
  touch the submodule under any condition, so this residual is
  tracked here only as a stable-gate checklist item.
- **C26B-024** residual (Step 1 — CI perf regression gate wiring)
  — **Step 4 full acceptance FIXED** at Round 10 / wε (see the
  FIXED list above); the remaining Step 1 work is the additive
  CI wiring of the `bench_router.td` hard-gate so regressions
  against the Round 10 / wε contractual baseline
  (Native real 0.34 s / sys 0.03 s / sys-ratio 9 % / 2.0× JS at
  N=200 × M=500) are caught automatically. The gate body itself
  is not a blocker for `@c.26` since Step 4 acceptance has
  already been measured; Step 1 is post-stable infrastructure
  polish.
- **C26B-013** — ongoing docs amendment (this §5.6 snapshot,
  and CHANGELOG re-syncs are part of the C26B-013 track). The
  `docs/advisory/` scaffold landed at Round 6 / wR was removed
  at Round 8 / wX2 alongside the C26B-008 closure. The Round 9
  rolling amendment continues under wδ; the Round 10 merge
  narrative is absorbed by the wθ amendment (this commit).
- **C26B-022** residuals — (a) `-Wformat-truncation` promotion
  to warning-as-error in CI, and (b) Native-side parser
  enforcement of the method / path / authority ceilings. The
  interpreter-side h1 / h2 / h3 enforcement itself is FIXED
  as of Round 4 / wJ; the Native companion is tracked in
  parallel Round-6 worktree wS.
- **Float denormal 3-backend rendering parity** (tracked under
  C26B-011's follow-up pin) — acceptance fixture still pending
  a cross-backend render audit; tracked for the next Cluster 5
  session. The signed-zero half of this follow-up closed at
  Round 7 / wV-a; only the denormal rendering audit remains
  under this pin.

#### 5.6.1. `@c.26` GATE status (historical, retained as the C27 inheritance basis; 2026-04-25 review)

> **Historical note**: this subsection is retained from the C26
> Phase 14 GATE review. It documents the per-blocker landing
> verdict that C27 Phase 0 Design Lock used to determine which
> C27B-001..013 to flip to `CLOSED (not required)` versus
> `confirmed Must Fix`. The `GATE-READY → HOLD` downgrade applied
> to `@c.26`; the label-less `@c.26` tag was deferred to C27
> rather than re-issued (see §5.6.0 above for the current
> `@c.27` snapshot).

As of the 2026-04-25 review amendment, the previous wθ
**GATE-READY** claim is downgraded to **HOLD**. The review
surfaced four items (source-of-truth drift, CI false-green
holes, the install-side half of SEC-011, and the 3-backend
TLS construction pin); three are now FIXED under the review
follow-up (C26B-002, C26B-029, C26B-030) and the last one
(C26B-005 — the 24 h soak PASS record itself) is a
user-action blocker.

| Agent-side blocker | Status | Landing |
| --- | --- | --- |
| C26B-001 (h2 3-backend parity, 10-case pin) | FIXED | Round 1 + Round 2 + Round 3 / wE |
| C26B-002 (full 3-backend TLS construction pin) | FIXED (review follow-up) | Round 11 wι (`test_net6_1c_c26b002_{1..5}_*`) |
| C26B-003 (port-bind race eradication, Critical) | FIXED | C26 Phase 3 |
| C26B-004 (perf-gate hard-fail) | FIXED after review hardening | Round 2 / wB + C26B-029 |
| C26B-005 (soak runbook + 24 h PASS) | REOPEN: 24 h PASS evidence is a user action | Round 2 / wA runbook + Round 11 wι fast-soak proxy |
| C26B-006 (retry-shim retirement) | FIXED | Round 4 / wJ |
| C26B-007 (SEC-002..010 + SEC-011 Sigstore/SLSA) | FIXED (release + install sides) | Round 2 / wB + Round 9 / wβ + Round 11 wι C26B-030 |
| C26B-008 (GHSA advisory) | CLOSED (zero install base) | Round 8 / wX2 |
| C26B-009 (parser FSM + arm-body throw) | FIXED | Round 1 |
| C26B-010 (memory-leak CI gate) | FIXED | Round 4 / wM |
| C26B-011 (float parity incl. signed-zero JS) | FIXED | Round 6 / wS + Round 7 / wV-a |
| C26B-012 (BuchiPack Arc + terminal PENDING_BYTES) | FIXED in code; terminal publish remains user action | Round 6 / wQ + Round 11 wζ |
| C26B-013 (rolling docs amendment) | FIXED (review follow-up) | Round 11 wθ + Round 11 wι |
| C26B-014 (core-bundled import-less) | FIXED | Round 1 |
| C26B-015 (path traversal) | FIXED | Round 1 |
| C26B-016 (span-aware mold family + StrOf) | FIXED | Round 2 / wD + Round 3 / wH |
| C26B-017 (partial-app closure capture) | FIXED | Round 3 / wH |
| C26B-018 (Str (A) + (B) + (C)) | FIXED | Round 4 / wK + Round 6 / wP + Round 8 / wU |
| C26B-019 (multi-line TypeDef + checker/build parity) | FIXED | Round 1 |
| C26B-020 (readBytesAt + BytesCursor + wasm-wasi) | FIXED | Round 1 + Round 3 / wI + Round 5 / wO |
| C26B-021 (stdout line-buffer, Option B) | FIXED | Round 1 |
| C26B-022 (HTTP wire byte ceilings, Step 3 Option B) | FIXED (interp-side h1/h2/h3) | Round 3 / wE + Round 4 / wJ |
| C26B-023 (2-arg body docs-path) | FIXED (docs) | Round 3 / wH |
| C26B-024 (Native clone-heavy + perf-router gate) | FIXED (Step 1–4 + review hardening) | Round 8 / wT + Round 10 / wε + Round 11 / wη + C26B-029 |
| C26B-025 (publish self-identity) | FIXED | Round 1 |
| C26B-026 (Native h2 HPACK custom headers) | FIXED | Round 2 / wC |
| C26B-027 (doc_examples_parse regression) | FIXED | Round 9 / wα |
| C26B-028 (release workflow symmetry + SEC-011 invariants) | FIXED | Round 9 / wβ |
| C26B-029 (CI perf gate false-green hardening) | FIXED | Review 2026-04-25 |
| C26B-030 (SEC-011 install-side verify wiring) | FIXED | Review 2026-04-25 (Round 11 wι) |

Remaining OPEN / REOPEN items on the stable-gate checklist:

- **C26B-005** — 24 h soak PASS record is the last remaining
  user action. `.dev/C26_SOAK_RUNBOOK.md` § 0.1 pins the
  acceptance as user-owned; the fast-soak proxy
  (`scripts/soak/fast-soak-proxy.sh`) provides a 30-min to
  3-hour short run for iteration but does **not** close the
  acceptance. A PASS record lands in `.dev/C26_PROGRESS.md`
  Phase 5 session log when the user runs the full 24 h soak.
- **C26B-012 terminal publish** — terminal submodule commit
  `4692fd8` still needs upstream push / PR / release tag
  before downstream users can pick up the FIFO fix.
- **Phase 14 GATE promotion itself** — user-approved
  `@c.26.rcM` → `@c.26` tag sequence. The agent does not
  cut either tag under any condition.
- **Downstream `bonsai-wasm` Phase 6 acceptance smoke**
  (C26B-020 acceptance; user action).
- **First signed official addon release** — SEC-011
  publish-side signing + install-side verify both FIXED;
  producing and tagging the first actually-signed release
  is user action.

This subsection is informational (as is the rest of §5.6)
and is not part of the stable-surface contract. It is
removed once `@c.26` is tagged. D28 escalation checklist
for the Round 11 wι review amendment: 3/3 NO — no public
mold signature / pinned error string / existing parity
assertion altered. The new `test_net6_1c_c26b002_*` cases
are additive; the new `src/addon/signature_verify.rs`
module is an install-time hook that fails closed only when
the operator explicitly opts in with
`TAIDA_VERIFY_SIGNATURES=required`.

---

## 6. Process

### 6.1. How breaking changes are introduced

1. The change is proposed in `.dev/D28_BLOCKERS.md` (or the
   successor D-series tracker) with motivation, migration plan,
   and an explicit statement of which §1.1 bullet it touches.
2. The proposal is reviewed and accepted / rejected by the
   maintainer (currently `shijimic`).
3. Accepted proposals land only at `<gen>` bumps.
4. A migration guide is written in `docs/guide/` before the
   `<gen>` release.

### 6.2. How additions are introduced

1. The addition is proposed in `.dev/FUTURE_PROGRESS.md` or a
   tracked blocker (`C26B-xxx` style, `C27B-xxx`, or `D28B-xxx`,
   or `FB-xx`).
2. The addition is implemented with 4-backend parity from the
   first commit.
3. It lands at the next `<num>` bump. No approval gate is
   required beyond the standard review / gatekeeper flow.

### 6.3. How bugs are fixed

1. A bug fix that changes observable semantics is **not** a
   breaking change **if** the previous behaviour was a documented
   bug or is demonstrably unintended. The fix lands at a `<num>`
   bump.
2. A bug fix that changes observable semantics in a way that would
   plausibly break well-written programs (rather than programs
   relying on a mis-behaviour) is escalated to §6.1 and held
   for the next `<gen>`.
3. The judgement call in step 1 vs step 2 is the maintainer's.
   The default in ambiguous cases is §6.1 (hold for `<gen>`).

### 6.4. Deprecation policy

A prelude symbol, CLI flag, or manifest field may be marked
`deprecated` at any `<num>` bump. A deprecation warning is emitted
by the compiler or CLI when the deprecated symbol is used. The
symbol is **not** removed until the next `<gen>` bump. The minimum
deprecation window is one full generation.

### 6.5. gen-D rationale and `@d.X` breaking-change manifest

This subsection enumerates the breaking changes that justify the
generation bump from `gen-C` to `gen-D` and that land at the
label-less `@d.X` tag (X is fixed by the CI build counter at the
Phase 12 GATE; the literal `@d.X` is used wherever the ordinal is
not yet known). Each item maps to a §1.1 bullet so downstream
tooling and addon authors can audit how each change is justified
under the policy in §6.1.

The single source of truth for the per-item acceptance evidence is
`.dev/D28_BLOCKERS.md`; this subsection is the surface-side
manifest pinned for the entire gen-D generation.

#### 6.5.1. Naming-rule lock and rule-violator normalisation

- **Locked rules** (D28B-001, `docs/reference/naming_conventions.md`):
  the seven naming categories (class-like type / mold type / schema
  PascalCase, function camelCase, buchi-pack field with
  function-value camelCase / non-function-value snake_case,
  variable holding function value camelCase / variable holding
  non-function value snake_case, constant SCREAMING_SNAKE_CASE,
  error variant PascalCase) and the type-variable convention
  (single capital letter such as `T`, `U`, `E`, `K`, `V`, `P`, `R`)
  are pinned for the whole gen-D generation.
- **Why this is breaking** (§1.1 bullet 2 — *Removing or renaming
  a prelude function, mold, or type*): symbols that violated the
  locked rules (for example buchi-pack non-function-value fields
  spelled `callSign`, `syncRate`, `updatedBy`) are renamed to the
  rule-conformant casing (`call_sign`, `sync_rate`, `updated_by`).
  Programs that referenced the old names by literal field access
  must be updated. Mechanical rewriting was provided during the D28
  RC work, but E31 no longer ships AST migration commands; current
  migrations are documented as manual guide steps.
- **Mold-form / function-form coexistence** (D28B-015):
  `Map[xs](_)` / `map(xs, _)`, `StrOf[span, raw]()` / `strOf(span, raw)`
  remain simultaneously valid; the lock confirmed that PascalCase
  mold-form and camelCase function-form occupy different naming
  categories and need not be unified.

#### 6.5.2. Lint hard-fail (E1801..E1809)

- **New diagnostic codes** (D28B-008, `docs/reference/diagnostic_codes.md`):
  E1801 buchi-pack non-function-value field rule violation,
  E1802 buchi-pack function-value field rule violation,
  E1803 schema field rule violation, E1804 PascalCase
  type-shape rule violation, E1805 reserved (constants, requires
  usage tracking — currently AST-only detection is impractical),
  E1806 type-variable single-letter rule violation, E1807 function
  rule violation, E1808 variable casing rule violation, E1809
  return-type `:` marker omission.
- **Why this is breaking** (§1.1 bullet 5 — *Removing or renaming a
  diagnostic code `E1xxx` used in tooling*, by addition / band
  expansion): the E18xx band is now reserved for naming-rule lints
  and is enforced as a CI hard-fail on the curated user-facing
  scope (`examples/*.td` minus `compile_*.td` and minus
  `examples/quality/`). Tooling that previously assumed the E18xx
  band was unused must be updated.

#### 6.5.3. Addon manifest `targets` field contract (D28B-021)

- **Default-inject contract**: `targets` is a new manifest field.
  Manifests that omit `targets` are treated bit-identically to
  manifests that declare `targets = ["native"]`; the loader
  injects the default explicitly rather than silently falling
  through. Unknown target strings are rejected at load time with
  diagnostic `[E2001] unknown addon target` and `[E2002] addon
  manifest targets must be a list of strings`.
- **Why this is breaking** (§1.1 bullet 6 — *Incompatible changes
  to the addon manifest schema*): the schema is widened to admit
  the field, but the rejection of unknown target strings is a new
  fail-closed surface that did not exist in gen-C.
- **Stable-after default-change policy**: once `@d.X` is tagged,
  the default value of `targets` (`["native"]`) is itself part of
  the surface contract. Changing it is a breaking change and is
  admissible only at the next generation bump (`@e.*`). A widening
  that adds a new admissible target string (e.g. `"wasm"`) without
  changing the default is additive and lands at a `<num>` bump.

#### 6.5.4. Historical migration tooling

D28 carried an AST rewrite prototype for naming-rule cleanup during
its RC work. That tool is not part of the E31 public CLI. The current
stable surface keeps `taida upgrade` for self-upgrade only; source
syntax migrations are handled by guide-driven manual edits.

#### 6.5.5. Auxiliary rules

- **`.td` filenames**: snake_case (D28B-001 auxiliary).
- **Module imports**: `<author>/<package>` slug pair, each in
  kebab-case (D28B-001 auxiliary).
- **Argument / field type-annotation forms A and B**: both
  `arg: Type` (form A, identifier without `:` prefix) and
  `arg :Type` (form B, type literal with `:` prefix) are valid;
  the writer chooses. The mixed form `arg: :Type` is rejected by
  the parser. The return-type position (`=> Type` vs `=> :Type`)
  is parsed leniently for backward compatibility but lints (E1809)
  warn when the `:` marker is absent.
- **`docs/reference/operators.md`**: opens with the per-context
  type-notation rules table that documents which positions
  require the `:` marker and which positions are
  identifier-position (D28B-016 acceptance landed in Round 1 wA).

### 6.6. Migration tooling

E31 does not provide AST migration tooling. `taida upgrade` is reserved
for upgrading the Taida binary itself. Source migrations such as the
E30 class-like syntax cleanup are manual and documented in
`docs/guide/migration_e30.md`.

### 6.7. Stable-after surface lock

After the `@d.X` tag is pushed, the stable-surface contract in
§§ 2-4 is in effect for the entire gen-D generation
(`@d.X.*` `<num>` increments). All breaking-change additions
proposed during gen-D follow §6.1 and land only at the next
generation bump (`@e.*`). The tracking files for those proposals
are `.dev/FUTURE_BLOCKERS.md` (post-stable items deliberately
deferred) and a future `.dev/E*_BLOCKERS.md` (will be created when
gen-E planning starts).

### 6.8. gen-E rationale and `@e.30` breaking-change manifest (Phase 9 pre-flight)

> **Status**: Phase 9 pre-flight (E30B-001〜011 stable prerequisite FIXED、
> 2026-04-28). Phase 9 GATE で final CI evidence と user tag 承認を反映して
> 完成。本節は §6.1 の gen bump policy 整合のため staged。

gen-E は **言語仕様 (`.td`) の破壊的変更** を land する gen 系列。`@e.30`
(gen-E 最初の stable) は D series 最終 stable (`@d.29`) の後続として、
型システム surface の構造的統一 + interface 機能 + defaultFn + addon facade
explicit binding を主軸に scope in する。

Per-item acceptance evidence は `.dev/E30_BLOCKERS.md::E30B-001〜011` を
single source of truth とする。本節 (`@e.30`) は gen-E の surface-side
manifest。

主要 breaking change:

- **§1.1 bullet 1 (型システム surface 統一)**: TypeDef / Mold 継承 / Error
  継承の 3 系統を `Name[?type-args] [=> Parent] = @(...)` 単一構文に統合
  (E30B-001、Lock-B/F)。
- **§1.1 bullet 5 (診断コード再定義)**: `[E1407]` umbrella / `[E1410]` 新意味 /
  `[E1411]` 番号移動 / `[E1412]` 新規 (E30B-008、Lock-B Sub-B3 / Lock-C)。
- **§1.1 bullet 6 (addon facade 明示 binding)**: `RustAddon["fn"](arity <= N)`
  形式の facade binding (E30B-007 sub-step B-2、Lock-G)。

### 6.9. E31 CLI hierarchy manifest

E31 is the gen-E CLI surface cleanup. It consolidates the previous
top-level command spread into semantic hubs:

- Quality commands move under `taida way`.
- Package commands move under `taida ingot`.
- `taida transpile` is removed in favour of `taida build js`.
- `taida inspect` is removed in favour of `taida graph summary`.
- `taida upgrade` is self-upgrade only; AST migration flags are rejected.

Removed commands return `[E1700]` with a replacement hint. There is no
deprecation alias period for E31.

Migration note: E31 does not ship an AST migration command. E30 source
updates are documented as manual guide steps in
`docs/guide/migration_e30.md`.

---

## 7. Scope note

This policy document itself lives at a stable URL
(`docs/STABILITY.md`) inside the Taida repository. It is intended
to be the document downstream projects and addon authors cite when
planning their own compatibility contracts. Changes to this
document that **tighten** the contract (reducing what consumers can
rely on) are themselves breaking changes and follow §6.1. Changes
that **widen** the contract (more guarantees to consumers) may land
at any `<num>` bump.

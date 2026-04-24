# Taida Lang Stability Policy

> Target: **`@c.26`** (gen-C stable — second candidate)
> Status: **provisional** — the label-less `@c.25` tag was **skipped**
> (see §1.3 below); the gen-C stable tag is now being pursued through
> the C26 fix-only RC cycle. Intermediate tags are `@c.26.rcM`. The
> policy contract in this document is pinned for the whole gen-C
> generation (`@c.25.*` and `@c.26.*`) so downstream tooling, packagers,
> and addon authors have a fixed target before stable is declared.

Related references:

- `PHILOSOPHY.md` — the four philosophies the language is bound to.
- `.dev/C26_BLOCKERS.md` — open quality blockers and their severity
  (C26 track; `.dev/C25_BLOCKERS.md` is archived).
- `.dev/C26_PROGRESS.md` — phase map for the C26 fix-only RC cycle.
- `.dev/D27_BLOCKERS.md` — breaking changes deferred to the gen-D phase.
- `docs/reference/addon_manifest.md` — addon manifest schema.
- `docs/reference/operators.md`, `docs/reference/mold_types.md`,
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
- `@d.27.rc1` — next breaking-change generation (see §1.2 below).

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
7. Incompatible changes to the CLI flag grammar for `taida build`,
   `taida run`, `taida init`, `taida publish`, `taida upgrade`.

Additions that keep every previously-legal program working unchanged
do **not** constitute a breaking change. They land at a `<num>` bump.

### 1.2. D27 (breaking-change phase)

Generation D27 (originally planned as D26 and renamed on 2026-04-24 —
see `MEMORY/project_d27_breaking_change_phase.md`) is reserved for the
breaking changes deliberately deferred from gen-C. The principal
motivators are (non-exhaustive):

- **Function name capitalisation cleanup** — `Str` / `lower` /
  `toString` etc. have drifted between `PascalCase` / `camelCase` /
  `lowercase`. D27 will pick one convention and migrate en masse.
- **WASM backend extension for addons** — gen-C locks `AddonBackend`
  to `Native | Interpreter` and rejects `Js`. D27 introduces a
  WASM backend, potentially requiring manifest schema changes
  (`targets` field, see §4.3).
- **Addon ABI v2** — host-side callbacks (`on_panic_cleanup`,
  termios-restore hook) that require manifest + loader coordination.
- **Diagnostic renumbering** — any cleanups that require renaming or
  renumbering `E1xxx` codes.

See `.dev/D27_BLOCKERS.md` and `MEMORY/project_d27_breaking_change_phase.md`
for the live worklist. Anything in that list is out-of-scope for
both C25 and C26 even if it is otherwise attractive.

> **Error-string note.** The legacy substring `wasm planned for D26`
> remains in the runtime diagnostic emitted by
> `src/addon/backend_policy.rs` (see §4.2). That substring is a
> **pinned surface token** for the entire gen-C generation and will
> not be renamed to `D27` mid-generation; doing so would break
> tooling that matches on the substring. The rename to `D27` in prose
> here is documentation-only. The token is planned to be rewritten
> at the gen-D boundary alongside the other breaking changes.

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
  (`docs/reference/mold_types.md`).
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

`taida build`, `taida run`, `taida init`, `taida publish`,
`taida upgrade`, `taida check`, `taida fmt` accept the flag grammar
documented in `docs/reference/cli.md`. Adding flags is additive.
Changing the meaning of an existing flag, tightening its argument
grammar, or retiring a flag is a breaking change.

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
- `Js` — deterministically rejected; no dispatcher exists.
- `Wasm` — deterministically rejected; planned for the D27
  breaking-change phase (see §1.2).

The error message `"(supported: interpreter, native; wasm planned for
D26). Run 'taida build --target native' or use the interpreter."` is
part of the stable surface for the gen-C generation — tooling is
permitted to match on the substring `"supported: interpreter, native"`
to detect the current policy. The literal `D26` token inside that
string is a pinned surface artefact from C25B-030 and is **not**
renamed to `D27` mid-generation; the rename is a gen-D breaking
change (see §1.2). New code should match on the
`"supported: interpreter, native"` prefix rather than the trailing
`D26` token.

### 4.3. `targets` field (forward-compat pin)

`addon.toml` across the gen-C generation has **no** `targets` field.
The label-less `@c.26` stable release will ship the same schema.

When `targets` is introduced at a later generation (tentatively D27,
coupled with the WASM backend), the migration rule is **pinned now**
so that existing gen-C addons remain valid:

> An `addon.toml` with no `targets` field is interpreted as
> `targets = ["native"]`.

That is: the absence of `targets` means **native only**, matching the
gen-C reality. Addon authors who want multi-target support at D27+
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

D27 is expected to introduce ABI v2 (adds `on_panic_cleanup` etc.
host callbacks). The gen-C generation (`@c.25.*` / `@c.26.*`) keeps
ABI v1 intact for the full generation.

### 4.5. Publishing workflow

The `taida publish` / `taida init --target rust-addon` workflow
(C25B-007 / RC2.6) is part of the stable surface. The release
workflow template (`crates/addon-rs/templates/release.yml.template`),
tag-push semantics, and `--dry-run` / `--force-version` flag behaviour
are pinned by `tests/init_release_workflow_symmetry.rs` and the
`tests/publish_*` suites.

Core-bundled addons (`taida-lang/os`, `taida-lang/net`,
`taida-lang/crypto`, `taida-lang/pool`, `taida-lang/js`) do **not**
pass through `taida publish`; they are bundled through the
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

- **HTTP/2 parity across interpreter / native / JS** —
  scatter-gather response handling, flow-control edge cases, and
  real-world client conformance (C26B-001, Must Fix, 3-backend).
  **Acceptance reached for the test-pin target (2026-04-24, Round 3
  / wE)**: C26B-001 now pins 10 h2 3-backend parity cases (7 new
  `test_net6_c26b001_*` cases — baseline GET, POST, GET+query,
  status 404, large body, + method PUT / DELETE / PATCH — plus 3
  pre-existing baseline fixtures). JS branch rejects with
  `H2Unsupported` per §5.1 in every case. The remaining gating
  work is the Sub-finding custom-header fix (C26B-026, below) and
  TLS construction (C26B-002). The `§5.1 → FIXED` flip is held
  until the rest of Cluster 1 (C26B-002 / C26B-004 / C26B-005 /
  C26B-006) also lands; the 10-case pin itself is stable.
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
  that the current `taida-lang/net` facade covers only partially
  (C26B-002, Must Fix, 3-backend).
- **Port-bind race eradication** — **FIXED (2026-04-24, C26 Phase 3)**.
  C26B-003 landed the root-cause fix for the H2 parity flaky-bind
  timeout inherited from C25B-002. 100 consecutive CI-equivalent
  runs of the former flaky fixtures pass with no retry shim firing
  (the shim itself is retired by C26B-006). The MEMORY note
  `project_flaky_h2_parity.md` is archived. Listed here for
  audit continuity; the gating item for §5.1 is no longer C26B-003.
- **Throughput regression guard hard-fail promotion** —
  **FIXED (2026-04-24, Round 2 / wB)**. C26B-004 promoted the
  `benches/perf_baseline.rs` harness from `continue-on-error` to
  hard-fail on 10 % regression against a 30-sample baseline.
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

Gen-C locks `AddonBackend::Wasm` as "rejected, planned for D27"
(see §1.2 for the D26→D27 rename note). The stable surface
contract at §4.2 explicitly permits D27 to add WASM support
without a `<gen>` bump, because doing so only widens the set of
accepted backends. The `targets` default-to-`["native"]` rule at
§4.3 ensures no existing addon is reinterpreted by the widening.

### 5.3. Async redesign

C25B-016 tracks an audit of async lambda closure lifetime across
suspend points. Until that audit lands, the `Async[T]` surface is
stable in **syntax and type shape** (pinned by §2.2) but the exact
behaviour of a lambda whose closure outlives its defining frame
through a `]=>` suspend is not contractual. Programs that depend on
this edge case should assume it will be redesigned at D27+.

### 5.4. Terminal addon async FIFO

`PENDING_BYTES` FIFO ordering across concurrent `ReadEvent()` calls
is owned by **C26B-012** (formerly tracked under C25B-019, promoted
to Must Fix at the 2026-04-24 Phase 0 Design Lock and coupled with
the BuchiPack interior Arc migration). The terminal addon's
behaviour under concurrent event-read becomes contractual at
`@c.26`; until then the ordering is not guaranteed.

### 5.5. Performance

No wallclock / RSS / throughput guarantee is made for any program
at `@c.25.rc7`. The perf-gate harness (`benches/perf_baseline.rs`,
inherited from C25B-004) tracks regressions but is
`continue-on-error` throughout the `@c.25.*` track. Hard-fail
gating is **C26 scope** (C26B-004, Must Fix): the label-less
`@c.26` tag ships with the gate promoted to hard-fail on 10%
regression against a 30-sample baseline. Related runtime-perf
work items (`C26B-010` / `C26B-012` / `C26B-018` / `C26B-020`
/ `C26B-024`) land alongside the gate promotion so the baseline
is measured against the post-fix runtime.

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

### 5.6. C26 fix-track progress snapshot (informational)

This subsection is **informational** and updated as C26 blockers
land. It is not part of the stable surface contract and may be
removed once `@c.26` is tagged. Canonical worklist is
`.dev/C26_BLOCKERS.md`.

FIXED on `feat/c26` (Round 1 + Round 2 + Round 3 + Round 4 + Round 5):

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
  `taida publish` workflow (Round 2 / wB).
- **C26B-009** — parser state-machine transition graph
  (`.dev/C26_PARSER_FSM.md`) + arm-body throw propagation.
- **C26B-011** — Float parity (NaN / ±Inf / denormal) + Div /
  Mul divergence resolved across 3-backend.
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
  `req.method`) remains D27-deferred.
- **C26B-017** — Interpreter partial-application closure-capture
  bug fixed (Round 3 / wH); `makeAdder(10)(7) == 17` 3-backend.
- **C26B-019** — multi-line `TypeDef(field <= v, ...)`
  constructor parse + `taida check` vs `taida build` parser
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
  `tests/c26b_018_repeat_join.rs`. Option (A) char-index cache
  remains OPEN (deferred to the next Cluster 4 session per the
  wO commit note; the Str super-linear hot path is bounded by
  (B) + (C) in the interim).
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
- **C26B-025** — `taida publish` rejects stale `packages.tdm`
  self-identity before tag push.
- **C26B-026** — Native h2 HPACK custom-header preservation fix
  (Round 2 / wC). See §5.1.

Design decisions locked without code (informational):

- **Cluster 4 common abstraction LOCKED (wG Round 3, 2026-04-24)**:
  all Phase 10 blockers (C26B-010 / 012 / 018 / 020 pillar 2 /
  024) adopt the **Arc + try_unwrap COW family**
  (`.dev/C26_CLUSTER4_ABSTRACTION.md`). Zero-copy slice views are
  subsumed as a specialisation; the arena option is D27-deferred.
  No code landed in the wG session — the decision is a gating
  artefact for every Phase 10 follow-up session.

OPEN (owned by C26, not yet landed):

- **C26B-002** — TLS construction across 3-backend. Round 4 / wJ
  landed a TLS-observability surface tranche (interpreter
  `net_eval/h1.rs` + `h3.rs`); the full 3-backend TLS
  construction pin is still OPEN.
- **C26B-008** — C25B-014 advisory publication + CVE request
  **(owner action)**. Advisory staging moved to `docs/advisory/`
  at Round 6 / wR so the owner can submit it via `gh api` without
  hunting inside `.dev/`. The agent does not publish; the
  publication / CVE request remains a strictly manual step.
- **C26B-018 (A)** char-index cache for `Value::Str`,
  **C26B-012** `PENDING_BYTES` FIFO + BuchiPack interior Arc
  migration, and **C26B-024** Native list / BuchiPack clone-heavy
  paths — the three remaining Cluster 4 items. All land against
  the locked Arc + try_unwrap COW abstraction; each is cross-cut
  on 500+ call sites and is scheduled across the next Phase 10
  sessions.
- **C26B-013** — ongoing docs amendment (this §5.6 snapshot, the
  `docs/advisory/` scaffold, and CHANGELOG re-syncs are part of
  the C26B-013 track).
- **C26B-022** residuals — (a) `-Wformat-truncation` promotion
  to warning-as-error in CI, and (b) Native-side parser
  enforcement of the method / path / authority ceilings. The
  interpreter-side h1 / h2 / h3 enforcement itself is FIXED
  as of Round 4 / wJ; the Native companion is tracked in
  parallel Round-6 worktree wS.
- **Float denormal 3-backend rendering parity** (tracked under
  C26B-011's follow-up pin) — acceptance fixture still pending
  a cross-backend render audit; tracked for the next Cluster 5
  session.

---

## 6. Process

### 6.1. How breaking changes are introduced

1. The change is proposed in `.dev/D27_BLOCKERS.md` (or the
   successor D-series tracker) with motivation, migration plan,
   and an explicit statement of which §1.1 bullet it touches.
2. The proposal is reviewed and accepted / rejected by the
   maintainer (currently `shijimic`).
3. Accepted proposals land only at `<gen>` bumps.
4. A migration guide is written in `docs/guide/` before the
   `<gen>` release.

### 6.2. How additions are introduced

1. The addition is proposed in `.dev/FUTURE_PROGRESS.md` or a
   tracked blocker (`C26B-xxx` style, or `D27B-xxx`, or
   `FB-xx`).
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

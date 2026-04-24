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
RC cycle** (`.dev/C26_BLOCKERS.md::C26B-001〜C26B-006`). They block
the label-less `@c.26` tag until FIXED; the severity assignments
below are pinned by the 2026-04-24 Phase 0 Design Lock:

- **HTTP/2 parity across interpreter / native / JS** —
  scatter-gather response handling, flow-control edge cases, and
  real-world client conformance (C26B-001, Must Fix, 3-backend).
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
- **Throughput regression guard hard-fail promotion** — the
  `benches/perf_baseline.rs` harness is `continue-on-error` today;
  C26B-004 promotes it to hard-fail on 10% regression against a
  30-sample baseline (Must Fix).
- **Scatter-gather long-run** — the `httpServe` path is verified
  under a 24-hour soak test via a manual runbook
  (`.dev/C26_SOAK_RUNBOOK.md`, C26B-005, Must Fix).
- **HTTP parity retry-shim retirement** — C26B-006 removes the
  remaining retry shim once C26B-003 is FIXED at the root
  (Must Fix).

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

**Bytes I/O addendum (C26B-020 pillar 1 partial, 2026-04-24):**
The `readBytesAt(path: Str, offset: Int, len: Int) -> Bytes` API
is landed across 3-backend (interpreter / JS / native) and the
previous 64 MB ceiling of `readBytes` is runtime-configurable.
The full `@c.26` stable gate still requires pillars 2 (zero-copy
`BytesCursorTake`) and 3 (`wasm-wasi` / `wasm-edge` / `wasm-full`
lowering) to land; until then the bytes I/O surface is
**partially** contractual (only the 3-backend `readBytesAt`
signature is pinned).

### 5.6. C26 fix-track progress snapshot (informational)

This subsection is **informational** and updated as C26 blockers
land. It is not part of the stable surface contract and may be
removed once `@c.26` is tagged. Canonical worklist is
`.dev/C26_BLOCKERS.md`.

FIXED on `feat/c26` (Round 1 + early Round 2):

- **C26B-003** (Critical) — port-bind race root cause.
- **C26B-007** sub-phase 7.1 / 7.2 / 7.3 — SEC-002〜010 localised
  fixes, `cargo-audit` / `cargo-deny` promoted to hard-fail,
  C static analysis (`cppcheck` + `gcc -Wall -Wextra
  -Wformat-security`) wired into `.github/workflows/security.yml`
  with a pinned warning baseline. Sub-phase 7.4 (SEC-011
  Sigstore + SLSA) is still OPEN and owned by C26 Cluster 2.
- **C26B-009** — parser state-machine transition graph
  (`.dev/C26_PARSER_FSM.md`) + arm-body throw propagation.
- **C26B-011** — Float parity (NaN / ±Inf / denormal) + Div /
  Mul divergence resolved across 3-backend.
- **C26B-014** — core-bundled packages (`taida-lang/os`, `net`,
  `crypto`, `pool`, `js`) resolvable without an explicit
  `packages.tdm` entry (Option B pinned, widening).
- **C26B-015** — native-backend path traversal no longer rejects
  project-root-internal `..` imports; root-escape still rejected.
- **C26B-019** — multi-line `TypeDef(field <= v, ...)`
  constructor parse + `taida check` vs `taida build` parser
  divergence eliminated (widening, §6.2).
- **C26B-020** pillar 1 — `readBytesAt(path, offset, len)`
  3-backend API (see §5.5 addendum).
- **C26B-021** — native `stdout` / `stderr` line-buffered at the
  C entry point via `setvbuf(_IOLBF, 0)` (Option B pinned).
- **C26B-025** — `taida publish` rejects stale `packages.tdm`
  self-identity before tag push.

OPEN (owned by C26, not yet landed):

- **C26B-001** / **C26B-002** / **C26B-004** / **C26B-005** /
  **C26B-006** — remaining NET stable viewpoint items (HTTP/2
  residuals, TLS config, throughput hard-fail promotion, 24h
  soak, retry shim retirement).
- **C26B-007** sub-phase 7.4 — SEC-011 Sigstore cosign + SLSA
  provenance wired into `taida publish` workflow.
- **C26B-008** — C25B-014 advisory publication + CVE request
  (owner action).
- **C26B-010** / **C26B-012** / **C26B-018** / **C26B-020**
  pillars 2 & 3 / **C26B-024** — Cluster 4 runtime perf items,
  gated on the common abstraction decision.
- **C26B-013** — ongoing docs amendment (this §5.6 snapshot is
  part of the C26B-013 track).
- **C26B-016** / **C26B-017** / **C26B-022** / **C26B-023** /
  **C26B-026** — remaining Cluster 6 surface / Cluster 1 NET
  items.

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

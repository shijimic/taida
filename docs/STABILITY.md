# Taida Lang Stability Policy

> Target: **`@c.25.rc7`** (in preparation)
> Status: **provisional** — this document lands inside an RC cycle. The
> label-less `@c.25` stable tag is *deferred* to a follow-up RC cycle
> (see CHANGELOG `@c.25.rc7` § Deferred). This file locks the policy
> contract now so that downstream tooling, packagers, and addon authors
> have a fixed target to aim at before stable is declared.

Related references:

- `PHILOSOPHY.md` — the four philosophies the language is bound to.
- `.dev/C25_BLOCKERS.md` — open quality blockers and their severity.
- `.dev/C25_PROGRESS.md` — phase map (Phase 9 = stability policy).
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
- `@c.25` — 25th generation, stable (label absent).
- `@c.26.rc1` — breaking-change generation (see D26 below).

**Agents / automation must not write semver-shaped numbers (`0.1.0`,
`1.2.3`) into release artifacts, tag names, or manifest versions.**
Doing so is an immediate reject condition — see
`MEMORY/feedback_taida_versioning.md`.

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

### 1.2. D26 (breaking-change phase)

Generation D26 is reserved for the breaking changes currently planned
but intentionally not executed during C25. The principal motivators
are (non-exhaustive):

- **Function name capitalisation cleanup** — `Str` / `lower` /
  `toString` etc. have drifted between `PascalCase` / `camelCase` /
  `lowercase`. D26 will pick one convention and migrate en masse.
- **WASM backend extension for addons** — C25 locks `AddonBackend`
  to `Native | Interpreter` and rejects `Js`. D26 introduces a
  WASM backend, potentially requiring manifest schema changes
  (`targets` field, see §4.3).
- **Addon ABI v2** — host-side callbacks (`on_panic_cleanup`,
  termios-restore hook) that require manifest + loader coordination.
- **Diagnostic renumbering** — any cleanups that require renaming or
  renumbering `E1xxx` codes.

See `.dev/D26_BLOCKERS.md` and `MEMORY/project_d26_breaking_change_phase.md`
for the live worklist. Anything in that list is out-of-scope for C25
even if it is otherwise attractive.

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

At `@c.25.rc7`:

- `Native` — supported.
- `Interpreter` — supported (first-class, not a degraded fallback).
- `Js` — deterministically rejected; no dispatcher exists.
- `Wasm` — deterministically rejected; planned for D26.

The error message `"(supported: interpreter, native; wasm planned for
D26). Run 'taida build --target native' or use the interpreter."` is
part of the stable surface for the `@c.25` generation — tooling is
permitted to match on the substring `"supported: interpreter, native"`
to detect the current policy.

### 4.3. `targets` field (forward-compat pin)

`addon.toml` at `@c.25.rc7` has **no** `targets` field. The
label-less `@c.25` stable release will ship the same schema.

When `targets` is introduced at a later generation (tentatively D26,
coupled with the WASM backend), the migration rule is **pinned now**
so that existing `@c.25` addons remain valid:

> An `addon.toml` with no `targets` field is interpreted as
> `targets = ["native"]`.

That is: the absence of `targets` means **native only**, matching the
`@c.25` reality. Addon authors who want multi-target support at D26+
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

D26 is expected to introduce ABI v2 (adds `on_panic_cleanup` etc.
host callbacks). `@c.25` will keep ABI v1 intact for the full
generation.

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

The following items are **not** covered by the `@c.25.rc7` stability
contract. They are the reason `@c.25.rc7` is an RC cycle and the
label-less `@c.25` release is deferred.

### 5.1. NET stable viewpoint

The following NET-adjacent items are the reason `@c.25.rc7` keeps
its `rc` label and the label-less `@c.25` stable tag is deferred:

- **HTTP/2 parity across interpreter / native / wasm** —
  scatter-gather response handling, flow-control edge cases, and
  real-world client conformance are not yet locked.
- **TLS construction** — cert chains, ALPN, and verification modes
  that the current `taida-lang/net` facade covers only partially.
- **HTTP parity under port-bind race** — hyper / tokio port-bind
  failures under concurrent tests still lean on the retry shim.
- **Port-bind race eradication (C25B-002)** — `flaky_h2_parity`
  is still papered over with a retry shim rather than
  eliminated at the root.
- **Throughput regression guard (C25B-004)** — the
  `benches/perf_baseline.rs` harness runs `continue-on-error`;
  there is no automated perf benchmark that blocks regressions.
- **Scatter-gather long-run** — the `httpServe` path has not
  been stressed with multi-hour runs yet.

These are expected to close out in a follow-up RC cycle (`@c.25.rcN+`
or `@c.26.rcM`, decision deferred) before the label-less `@c.25`
is tagged. The label-less `@c.25` tag **will not** be declared
inside the `@c.25.rc7` cycle.

### 5.2. Addon WASM backend

C25 locks `AddonBackend::Wasm` as "rejected, planned for D26".
The stable surface contract at §4.2 explicitly permits D26 to add
WASM support without a `<gen>` bump, because doing so only widens
the set of accepted backends. The `targets` default-to-`["native"]`
rule at §4.3 ensures no existing addon is reinterpreted by the
widening.

### 5.3. Async redesign

C25B-016 tracks an audit of async lambda closure lifetime across
suspend points. Until that audit lands, the `Async[T]` surface is
stable in **syntax and type shape** (pinned by §2.2) but the exact
behaviour of a lambda whose closure outlives its defining frame
through a `]=>` suspend is not contractual. Programs that depend on
this edge case should assume it will be redesigned at D26+.

### 5.4. Terminal addon async FIFO

C25B-019 tracks `PENDING_BYTES` FIFO ordering across concurrent
`ReadEvent()` calls. Until that lands (coupled with §5.3), the
terminal addon's behaviour under concurrent event-read is not
contractual.

### 5.5. Performance

No wallclock / RSS / throughput guarantee is made for any program.
The perf-gate harness (`benches/perf_baseline.rs`, C25B-004) tracks
regressions but is continue-on-error throughout `@c.25.rc7`.
Hard-fail gating is a post-stable follow-up.

---

## 6. Process

### 6.1. How breaking changes are introduced

1. The change is proposed in `.dev/D26_BLOCKERS.md` (or the
   successor D-series tracker) with motivation, migration plan,
   and an explicit statement of which §1.1 bullet it touches.
2. The proposal is reviewed and accepted / rejected by the
   maintainer (currently `shijimic`).
3. Accepted proposals land only at `<gen>` bumps.
4. A migration guide is written in `docs/guide/` before the
   `<gen>` release.

### 6.2. How additions are introduced

1. The addition is proposed in `.dev/FUTURE_PROGRESS.md` or a
   tracked blocker (`C25B-xxx` style, or `D26B-xxx`, or
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

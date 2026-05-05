# Taida Lang Stability Policy

> Target: **`@e.X`** (gen-E CLI surface stable candidate)
> Status: **E31 draft** — updated for the E31 top-level CLI hierarchy.
> The final stable tag number is chosen at release gate time; `@e.X`
> is used until then.

Related references:

- `PHILOSOPHY.md` — the four philosophies the language is bound to.
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
Doing so is an immediate reject condition.

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
of the C27 fix-only RC cycle) is reserved for the breaking changes
deliberately deferred from gen-C. The principal motivators are
(non-exhaustive):

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

The live worklist is tracked internally. Anything in that list is
out-of-scope for C25, C26, and C27 (the fix-only RC cycle series)
even if it is otherwise attractive.

> **Error-string note.** Through the gen-C generation, the
> diagnostic emitted by `src/addon/backend_policy.rs` (see §4.2)
> carried a legacy `wasm planned for D26` substring. At the gen-D
> boundary the diagnostic was rewritten to match the wasm-full
> addon backend widening: the token list is now `(supported:
> interpreter, native, wasm-full)`. Tooling that matches on the
> substring `"supported: interpreter, native"` continues to work
> transparently — the prefix is preserved as the stable matchable
> token across the gen-C to gen-D boundary. The legacy reference
> has been removed.

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

The `E19xx` band is reserved for build-driver diagnostics that
target the multi-backend mixed build surface
(`docs/reference/build_descriptors.md`). The band assignment and the
`build` block layout in JSONL records are stable; specific code
numbers within the band become contractual as their messages land.

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
- The mapping from `.td` source files to addon facade nesting,
  including the relative `>>> ./x.td` import rules.

### 2.6. Tail call and mutual recursion

Direct tail-call optimisation is contractual on every backend listed
in `docs/reference/tail_recursion.md`. Mutual recursion is supported
on the Interpreter and JS backends. The Native backend, and any
WASM profile that lowers through the same path as Native, reject
mutual recursion at compile time with `[E0700]`. There is no
fallback to ordinary stack-consuming calls hidden from the user.
Programs that require deep mutual recursion must either run on the
Interpreter or JS backend, or be rewritten into direct recursion or
iteration. A future generation may introduce a Native trampoline;
that is not in scope of `@e.X`.

### 2.7. Type-hierarchy graph schema

The five-graph introspection model has converged on a unified
class-like node kind. `MoldType`, `BuchiPackType`, and `ErrorType`
node kinds are merged into a single `ClassLikeType` node, and
`MoldInheritance` / `ErrorInheritance` edge kinds are merged into a
single `Inheritance` edge. The base lineage is preserved on the
node as `metadata.parent_lineage` (`"none"` / `"mold"` / `"error"` /
`"named"`). Tools consuming the graph schema must read the
`parent_lineage` metadata; relying on the old kind names is no
longer supported. The schema version exposed in the JSON output is
bumped accordingly.

### 2.8. Self-upgrade supply-chain gate

`taida upgrade` is a self-replacing executable path, so its release
verification policy is stable for `@e.X`:

- Release metadata is fetched only from `https://api.github.com` for
  `taida-lang/taida`; `TAIDA_GITHUB_API_URL` is not read by the
  production upgrade path.
- The release must publish `SHA256SUMS`, and that file must contain
  the selected archive name. Missing data is a hard failure with
  `[E32K1_UPGRADE_NO_SHA256SUMS]`; there is no unsigned escape hatch.
- `SHA256SUMS` is verified through cosign keyless verification before
  its hashes are trusted. The accepted certificate identity is pinned
  to the tagged `taida-lang/taida` GitHub Actions release workflow,
  with OIDC issuer `https://token.actions.githubusercontent.com`.
- The archive bytes are accepted only if their SHA-256 matches the
  verified `SHA256SUMS` entry. Self-replacement runs after those
  checks pass.

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
  is tracked via the perf-gate harness but not contractual.
- Host resource exhaustion, including out-of-memory termination. The
  native runtime may print a fatal allocation message and exit with
  status 1, while interpreter / JS / WASM surfaces remain
  host-runtime-dependent. Backend parity tests do not assert OOM
  message or recovery behaviour.
- Addon ABI major version: §4.4 permits ABI minor additions at a
  `<num>` bump, but major ABI revisions require a `<gen>` bump.
- Any internal development-scratch files that are not distributed
  in the published artifact tarball.

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
- `WasmFull` — **supported at @d.X** as a §6.2 additive widening.
  The wasm-full backend reuses the same registry / facade path as
  Native and Interpreter; manifest authors opt in by adding
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
A broader multi-target roadmap is tracked internally as a
post-stable item.

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

The `taida ingot publish` and `taida init --target rust-addon`
workflow is part of the stable surface. The release workflow
template (`crates/addon-rs/templates/release.yml.template`),
tag-push semantics, and the `--dry-run` and `--force-version`
flag behaviour are pinned by tests under `tests/`.

Core-bundled addons (`taida-lang/os`, `taida-lang/net`,
`taida-lang/crypto`, `taida-lang/pool`, `taida-lang/js`) do **not**
pass through `taida ingot publish`; they are bundled through the
`CoreBundledProvider` path. The only externally-publishable official
addon at `@c.25.rc7` is `taida-lang/terminal`.

---

## 5. Deferred / Caveats

The items below are areas that need explicit notes on what is and
is not covered by the stability contract.

### 5.1. NET stable viewpoint

The NET surface contract covers the following:

- **HTTP/2 parity across interpreter / native / JS / WASM (WASI)**.
  HTTP/2 is fully supported on the interpreter and native backends.
  The JS and WASI backends report the request as unsupported via a
  dedicated diagnostic rather than silently downgrading. The
  per-stream and per-connection arena boundaries are sealed against
  RSS growth, and the no-body status-response path strips
  `content-length` and `transfer-encoding` before HPACK encoding so
  RFC 9113 §8.1.1 conformance is preserved.
- **Native h2 HPACK custom-header preservation**. The native h2
  response path encodes custom headers (such as `set-cookie`,
  `content-type`, `x-request-id`) verbatim. The header capacity
  matches the public h2 limit.
- **TLS construction**. Certificate chains, ALPN protocol selection,
  and verification modes covered by the `taida-lang/net` facade are
  pinned with a 3-backend construction matrix. Missing-certificate,
  key-only, plaintext-fallback (`tls = @()`), invalid-PEM, and
  unknown-protocol-token cases produce identical behaviour across
  interpreter, JS, and native backends. Live certificate rotation
  and full ALPN negotiation remain runtime-dependent and are
  validated by long-running soak tests rather than unit tests.
- **Port-bind race eradication**. Server bind paths use real
  ephemeral-port allocation rather than retry shims; the previous
  retry shim is removed.
- **Throughput regression gate**. The throughput-regression harness
  runs as a CI hard-fail gate against a 30-sample baseline. See
  §5.5.
- **Long-run soak**. The `httpServe` path is verified under a
  24-hour soak test via a runbook stored separately.

The scope is pinned to the 3-backend matrix
(interpreter / JS / native); WASM targets share the byte-I/O
surface but are not subject to the full HTTP/2 parity matrix at
gen-C.

### 5.2. Addon WASM backend

**Gen-D widens the addon backend set to include `WasmFull`**
The widening is structurally an addition rather than a breaking
change — the set of accepted backends grows; no existing addon is
reinterpreted. `AddonBackend::WasmFull` joins `Native` and
`Interpreter` as a first-class addon backend; manifest authors opt
in by listing `"wasm-full"` in the top-level `targets` array.
Addons that omit `targets` continue to default to `["native"]`,
so no existing addon is reinterpreted by the widening.

`WasmMin`, `WasmWasi`, and `WasmEdge` remain unsupported at @d.X.
Adding any of them to the supported set is a future widening and
must be made in lock-step with `AddonBackend::supports_addons` in
`src/addon/backend_policy.rs`, the manifest allowlist
`SUPPORTED_ADDON_TARGETS`, and the `addon_manifest.md` reference.

cdylib loading on the wasm-full backend at @d.X reuses the host's
native loader (the wasm-full target compiles to a wasm module that
calls back into the host runtime for addon dispatch). A wasm-side
dispatcher (cdylib loaded inside the wasm module sandbox) is
post-stable scope and tracked as a future improvement.

### 5.3. Async closure lifetime

For the gen-E stable line the `Async[T]` closure-lifetime contract
is pinned to **capture by value**: when an async lambda is created,
the values it references are captured at that moment, and the
lambda keeps using those captured values across `]=>` suspend
points, independently of any later mutation in the defining frame.
The `Async[T]` syntax and type shape are also pinned (§2.2).

Any further redesign of this contract is deferred to a future
generation and will not land within `@e.X`.

### 5.4. Terminal addon event ordering

The terminal addon's `PENDING_BYTES` FIFO ordering across
concurrent `ReadEvent()` calls becomes contractual once the
shared-buffer migration lands. Until then the ordering is not
guaranteed.

### 5.5. Performance

The performance contract is the **gate policy** — workflow
structure, hard-fail flags, tolerance / minimum-samples literals,
and the fixture set — rather than a single empirical baseline
number. The four gates that ship with the stable release are:

| Gate | Workflow | Trigger | Hard-fail policy |
|------|----------|---------|-----------------|
| Throughput regression | `bench.yml` | PR + main-push + nightly cron | +10% slow-down vs the 30-sample EWMA baseline (10-sample alpha window) |
| Peak RSS regression | `bench.yml` | PR + main-push + nightly cron | +10% RSS growth vs the 30-sample EWMA baseline (10-sample alpha window) |
| Valgrind definitely-lost | `memory.yml` | PR + push | any `definitely lost` byte |
| Coverage threshold | `coverage.yml` | weekly cron + manual | line ≥ 80% / branch ≥ 70% on `src/interpreter/` |

The "30-sample-gating-threshold + 10-sample-alpha-window" phrase
above is precise: 30 is the minimum-samples-required value — the
number of accumulated bench samples the baseline must hold before
the gate switches from WARN to hard-fail. 10 is the
`--max-alpha-window` argument used by the baseline-update script;
it determines how quickly the EWMA reflects new samples
(`alpha = 1 / min(sc + 1, window)`).

The throughput-regression harness (`benches/perf_baseline.rs`)
is reaffirmed without policy change. The peak-RSS gate uses the
same regression engine invoked against a peak-RSS baseline
JSON file. Runtime peak RSS is captured in KiB by
`/usr/bin/time -v` against the perf-smoke fixtures shipped in
the repository. The coverage gate ships with hard-fail thresholds
for the interpreter backend (which is the source-of-truth
backend); the JS, native, and WASM backends stay
visibility-only at this generation by design (promotion is
post-stable scope).

The coverage gate is intentionally **not** PR-triggered. The
instrumented build is roughly 3x slower than a regular release
build and would double PR latency. The trade-off is that the
gate runs on weekly cron and on `workflow_dispatch` only, but
is hard-fail when run; a regression below the threshold blocks
the next stable follow-up release.

The structural shape of all four gates (no
`continue-on-error: true`, the exact tolerance / minimum-samples
/ threshold literals, the schema parity between the throughput
and peak-RSS baselines, and the existence of the perf-smoke
fixtures) is pinned by an invariant test under `tests/`, so a
future workflow-side regression is caught independently of the
CI configuration itself.

#### Bytes I/O contract

The `readBytesAt(path: Str, offset: Int, len: Int) -> Bytes` API
is contractual on all four backends (interpreter / JS / native /
WASM with WASI). On the WASI backend, the call lowers to a
`path_open` + `fd_read` sequence using the WASI preview1 API.
The 64 MB ceiling on byte-buffer construction is
runtime-configurable on every backend.

The `Value::Bytes` variant wraps an Arc-shared byte buffer
internally, so each `BytesCursorTake(size)` call performs an
O(1) reference-count bump instead of copying the entire buffer.
The cursor parser returns the buffer alongside its byte offset
to preserve the zero-copy path; destructive consumers use a
helper that prefers a try-unwrap fast path and falls back to a
clone only when the buffer is shared. Acceptance includes
256 MB × 16 chunks under 500 ms and (with the high-byte-count
opt-in environment variable) 1 GB × 64 chunks under 2 s, with
pointer-equality invariants asserted in the test suite.

## 6. Process

### 6.1. How breaking changes are introduced

1. The change is proposed with motivation, migration plan, and an
   explicit statement of which §1.1 bullet it touches.
2. The proposal is reviewed and accepted or rejected by the
   maintainer.
3. Accepted proposals land only at `<gen>` bumps.
4. A migration guide is written in `docs/guide/` before the
   `<gen>` release.

### 6.2. How additions are introduced

1. The addition is proposed and tracked.
2. The addition is implemented with 4-backend parity from the
   first commit.
3. It lands at the next `<num>` bump. No approval gate is
   required beyond the standard review flow.

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
generation bump from gen-C to gen-D and land at the label-less
`@d.X` tag. The build-counter ordinal is fixed at release time;
`@d.X` is used wherever the ordinal is not yet known. Each item
maps to a §1.1 bullet so downstream tooling and addon authors can
audit how each change is justified under §6.1.

This subsection is the surface-side manifest pinned for the entire
gen-D generation.

#### 6.5.1. Naming-rule lock and rule-violator normalisation

- **Locked rules** (`docs/reference/naming_conventions.md`):
  the seven naming categories (class-like type / mold type / schema
  PascalCase, function camelCase, buchi-pack field with
  function-value camelCase / non-function-value snake_case,
  variable holding function value camelCase / variable holding
  non-function value snake_case, constant SCREAMING_SNAKE_CASE,
  error variant PascalCase) and the type-variable convention
  (single capital letter such as `T`, `U`, `E`, `K`, `V`, `P`, `R`)
  are pinned for the whole gen-D generation.
- **Why this is breaking** (§1.1 bullet 2 — removing or renaming
  a prelude function, mold, or type): symbols that violated the
  locked rules (for example buchi-pack non-function-value fields
  spelled `callSign`, `syncRate`, `updatedBy`) are renamed to the
  rule-conformant casing (`call_sign`, `sync_rate`, `updated_by`).
  Programs that referenced the old names by literal field access
  must be updated. The current stable CLI does not ship an AST
  migration command; migrations are documented as manual guide
  steps.
- **Mold-form / function-form coexistence**:
  `Map[xs](_)` / `map(xs, _)`, `StrOf[span, raw]()` /
  `strOf(span, raw)` remain simultaneously valid. PascalCase
  mold-form and camelCase function-form occupy different naming
  categories and need not be unified.

#### 6.5.2. Lint hard-fail (E1801..E1809)

- **New diagnostic codes** (`docs/reference/diagnostic_codes.md`):
  E1801 buchi-pack non-function-value field rule violation,
  E1802 buchi-pack function-value field rule violation,
  E1803 schema field rule violation, E1804 PascalCase
  type-shape rule violation, E1805 reserved (constants, requires
  usage tracking that is currently impractical at the AST layer
  alone), E1806 type-variable single-letter rule violation, E1807
  function rule violation, E1808 variable casing rule violation,
  E1809 return-type `:` marker omission.
- **Why this is breaking** (§1.1 bullet 5 — adding diagnostic codes
  in a previously-unassigned band): the E18xx band is now reserved
  for naming-rule lints and is enforced as a CI hard-fail on the
  curated user-facing scope (`examples/*.td` minus `compile_*.td`
  and minus `examples/quality/`). Tooling that previously assumed
  the E18xx band was unused must be updated.

#### 6.5.3. Addon manifest `targets` field contract

- **Default-inject contract**: `targets` is a new manifest field.
  Manifests that omit `targets` are treated identically to
  manifests that declare `targets = ["native"]`; the loader
  injects the default explicitly rather than silently falling
  through. Unknown target strings are rejected at load time with
  diagnostic `[E2001] unknown addon target` and `[E2002] addon
  manifest targets must be a list of strings`.
- **Why this is breaking** (§1.1 bullet 6 — incompatible changes
  to the addon manifest schema): the schema is widened to admit
  the field, but the rejection of unknown target strings is a new
  fail-closed surface that did not exist in gen-C.
- **Stable-after default-change policy**: once `@d.X` is tagged,
  the default value of `targets` (`["native"]`) is itself part of
  the surface contract. Changing it is a breaking change and is
  admissible only at the next generation bump (`@e.*`). Adding a
  new admissible target string (for example `"wasm"`) without
  changing the default is additive and lands at a `<num>` bump.

#### 6.5.4. Historical migration tooling

An AST rewrite prototype was used internally during the gen-D RC
work for naming-rule cleanup. That tool is not part of the
public CLI. The current stable surface keeps `taida upgrade` for
self-upgrade of the binary only; source syntax migrations are
handled by guide-driven manual edits.

#### 6.5.5. Auxiliary rules

- **`.td` filenames**: snake_case.
- **Module imports**: `<author>/<package>` slug pair, each in
  kebab-case.
- **Argument / field type-annotation forms A and B**: both
  `arg: Type` (form A, identifier without `:` prefix) and
  `arg :Type` (form B, type literal with `:` prefix) are valid;
  the writer chooses. The mixed form `arg: :Type` is rejected by
  the parser. The return-type position (`=> Type` vs `=> :Type`)
  is parsed leniently for backward compatibility but lints (E1809)
  warn when the `:` marker is absent.
- **`docs/reference/operators.md`** opens with the per-context
  type-notation rules table that documents which positions
  require the `:` marker and which positions are
  identifier-position.

### 6.6. Migration tooling

The stable CLI does not provide AST migration tooling. `taida
upgrade` is reserved for upgrading the Taida binary itself.

### 6.7. Stable-after surface lock

After the `@d.X` tag is pushed, the stable-surface contract in
§§ 2-4 is in effect for the entire gen-D generation
(`@d.X.*` `<num>` increments). All breaking-change additions
proposed during gen-D follow §6.1 and land only at the next
generation bump (`@e.*`).

### 6.8. gen-E breaking-change manifest

gen-E は言語仕様 (`.td`) の構造的な破壊的変更を含む世代です。`@e.30`
(gen-E 最初の stable) は D 系列の最終 stable の後続として、型システム
surface の構造的統一、インタフェース機能、defaultFn、アドオンファサード
の明示 binding を主軸に scope in しています。

主要な破壊的変更:

- **型システム surface の統一** (§1.1 bullet 1): `TypeDef` /
  `Mold` 継承 / `Error` 継承の 3 系統を、単一の
  `Name[?type-args] [=> Parent] = @(...)` 構文に統合します。
- **診断コードの再定義** (§1.1 bullet 5): `[E1407]` の意味を
  umbrella 化し、`[E1410]` を新たに割り当て、`[E1411]` の番号を
  移動し、`[E1412]` を新規追加します。
- **アドオンファサードの明示 binding** (§1.1 bullet 6):
  `RustAddon["fn"](arity <= N)` 形式の binding をファサード先頭で
  必須にします。レガシーな暗黙 pre-inject は廃止します。

### 6.9. E31 CLI hierarchy manifest

E31 is the gen-E CLI surface cleanup. It consolidates the previous
top-level command spread into semantic hubs:

- Quality commands move under `taida way`.
- Package commands move under `taida ingot`.
- `taida transpile` is removed in favour of `taida build js`.
- `taida inspect` is removed in favour of `taida graph summary`.
- `taida upgrade` is self-upgrade only; AST migration flags are rejected.

Removed commands return `[E1700]` with a replacement hint. There
is no deprecation alias period: gen-E is allowed to break the CLI
shape immediately.

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

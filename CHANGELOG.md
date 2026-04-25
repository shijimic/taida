# Changelog

<!--
  Template for the gen-D stable initial release entry.
  Heading literal `@d.X` and `YYYY-MM-DD` are placeholders; the
  release-cutting user replaces X with the final build number and
  the date with the tag-push day at Phase 12 GATE. The §1-§9
  structure is fixed (Phase 0 Lock, D28B-022); only the section
  bodies fill in concrete verdicts.
-->

## @d.X — Stable initial release (YYYY-MM-DD)

> **Status: PLACEHOLDER.** This entry is a template invested ahead
> of the Phase 12 GATE. The version literal `@d.X`, the date, and
> every `<TBD>` placeholder below are filled in only after user
> approval at the GATE. Until then the entry is informational and
> must not be interpreted as a released stable tag.

### §1 Stable initial release

This is the first label-less stable tag of the **gen-D** generation
and the first stable tag of Taida Lang as a whole. All four pillars
of `PHILOSOPHY.md` (depth-free input, null/undefined elimination,
no implicit conversion, default-value guarantee under the
single-direction constraint) are honoured by the surface contracts
documented in `docs/STABILITY.md`.

- Predecessor RC chain: `@c.25.rc7` → C26 / C27 fix-only RC cycle
  series (label-less `@c.26` / `@c.27` were skipped per
  `docs/STABILITY.md` §1.3 / §5.6).
- Build number: `X = <TBD>` (CI build counter, fixed only at tag
  push).
- Tag scope: 4-backend parity (Interpreter / JS / Native /
  wasm-wasi). wasm-edge / wasm-full are widening additions with
  no regressions. wasm-min remains the floor.

### §2 Breaking changes from `@c.27` to `@d.X`

The gen increment from C → D is the explicit signal that this
release contains breaking changes. All renames are mechanical and
covered by `taida upgrade --d28`.

- Naming-rule normalisation (D28B-007 / D28B-008): symbols that
  violated the 7-category naming rules locked at Phase 0
  (`docs/reference/naming_conventions.md`) were renamed. Mold-form
  (PascalCase) and function-form (camelCase) names continue to
  coexist where both are rule-compliant.
- `<TBD: list of renamed symbols, including `strOf` function-form
  reaffirmation per D28B-015>`
- `<TBD: addon manifest `targets` contract surfacing per D28B-021,
  including the `["native"]` default-inject contract>`
- `<TBD: any further breaking surface changes landed in Round 3 wJ
  / wK or Round 4 wL>`

The single source of truth for breaking-change policy is
`docs/STABILITY.md` §6.

### §3 Migration guide

- Run `taida upgrade --d28` against your project root to apply the
  rename rewrites. The tool is single-direction; commit a clean
  state before invoking it.
- Manual review checklist: `<TBD: explicit checklist items not
  covered by the rewriter>`
- Addon authors: see `docs/reference/addon_manifest.md` for the
  `targets` field contract pinned by D28B-021.

### §4 NET stabilisation

- HTTP/2 4-backend parity (D28B-002): `<TBD>`
- Throughput regression hard-fail gate (D28B-005 + D28B-013):
  `<TBD>`
- Scatter-gather + 24h soak verification (D28B-006 + D28B-014):
  `<TBD>`
- TLS configuration (D28B-003): observation only, no active scope.
- Port-bind race (D28B-004): closed in C27 (C27B-003 FIXED).
- Native runtime path leak audit (D28B-012): `<TBD>`

### §5 Addon ecosystem

- Manifest `targets` field contract (D28B-021): default
  `["native"]` is loader-injected explicitly; missing-`targets`
  and explicit-`["native"]` are bit-identical at every observable
  surface; default changes after stable are admissible only across
  generations (`docs/STABILITY.md` §1.2 / §6).
- WASM addon dispatcher (D28B-010): **post-stable**. Deferred to
  D29 / E gen widening. See `.dev/FUTURE_BLOCKERS.md`.
- Bundled package surfaces: `taida-lang/os`, `taida-lang/net`,
  `taida-lang/terminal` (see `docs/guide/14_os_package.md`,
  `docs/guide/15_net_package.md`, `docs/guide/16_terminal_package.md`).

### §6 Memory and performance hard-fail gates

- Memory hard-fail gates (D28B-013): `<TBD>`
- Throughput baseline + percentage-drop gate (D28B-005 + D28B-013):
  `<TBD>`
- Coverage / RSS / FD / thread / unbounded-allocator gates:
  `<TBD>`

### §7 24h soak verification

- 24h scatter-gather soak runbook: `<TBD: pass / artefact link>`
- 30-min fast-soak-proxy 4-backend smoke: `<TBD>`

### §8 Known gaps

- POST-STABLE-001: WASM addon dispatcher (D28B-010) — D29 / E gen
  widening.
- `<TBD: any Should Fix items deferred to post-stable per the
  3-point post-stable check>`

### §9 Acknowledgements

`<TBD: contributor list, soak runners, reviewers, and addon
ecosystem maintainers who participated in the gen-D stable
qualification track>`

---

## @c.27 (in progress — gen-C stable, third candidate)

**Fix-only RC cycle, third wave.** The label-less `@c.25` and `@c.26`
tags were both **skipped** (see `docs/STABILITY.md` §1.3 / §5.6); the
gen-C stable tag is now being pursued as `@c.27`. Intermediate tags
are `@c.27.rcM`. No breaking changes land here — everything breaking
is deferred to the D28 generation (`.dev/D28_BLOCKERS.md`; the
rename history `D26 → D27 → D28` is documented in
`docs/STABILITY.md` §1.2).

The build-number rule is one-way: `@c.27.rcM` does **not**
auto-promote to `@c.27`. The stable tag is a separate build with
its own number, cut by the user only after the Phase 14 GATE
verdict in `.dev/C27_PROGRESS.md` reaches **GO**.

### Phase 0 Design Lock (2026-04-25)

- C27 inherits the C26 Phase 14 GATE verdict (`feat/c26` merged at
  `6c4fa5f`). 7 of the 13 originally tentative C27B-001..013
  blockers were closed out by C26 work and flipped to
  `CLOSED (not required)` (C27B-002 / 004 / 007 / 008 / 009 / 011 /
  013); 6 remain `confirmed Must Fix` (C27B-001 / 003 / 005 / 006 /
  010 / 012). Six new blockers (C27B-022..027) were opened from
  C26 residuals (C26B-015 / 016 PARTIAL / 017 / 021 / 022 / 023)
  per the DEFERRED-zero policy. Snapshot:
  19 `OPEN (confirmed)` + 7 `CLOSED (not required)` +
  1 `FIXED (historical)` + 0 `D28 ESCALATED` + 0 `tentative`.
- The Phase 14 GATE evidence template (Blocker closure / 3-backend
  parity / Backend matrix / NET soak / Security / Perf-memory /
  Docs hygiene / PHILOSOPHY consistency / `@c.27.rcM` operation
  discipline) is fixed in `.dev/C27_PROGRESS.md` and reproduced
  in `docs/STABILITY.md §5.6`.

### Round 1 (2026-04-25, parallel worktrees wA / wB / wC / wD)

`feat/c27` merge sequence: `666b938` (wC) → `d79a884` (wA) →
`d6ca943` (wD) → review fixes.

- **C27B-014 / C27B-015 / C27B-017** `[FIXED after fA review fix]`
  (wA: `d79a884`, fA review: `dc4b985`) — NET soak proxy
  infrastructure. Three-fold land in one merge: (1) **C27B-014
  port-bind announcement** — `httpServe` 1-arg / 2-arg paths now
  emit a deterministic `[port=<N>]` line on stdout under the
  opt-in env-var `TAIDA_NET_ANNOUNCE_PORT=1` (default OFF, so
  existing surface is non-breaking; `src/interpreter/net_eval/h1.rs:493`
  + JS / Native equivalents). (2) **C27B-015 fast-soak proxy
  multi-backend dispatch** — `scripts/soak/fast-soak-proxy.sh`
  gains `--backend {interp,js,native}`, supporting parallel
  3-backend soak runs against ports 18081 / 18082 / 18083.
  (3) **C27B-017 CI smoke** — new `.github/workflows/soak-smoke.yml`
  runs `--duration-min 1 --backend interp` as a 1-min smoke on
  every push (heredoc / parse-error catches without
  consuming CI wallclock). The fA review fix exported the
  `TAIDA_NET_ANNOUNCE_PORT` env-var to the proxy job and added a
  CI assertion that the announce line is grep-matchable from the
  proxy log (so the `USE_ANNOUNCE` branch is gated on a real CI
  hard-fail, not a silent fallback to fixed-port).
- **C27B-018 / C27B-028** — native `taida_str_alloc` arena leak
  Option A trial uncovered a Critical async/Str RC corruption
  (silent byte rewrite at offset 142 in
  `numbered_parity::fixture_numbered_13_async`); both blockers
  remain `OPEN` and are paired-fixed under wH in Round 2. The
  Option A 1-line guard removal is **not** landed on `feat/c27`
  yet — `4 GB plateau` is **not** an acceptable stable basis,
  per the C27B-018 Acceptance.
- **C27B-019** `[FIXED]` (wC: `666b938`) — `docs/reference/`
  hygiene sweep. 7 + 3 files migrated their RC-tag-narrative /
  blocker-ID-laden notes to `CHANGELOG.md`, leaving the
  reference body **ID-free** / **tag-free** / **date-free**:
  `addon_manifest.md` / `cli.md` / `graph_model.md` /
  `mold_types.md` / `net_api.md` / `os_api.md` /
  `tail_recursion.md` (initial 7) plus
  `diagnostic_codes.md` / `standard_methods.md` (re-sweep
  pickups), and the WASM SIMD note in `cli.md`. New
  `docs/reference/README.md` pins the writing guide
  (sections 1–6: responsibility split / forbidden patterns /
  justified exceptions / what to write / sweep regex /
  CHANGELOG cross-reference). The Round 1 review (M-1) flagged
  that the Stream-parity historical context disappeared from
  the reference; that context is now restored as a `@c.25.rc7`
  CHANGELOG sub-bullet (see C25B-001 entry below). The Round 1
  review (L-1) flagged that the sweep regex was hard-coded to
  `C2[0-9]B-[0-9]` / `@c\.[0-9]`; the regex is now generalised
  in `docs/reference/README.md §5` to
  `[A-Z][0-9]+B-[0-9]+` / `@[a-z]\.[0-9]+` so D28 / D29 /
  C28 cycles inherit the same discipline without manual regex
  edits per generation. Sweep stays at 0 hits
  (`grep -nE "Round [0-9]+|[A-Z][0-9]+B-[0-9]+|@[a-z]\.[0-9]+|FIXED" docs/reference/ --exclude=README.md`),
  with the version-syntax samples in
  `naming_conventions.md` / `operators.md` / `cli.md` /
  `standard_library.md:113` documented as justified exceptions.
- **C27B-020 / C27B-021** `[FIXED after fD review fix]`
  (wD: `d6ca943`, fD review: `8fbdab2`) — wasm widening
  addition (`STABILITY §6.2`): wasm-wasi bytes mold +
  wasm-full polymorphic_length lowering, plus wasm Float
  Div + bitwise / shift mold lowering. The fD review fix
  swapped in MUSL `fmod` for `taida_mod_mold_f` so 4-backend
  numeric parity holds across interpreter / JS / native /
  wasm under the new wasm code path. `core_wasm` FROZEN region
  untouched; lowering lives in the wasm-full / wasm-wasi
  runtime extension surface only.
- **C27B-022** `[FIXED after fJ review fix]`
  (wJ: `29a9ea3`, fJ review: `d2e5615`) — Native backend
  path traversal `..` reject 3-backend parity. JS / Native /
  Interpreter all converge on the same boundary judgement
  (project-root-internal `..` allowed, root-escape rejected)
  and emit the same canonical error string
  (`"Import path '<token>' resolves outside the project
  root. Path traversal beyond the project boundary is not
  allowed."`). 15 cases (5 × 3 backends) in
  `tests/c27b_022_path_traversal_parity.rs`; 4 of the
  C26B-015 cases also remain green. The fJ review tightened
  the assertion to a regex full-match
  (`assert_canonical_reject_strict`) and renamed the
  Case 3 helper to `boundary` → `direct_child`. Documented
  in `docs/reference/os_api.md §7` (Path traversal policy).
- **C27B-024** `[FIXED guard]` (wG: `55f3ad7`, fG empirical:
  `d3ba552`) — Interpreter closure capture bug regression
  guard. The actual fix landed in C26 (`73fd0a1`,
  Round 3 / wH); C27 wG added a 3-backend × 5-case parity
  guard in `tests/c27b_024_closure_capture_return.rs`
  (1-arg partial / 2-arg hole-in-first / nested closure /
  CondBranch arm body partial / Pack-field projection
  partial). The fG review reverted the 4-line guard at
  `eval.rs:820-821` to confirm 5/5 RED with the same
  HI-005-original symptom (`Cannot add <n> and @()`),
  proving the guard is still load-bearing.

### Round 2 (2026-04-25, parallel worktrees wE / wH / wI)

Round 2 scope reflects the Phase 0 Design Lock verdict
(wF dropped — Cluster 2 SEC fully closed in C26;
wG narrowed to closure capture; wJ added for path traversal):

- **wE (NET tls-h2)** — C27B-001 (h2 ≥10 cases + STABILITY §5.1
  flip) + C27B-003 (port-bind race Critical) + C27B-006 (retry
  shim retirement evidence pin) + C27B-027 NET portion.
- **wH (Runtime perf)** — C27B-010 (valgrind / RSS hard-fail +
  Coverage subset) + C27B-018 (native arena leak Option A,
  paired with C27B-028 silent-byte-corruption fix) +
  C27B-025 (Native stdout `setvbuf(_IOLBF)`) + C27B-026
  (snprintf truncation Step 3 Option B).
- **wI (Docs final amendment)** — this section + the
  `@c.27` GATE snapshot in `docs/STABILITY.md §5.6` +
  C27B-023 (`strOf` cold path docs) + C27B-027 docs portion +
  C27B-022 docs amendment portion. Round 1 review M-1
  (Stream parity historical context) and L-1 (README sweep
  regex generalise) folded in.

### Open at Round 2 boundary

- **C27B-001** (h2 ≥10 cases + STABILITY §5.1 flip) — wE.
- **C27B-003 (Critical)** (port-bind race recurrence on CI) — wE.
- **C27B-005** (24 h soak PASS record — user action) +
  agent-side proxy infra (014 / 015 / 017) FIXED.
- **C27B-006** (retry shim retirement evidence pin) — wE.
- **C27B-010 / C27B-018 / C27B-025 / C27B-026 / C27B-028** — wH.
- **C27B-012** (this rolling docs amendment, continues
  through GATE) — wI.
- **C27B-023** (`strOf` cold path mold) — see status note
  in this Round-1 wC entry; the `StrOf[span, raw]()` mold-form
  is already 3-backend parity-pinned (Interpreter / JS /
  Native via IR composition), and the function-form
  `strOf(span, raw)` extension is **not landed on `feat/c27`
  yet** — tracked under C27B-023 for Round 3 or D28
  re-evaluation per `.dev/C27_BLOCKERS.md`.

The Phase 14 GATE evidence template
(`.dev/C27_PROGRESS.md` Phase 14 GATE evidence template)
will be filled at GATE time with the merge-SHA / acceptance
log per row. `EXPECTED_TOTAL_LEN` baseline at the Round 2
boundary remains the C26 Round 10 / wε contractual value
(see `docs/STABILITY.md §5.5 / §5.6`); any C27 wH change
that touches Native runtime byte-length must update
`EXPECTED_TOTAL_LEN` / `F1_LEN` in lock-step.

---

## @c.26 (in progress — gen-C stable, second candidate)

**Fix-only RC cycle.** The label-less `@c.25` tag was **skipped**
(see `docs/STABILITY.md` §1.3); the gen-C stable tag is now being
pursued as `@c.26`. Intermediate tags are `@c.26.rcM`. No breaking
changes land here — everything breaking is deferred to the D27
generation (`.dev/D27_BLOCKERS.md`, formerly D26; the rename is
documented in `docs/STABILITY.md` §1.2).

The build-number rule is one-way: `@c.26.rcM` does **not**
auto-promote to `@c.26`. The stable tag is a separate build with
its own number.

### In-scope (fix-only)

All items below are **Must Fix** or **Critical** under the 2026-04-24
Phase 0 Design Lock (`DEFERRED` / `Should Fix` / `CONDITIONAL` /
`tentative` are retired). Live worklist: `.dev/C26_BLOCKERS.md`.

**Status legend**: `[FIXED]` = landed on `feat/c26`; otherwise = open
and owned by a later phase / worktree. A canonical snapshot lives at
`docs/STABILITY.md §5.6` and is re-synced on each RC tag.

#### Cluster 1 — NET stable viewpoint (Phase 1–6)

- **C26B-001** `[FIXED]` — HTTP/2 parity across 3-backend
  (interpreter / JS / native) reached the **10-case pin target** at
  Round 3 / wE. Round 1 landed `test_net6_c26b001_h2_post_body_*`;
  Round 2 Session 2 landed cases 2–4 (GET + query, status 404,
  64 KiB large body); Round 3 / wE added the three method
  variations (PUT / DELETE / PATCH via
  `c26b001_r3_h2_method_variation_test`). Pin count: 7 new
  `test_net6_c26b001_*` + 3 baseline h2 fixtures = 10. JS branch
  rejects with `H2Unsupported` in every case. The `§5.1 → FIXED`
  flip is held on the rest of Cluster 1 (C26B-002 TLS,
  C26B-006 retry shim), but the test-pin target is met.
- **C26B-002** `[FIXED during 2026-04-25 review]` — TLS
  construction matrix pinned across 3-backend. See the
  Round 11 wι review amendment below for the five new
  `test_net6_1c_c26b002_{1..5}_*` cases in `tests/parity.rs`
  (missing cert, key-only, plaintext fallback, invalid-PEM,
  unknown-protocol-token). Live cert-rotation + ALPN matrix
  coverage remains in the C26B-005 soak runbook because
  those branches are runtime-dependent.
- **C26B-003** `[FIXED]` — **Critical**. Port-bind race eradication
  (inherited from C25B-002). Root cause fixed; 100 consecutive local
  CI-equivalent runs pass with no retry-shim firing. Candidate
  solution 1 (`0.0.0.0:0` bind + `getsockname()`) sufficed; the
  D27-escalation checklist evaluated all-NO on the landed patch.
- **C26B-004** `[FIXED]` — Throughput regression gate promoted from
  `continue-on-error` to hard-fail on 10% regression against a
  30-sample baseline (Round 2 / wB).
- **C26B-005** — Scatter-gather 24-hour soak verification via a
  manual runbook. `.dev/C26_SOAK_RUNBOOK.md` is **landed**
  (2026-04-24, Round 2 / wA) with environment setup, tmux /
  `systemd-run` session layout, per-30s RSS / fd / thread monitor,
  valgrind (pre-soak 1 h) + heaptrack (3 × 1 h windows) leak
  gates, throughput drift boundary, backend-specific tolerances,
  and a REPORT.md template. The 24 h run itself is the stable-gate
  blocker and is still pending.
- **C26B-006** `[FIXED]` — HTTP parity retry shim retired at
  Round 4 / wJ (`c3805ff`). With C26B-003's root-cause fix in
  place, `tests/parity.rs` now binds to `0.0.0.0:0` and recovers
  the concrete port via `getsockname()` without any retry
  wrapper. No existing `parity.rs` assertion was rewritten — the
  shim was already effectively a no-op after C26B-003.
- **C26B-026** `[FIXED]` — Native h2 HPACK encode path now
  preserves custom response headers (Round 2 / wC, fix in
  `src/codegen/native_runtime/net_h1_h2.c::h2_extract_response_fields`).
  Discovered as a sub-finding of C26B-001 Session 2 when the
  multi-custom-header test dump showed that every
  `handler`-returned `set-cookie` / `x-*` header was dropped on
  the Native path. Root cause: the encoder treated
  `taida_list_get(hdrs_val, j)` as a raw pack and looked up
  `name` / `value` on the returned Lax wrapper instead of the
  inner pack. Fixed to mirror the h1 encode path; the header cap
  was raised to `H2_MAX_HEADERS = 128`. Regression pinned by
  `test_net6_c26b026_h2_multiple_custom_headers_3backend_parity`.
  `EXPECTED_TOTAL_LEN` resynced (982_976 → 983_593).

WASM targets are explicitly out of scope for gen-C NET (3-backend
fixed); the single exception is C26B-020 pillar 3 (a widening
addition under §6.2).

#### Cluster 2 — Security (Phase 7–8)

- **C26B-007** — SEC-002 through SEC-010 localised fixes;
  cargo-audit / cargo-deny promoted to hard-fail; cppcheck +
  clang-tidy integration for C21–C24 runtime C code. Includes
  sub-phase 7.4 **SEC-011**: Sigstore (cosign keyless) signing +
  SLSA provenance attestation wired into the `taida publish`
  workflow. Verify-on-install step added to `taida install`.
  - Sub-phase 7.1 `[FIXED]` — SEC-002 / 003 / 005 / 007 / 008 /
    009 / 010 localised fixes (see
    `.dev/taida-logs/docs/archive/SECURITY_AUDIT.md` once owner
    action lands). `.o` artefacts now land in `std::env::temp_dir()`
    so the source tree stays clean even when cleanup is skipped on
    panic (SEC-010).
  - Sub-phase 7.2 `[FIXED]` — `.github/workflows/security.yml` had
    `continue-on-error: true` stripped from the `cargo-audit` /
    `cargo-deny` jobs; hard-fail is now the default gate. The
    rationale for leaving `yanked` / `unmaintained` as warnings
    (the fastrand 2.4.0 transitive yank requires a dependency
    change that is scoped to a separate task) is pinned in
    `deny.toml`.
  - Sub-phase 7.3 `[FIXED]` — a `c-static-analysis` job was added
    to `.github/workflows/security.yml` that assembles the
    native-runtime C translation unit in `NATIVE_RUNTIME_C` order
    (`core → os → tls → net_h1_h2 → net_h3_quic`) and runs
    `gcc -Wall -Wextra -Wformat-security` + `cppcheck
    --enable=warning,style,performance,portability
    --error-exitcode=1`. The gcc warning baseline is pinned at
    `78`; any increase hard-fails. The allow / fix-queue policy
    lives at `.dev/C26_C_STATIC_ANALYSIS.md`.
  - Sub-phase 7.4 (SEC-011) `[FIXED]` — Sigstore cosign keyless
    signing + SLSA provenance attestation wired into the
    `taida publish` workflow, with a verify-on-install step for
    `taida install` (Round 2 / wB).
- **C26B-008** `[CLOSED — not required at @c.26 stable]` —
  Taida Lang has no confirmed install base as of `@c.26` cycle,
  so GHSA publication + CVE request has no notification target
  and would generate spurious disclosure noise. The underlying
  fix shipped in `@c.15.rc3`
  (`src/upgrade.rs::canonical_release_source_is_taida_lang_org`
  regression pin). Advisory scaffold staged at Round 6 / wR
  (`docs/advisory/` + `scripts/advisory/`) was removed at
  Round 8 / wX2 (`62fd54d`); the draft is recoverable from git
  history if an install base later emerges and the
  pre-`@c.15.rc3` window is confirmed exploitable against real
  users. Closure rationale: scope-out reclassification aligned
  with the DEFERRED 全廃方針 (Phase 0 Design Lock) — the fix
  exists, there is nothing to defer.

#### Cluster 3 — Parser quality (Phase 9)

- **C26B-009** `[FIXED]` — State-machine transition-graph
  formalisation for `parse_error_ceiling` / `parse_cond_branch`
  recovery paths landed at `.dev/C26_PARSER_FSM.md` (8 sections,
  stable node IDs `CondBranch:*` / `EC:*` / `SYNC:*`, E0301 /
  E0303 / C13B-010 / C12-4 cross-referenced). The `| _ |> <throw>`
  arm-body throw propagation bug was already fixed at C25
  commit `4696429`; regression pinned by
  `tests/c25b_032_arm_body_throw_propagation.rs`.
- **C26B-019** `[FIXED]` — Multi-line `TypeDef(field <= v, ...)`
  constructor parse works end-to-end; `parse_arg_list` now calls
  `skip_newlines()` at entry / after slot / after comma, and
  `parse_primary_expr` calls it before the `TypeInst` lookahead.
  The `taida check` vs `taida build` parser divergence turned out
  to be shared-parser lookahead insufficiency (not a second parser
  implementation) and is eliminated by the same fix. Widening per
  §6.2. Docs example addendum is owned by C26B-013.

#### Cluster 4 — Runtime perf / memory (Phase 10)

**Common-abstraction decision LOCKED** (Round 3 / wG, 2026-04-24):
all Phase 10 blockers (C26B-010 / 012 / 018 / 020 pillar 2 / 024)
adopt the **Arc + try_unwrap COW family**
(`.dev/C26_CLUSTER4_ABSTRACTION.md`, PROPOSED → LOCKED). Zero-copy
slice views are subsumed as a specialisation of the Arc family;
the arena option is D27-deferred. The lock itself landed with
zero code — it is a gating artefact so follow-up sessions land
3-backend simultaneously without breaking parity.

- **C26B-010** `[FIXED]` — Memory-leak CI gate. `.github/workflows/memory.yml`
  added at Round 4 / wM (`e444f81`): every-push valgrind smoke over
  `examples/quality/c26_mem_smoke/{hello,list,string}_smoke.td` plus a
  weekly heaptrack run, with reproduction helpers at `scripts/mem/`.
  Peak-RSS drift against this baseline is contractual for the
  `@c.26` gate; the 24 h soak (C26B-005) is orthogonal.
- **2026-04-25 review correction** — the "agent-side FULLY READY"
  wording is downgraded to **HOLD**. C26B-029 (CI false-green
  holes) + C26B-030 (SEC-011 install-side verify wiring) +
  C26B-002 (3-backend TLS construction pin) all FIXED during
  the review follow-up. **C26B-005** (24 h soak PASS record)
  remains the last REOPEN item and is a user-action blocker
  on the `@c.26` tag; see the Round 11 wι review amendment.
- **C26B-012** — `PENDING_BYTES` FIFO ordering (terminal addon
  concurrent `ReadEvent()`) + BuchiPack interior Arc migration.
  - BuchiPack `[FIXED]` at Round 6 / wQ (`6f72f7c`): `Value::BuchiPack`
    migrated to `Arc<Vec<(String, Value)>>`. `Value::clone()` on a pack
    is now an `Arc::clone()` (one atomic increment) vs field-by-field
    deep clone. New helpers: `Value::pack(Vec<(String, Value)>)` and
    `Value::pack_take(Arc<…>)` (try_unwrap fast path, clone fallback).
    Pattern-match arms such as `Value::BuchiPack(fields)` continue to
    yield `&Vec<(String, Value)>` via transparent `Arc` deref. Regression
    guard: `tests/c26b_012_buchipack_arc_ptr_eq.rs` pins `Arc::ptr_eq`
    invariants after `fields.clone()` / pack construction. Layout change
    is internal — `EXPECTED_TOTAL_LEN` unchanged; D27 escalation
    checklist: 3/3 NO.
  - PENDING_BYTES FIFO (terminal addon concurrent `ReadEvent()`) —
    **OPEN, user-side**. The FIFO lives in the `terminal` submodule
    at `.dev/official-package-repos/terminal/`, which the language
    agent is explicitly forbidden from touching; this half is owned
    by the downstream addon author. The BuchiPack Arc migration
    above removes the main interpreter-side deep-clone path on the
    event-batch return value, so the FIFO work can land downstream
    without re-visiting `Value` layout.
- **C26B-018** — `Str` primitive super-linear paths resolved via
  (A) char-index cache + (B) byte-level primitive + (C)
  `StringRepeatJoin` mold. Option (D) `StringBuilder` is explicitly
  **discarded** (conflicts with Taida's immutable-first philosophy
  — not deferred, not revisited at D27).
  - (B) + (C) `[FIXED]` at Round 4 / wK (`3e4c667`): byte-level
    primitive paths (`src/interpreter/mold_eval.rs` +
    `src/js/runtime/core.rs` + `src/codegen/native_runtime/core.c`)
    and the `StringRepeatJoin` mold lowered across 3-backend,
    pinned by `tests/c26b_018_byte_primitive.rs` and
    `tests/c26b_018_repeat_join.rs`.
  - (A) `Value::Str` Arc+COW foundation landed at Round 6 / wP
    (`6cf6648`): `Value::Str` migrated to `Arc<String>`, so
    `Value::clone()` on a string is now an `Arc::clone()` (atomic
    refcount bump) vs an O(len) copy. New helpers:
    `Value::str(String)` constructor and `Value::str_take(Arc<String>)`
    (try_unwrap fast path, fallback to `(*arc).clone()`). Regression
    guard: `tests/c26b_018_str_arc_ptr_eq.rs` pins `Arc::ptr_eq`
    after `value.clone()` and after pass-through assignment.
  - (A) char-index cache layer `[FIXED]` at Round 8 / wU
    (`9e69f96`): `Arc<String>` is wrapped in a new
    `StrValue { data: String, char_offsets: OnceLock<Vec<usize>> }`
    carrier so `Value::Str(Arc<StrValue>)` now carries a lazily-
    populated byte-offset table of length `char_count + 1`.
    `Deref<Target = String>` + full trait forwarding (`PartialEq`,
    `Eq`, `PartialOrd`, `Ord`, `Display`, `Hash`, `Default`,
    `AsRef<str>`, `AsRef<OsStr>`, `Borrow<str>`, `From<String>`,
    `From<&str>`) keep every existing byte-oriented call site working
    through autoderef — including the addon ABI surface
    (`s.as_ptr()` / `s.len()`). The hot super-linear paths are now
    O(1) per call after the first touch:
    `Slice[str]()` / `CharAt[str, idx]()` / `Str.length()` /
    `Str.get(idx)` / `Str.indexOf(sub)` / `.lastIndexOf(sub)`
    (the last two use binary search over the cache for a
    byte-offset → char-index mapping). Lock-free `OnceLock` matches
    Taida's immutable-first model; no mutable interior state
    escapes. `idx.saturating_add(1)` defends the negative-index
    Lax cast (`-1i64 as usize == usize::MAX`) so the `CharAt`
    out-of-bounds path continues to return `None`. 13 unit tests
    in `src/interpreter/value.rs::tests` pin ASCII + UTF-8 char
    counting (`aあ🙂b` = 9 B / 4 chars), `cached_char_at` /
    `cached_char_slice` / `cached_byte_to_char_index` round-trip,
    `Arc` sharing of the cache across clones, and the
    `Value::str_take` unique + shared paths. Option (D)
    `StringBuilder` remains **discarded** (not deferred).
- **C26B-020** — **Downstream-blocking hard blocker.** Three
  pillars, all required for DONE:
  1. `[FIXED]` `readBytesAt(path, offset, len) -> Bytes` API;
     `readBytes` gets a runtime-configurable 64 MB ceiling. Landed
     across 3-backend; scale test pins 1 GB file × 64 × 16 MB
     chunked read in under 2 s.
  2. `[FIXED]` `BytesCursorTake` zero-copy via `Arc<Vec<u8>>` +
     offset/len view. Landed at Round 5 / wO (`f15c145`):
     `Value::Bytes` migrated to `Arc<Vec<u8>>`;
     `parse_bytes_cursor` returns `(Arc<Vec<u8>>, usize)`; each
     `BytesCursorTake(size)` is now an `Arc::clone` (O(1)
     refcount bump) rather than a full-buffer memcpy. New
     helpers: `Value::bytes(Vec<u8>)` constructor and
     `Value::bytes_take(Arc<Vec<u8>>)` (try_unwrap fast path,
     clone fallback). Regression guards in
     `tests/c26b_020_bytes_cursor_zero_copy.rs`: Arc::ptr_eq
     invariant + 256 MB × 16 < 500 ms baseline +
     (`TAIDA_BIG_BYTES=1`) 1 GB × 64 < 2 s acceptance.
     `EXPECTED_TOTAL_LEN` is unchanged (Value layout is
     internal). D27 escalation checklist: 3/3 NO.
  3. `[FIXED]` `readBytesAt` + related molds lowered for
     `wasm-wasi` / `wasm-full` at Round 3 / wI via a new
     `src/codegen/runtime_wasi_io.c` (WASI preview1 `path_open` +
     `fd_read`, 64 MB runtime-configurable ceiling preserved).
     Regression guard: `tests/c26b_020_wasm_bytes_at.rs`. This is
     the **only** wasm-scope addition in gen-C NET work.

  All three pillars are now **FIXED**. The downstream `bonsai-wasm`
  Phase 6 unblock is material; the end-to-end acceptance smoke
  still runs against the stable gate.
- **C26B-024** `[FIXED]` — Native list / `BuchiPack` clone-heavy
  paths. **Step 2 + Step 3 Option A first pass** landed at Round 8
  / wT (`81c4fc1`): a bounded per-thread freelist for 4-field Packs
  (`taida_pack4_freelist_pop` / `taida_pack4_freelist_push` in
  `src/codegen/native_runtime/core.c`, `__thread` storage, 32-entry
  cap, full-cap fallback to `free()`). The profile-identified hot
  path was `taida_lax_new` allocating a 4-field Pack (112 B) on
  every `list.get(i) ]=> x`; allocator thrash dominated wall time
  at `bench_router.td` N=1000 / M=5000 (sys/real = 80%). Freelist
  dispatch in `taida_pack_new` / `taida_release` re-initialises
  every slot on reuse, so no stale child leaks; bounded per-thread
  storage prevents unbounded RSS growth; cross-thread release falls
  through to `free()` (correct but without reuse win). Internal
  micro-bench (Lax churn 200k wrappers, 3-run median):
  `baseline real 0.510s / sys 0.412s / user 0.094s` →
  `freelist real 0.295s / sys 0.240s / user 0.056s` — delta
  `-42% / -42% / -38%`. Five 3-backend parity tests
  (`tests/c26b_024_pack_freelist_parity.rs`: `lax_churn_int` /
  `lax_churn_str` / `lax_oob_empty` / `freelist_bound` /
  `mixed_type_lax`) exercise the optimised access pattern, the OOB
  empty-Lax path, and 40-depth recursion that exceeds the
  freelist cap. **Step 4 full acceptance** landed at Round 10 /
  wε (`78a70f4` / merge `baff13d`), closing the Native ≤ JS × 2
  hard-gate with the Native router-bench now at sys/real = 9 %
  (down from 81 %) and Native / JS = 2.0× (down from 12.1×).
  Measured on `examples/quality/c26_native_router_bench/router.td`
  at N=200 routes × M=500 iterations (3-backend parity, Linux
  x86_64, gcc):
  ```
  | Backend | real  | sys   | sys/real | vs JS |
  |---------|------:|------:|---------:|------:|
  | Native  | 0.34s | 0.03s | 9 %      | 2.0×  |
  | JS      | 0.17s | 0.01s | 6 %      | 1.0×  |
  ```
  (baseline `@c.25.rc7` / wT Round 8: Native 2.05 s / sys 1.66 s /
  12.1× JS / 81 % sys). The six-change runtime refactor lives in
  `src/codegen/native_runtime/core.c` F1 and is superset-additive
  over wT:
  (1) thread-local bump arena (`TAIDA_ARENA_*`, 2 MiB chunks,
  per-thread chain, 16 B aligned; allocations ≤ 1024 B take the
  arena path, larger fall back to `malloc`; arena chunks reclaimed
  at process exit — the Native codegen does not emit `taida_release`
  for short-lived bindings, so the bump allocator is semantically
  equivalent to `malloc` for this workload). Router-bench `malloc`
  calls 2.97 M → ≈ 300 (**-99.99 %**).
  (2) Tier-1 thread-local freelists for the residual `taida_release`
  paths: cap=16 List freelist (`taida_list_freelist_{pop,push}`) +
  3-bucket small-string freelist (≤ 32 B / ≤ 64 B / ≤ 128 B total
  block size, `taida_str_bucket_for` / `taida_str_freelist_{pop,
  push}`), hooked from `taida_list_new` / `taida_str_alloc` (alloc
  side) and `taida_release` / `taida_str_release` (release side).
  Arena-backed slabs bypass the freelist since `free()` on an arena
  pointer is undefined behaviour.
  (3) Heap-range tracker — `taida_safe_malloc` updates
  `[taida_heap_min, taida_heap_max)` on every `malloc`;
  `ptr_is_readable` fast-paths via O(1) range membership instead of
  syscalling `mincore`.
  (4) 64-entry `mincore`-page cache (`taida_mincore_cache`) for
  pages that have already been verified as mapped; subsequent probes
  on the same page skip the syscall. Wired into
  `taida_ptr_is_readable` / `taida_is_string_value` /
  `taida_read_cstr_len_safe`. Router-bench `mincore` syscalls
  9.45 M → 20 (**-99.9998 %**).
  (5) Arena-aware `list_push` migration: `realloc()` on an arena-
  backed slab is UB, so when an arena-origin list grows past cap=16
  we `malloc` the new capacity and `memcpy` header + elements; the
  abandoned arena slot is reclaimed at process exit.
  (6) Arena-skip guards in `taida_release` / `taida_str_release`
  so `free()` is never called on an arena pointer.
  Three new Round 10 parity tests
  (`tests/c26b_024_router_bench_parity.rs`:
  `router_bench_smoke_parity` / `list_push_arena_migration_parity`
  / `small_string_churn_parity`) confirm bit-for-bit 3-backend
  output under the workload that triggers arena allocation, list
  push migration, and small-string churn; the five Round 8 wT
  pack4 freelist tests remain green (arena-skip guard inserted).
  The Round 8 / wT `EXPECTED_TOTAL_LEN` 998,598 → Round 10 / wε
  **1,012,971** (F1 251,878 → **266,252**; F2 / other fragments /
  interp / JS / wasm profiles all unchanged — F1 absorbs the full
  arena / freelist / heap-range / mincore-cache / fast-path /
  migration / guard delta). D27 escalation checklist: 3/3 NO —
  public mold surface untouched, error contract unchanged, all 185
  parity tests + 880+ total tests green (including C25 / C24 / C23
  / C21 regression guards and all four wasm profiles; wasm profiles
  are untouched by Round 10). Step 1 (CI perf regression gate
  wiring against `bench_router.td`) is additive infrastructure
  tracked separately under C26B-024 on `.dev/C26_BLOCKERS.md`;
  the stable-gate acceptance numbers above are the contractual
  baseline it will enforce once wired.

#### Cluster 5 — Float parity (Phase 11)

- **C26B-011** `[FIXED]` — NaN / ±Infinity / denormal parity across
  3-backend (`Div` / `Mod` / math molds), `Div[1.0, 0.0]()` Lax
  default rendering divergence resolved, and `Mul[1.5, 2.0]()`
  native-build exit-code-1 isolated and fixed.
  - Signed-zero `-0.0` runtime handling `[FIXED]` at Round 6 / wS
    (`547972c`) across interpreter and native — `taida_float_to_str`
    dispatches on `signbit` so `-0.0` renders as `"-0.0"` rather than
    `"0.0"`.
  - JS-codegen Float-literal rendering of `-0.0` `[FIXED]` at
    Round 7 / wV-a (`d00e896`) — the final 3-backend divergence on
    C26B-011 is now closed. Two fixes in
    `src/js/runtime/core.rs`: (a) `__taida_float_render` probes
    `Object.is(v, -0)` before `toFixed(1)` so signed zero surfaces
    as `"-0.0"` (pure `(-0).toFixed(1)` drops the sign);
    (b) `__taida_mul` stays on the Number multiplication path
    when either operand is `-0` or when the Number-path product
    itself is `-0` (BigInt has no `-0`, so the integer fast-path
    would otherwise collapse the sign bit on `-1.0 * 0.0`).
    `src/js/codegen.rs::Expr::FloatLit` adds defensive handling for
    `-0.0` / NaN / ±Infinity literals (Rust `f64::to_string` emits
    `"inf"` / `"-inf"` which are invalid JS `Number` literals; the
    defensive path emits `(1/0)` / `(-1/0)` / `(0/0)` so the JS
    backend remains safe for any float literal the interpreter
    accepts). Regression guard:
    `tests/c26b_011_signed_zero_parity.rs` (3-backend).

#### Cluster 6 — Surface fixes (Phase 12) & docs (Phase 13)

- **C26B-013** — Stability / CHANGELOG / `net_api.md` (new) /
  Stream-lowering doc updates / 2nd-party inbox rule
  (`.dev/C26_PROGRESS.md` § NEW-E).
- **C26B-014** `[FIXED]` — Core-bundled packages (`taida-lang/os`,
  `net`, `crypto`, `pool`, `js`) resolvable without an explicit
  `packages.tdm` entry. **Option B pinned** (implementation brought
  in line with docs — widening, not breaking). The interpreter
  resolver now calls `CoreBundledProvider::materialize_core_bundled`
  on `resolve_package_module` failure; the native backend's existing
  `is_core_bundled_path` branch (C25B-030) covers the codegen side
  with no changes. Regression guard:
  `tests/c26b_014_core_bundled_importless.rs` (interpreter × 4 +
  native × 2).
- **C26B-015** `[FIXED]` — Native-backend path-traversal check no
  longer rejects project-root-internal `..` imports; parity across
  3-backend (root-escape is still rejected via the canonicalized
  component walk).
- **C26B-016** `[FIXED]` — `httpServe` handler `req` pack shape
  pinned in `docs/reference/net_api.md` (new). **Option B+
  complete**: the zero-copy span pack (`make_span`,
  `src/interpreter/net_eval/helpers.rs:195-200`) is **retained**
  for perf; ergonomics is widened via the full public mold family
  — `SpanEquals` / `SpanStartsWith` / `SpanContains` / `SpanSlice`
  landed at Round 2 / wD, and the cold-path materialiser
  `StrOf(span, raw) -> Str` landed at Round 3 / wH as pure IR
  composition (`src/codegen/lower_molds.rs::StrOf`,
  `taida_pack_get` + `taida_slice_mold` + `taida_utf8_decode_mold`
  + `taida_lax_get_or_default`, no new C runtime helpers).
  Regression guard: `tests/c26b_016_strof_parity.rs` (3-backend).
  Option A (auto-`Str` promotion of `req.method`) would break
  `tests/parity.rs` fixtures and is **deferred to D27**.
- **C26B-017** `[FIXED]` — Interpreter: partially-applied function
  returned from an outer function no longer collapses to `@()`;
  closure capture works across return boundaries (3-backend parity:
  `makeAdder(10)(7) == 17`). Landed at Round 3 / wH. Regression
  guard: `tests/c26b_017_partial_app_closure_capture.rs`.
- **C26B-021** `[FIXED]` — `stdout` / `stderr` now line-buffered at
  the C entry point via `setvbuf(_IOLBF, 0)`. **Option B pinned**
  (per-call `fflush` is not adopted, because the per-call overhead
  is higher than the one-shot entry-point setup).
- **C26B-022** — HTTP wire parser enforces method 16 / path 2048 /
  authority 256 byte ceilings; over-limit requests emit `400 Bad
  Request`. **Step 3 Option B pinned**. `-Wformat-truncation` is
  warning-as-error in CI.
  - Step 2 (Interpreter h1 method + path) `[FIXED]` at Round 3 /
    wE (`src/interpreter/net_eval/h1.rs`, constants
    `HTTP_WIRE_MAX_METHOD_LEN = 16` / `HTTP_WIRE_MAX_PATH_LEN =
    2048`). The check runs after `parse_request_head` and before
    `dispatch_request`, so over-limit inputs are rejected before
    the handler is invoked. Additive widening per §6.2; no
    existing assertion is altered.
  - Authority (256) enforcement `[FIXED]` at Round 4 / wJ
    (`c3805ff`) across h1 / h2 / h3
    (`src/interpreter/net_eval/h1.rs`, `h2.rs`, `h3.rs`);
    over-limit authorities return `400 Bad Request` symmetrically
    with the method / path ceilings.
  - **OPEN** residual: `-Wformat-truncation` promotion to
    warning-as-error in CI.
- **C26B-023** `[FIXED, docs-path]` — 2-arg `httpServe` handler
  `req.body` empty-span caveat documented in
  `docs/reference/net_api.md` (§3.2 / §8) at Round 3 / wH,
  including the `readBody(req)` / `readBodyChunk(req)` /
  `readBodyAll(req)` usage matrix, the `__body_stream` sentinel
  design note, and the silent-breakage anti-pattern. Regression
  guard: `tests/c26b_023_two_arg_handler_body.rs`. The runtime
  warning emission for direct `req.body` slice in 2-arg handlers
  is part of the diagnostic-code track and remains OPEN.
- **C26B-025** `[FIXED]` — `taida publish` validates `packages.tdm`
  self-identity (`<<<@version` vs `next_version`) at `plan_publish()`
  and rejects a stale manifest before any tag is pushed. `--retag`,
  `--force-version`, and `--label` all pass through the same check;
  label addenda (manifest `a.7` + `--label rc3` → tag `a.7.rc3`) are
  accepted as a legitimate match. The optional `--bump-manifest`
  auto-rewrite is deferred to D27 (listed under
  `.dev/D27_BLOCKERS.md`); the C26 cut rejects the stale case.
  Regression guards:
  `tests/c26b_025_publish_rejects_stale_manifest_self_identity.rs` +
  `tests/c26b_025_publish_accepts_label_addendum.rs`.

#### Cluster 7 — Stable GATE (Phase 14)

- User-approved `@c.26` tag (agent must never cut `@c.26.rcM` /
  `@c.26` tags). Promotion gate requires:
  - All Critical / Must Fix closed.
  - `cargo test --release` all-pass, 0 red, 0 SLOW warnings.
  - 3-backend parity across all fixtures.
  - CI 2C wall-clock median ≤ 8 minutes.
  - Parallelism efficiency ≥ 80%.
  - 24-hour soak test PASS (C26B-005).
  - Downstream `bonsai-wasm` Phase 6 smoke (C26B-020 acceptance).
  - `SECURITY_AUDIT.md` open = 0; SEC-011 recorded complete.
  - Sigstore + SLSA-signed official addon release.

### Round 1 – Round 10 — commits already on `feat/c26`

Round 1 merge order: P3 (C26B-003) → P10 pillar 1 (C26B-020) →
P11 (C26B-011) → P12 (C26B-014 / 015 / 021 / 025), then Phase 7
sub-phases 7.1–7.3 (C26B-007) and Phase 9 (C26B-009 / 019) in
parallel sessions.

Round 2 additions: wA (soak runbook + docs amendment), wB
(C26B-004 perf gate hard-fail + SEC-011 Sigstore / SLSA), wC
(C26B-026 Native h2 HPACK custom-header fix), wD (C26B-016 Option
B+ span-aware molds: `SpanEquals` / `SpanStartsWith` /
`SpanContains` / `SpanSlice`).

Round 3 additions: wE (C26B-001 h2 method PUT / DELETE / PATCH
cases → 10-case pin target + C26B-022 Step 2 interp wire limits),
wG (Cluster 4 common-abstraction LOCK decision, decide-only),
wH (C26B-016 `StrOf` + C26B-017 partial-app closure capture +
C26B-023 2-arg body docs), wI (C26B-020 pillar 3 wasm-wasi /
wasm-full lowering).

Round 4 additions: wJ (`c3805ff`, C26B-006 retry-shim removal +
C26B-002 TLS observability surface tranche + C26B-022 authority
byte ceiling across h1 / h2 / h3), wK (`3e4c667`, C26B-018 (B)
byte-level primitive paths + (C) `StringRepeatJoin` mold
3-backend), wM (`e444f81`, C26B-010 memory-leak CI gate —
valgrind smoke on every push + weekly heaptrack, plus
`scripts/mem/` helpers), wN (`a146b76`, docs-only re-sync of
STABILITY / CHANGELOG / `net_api.md` to Round 3 FIXED set).

Round 5 additions: wO (`853900f`, C26B-020 pillar 2 —
`Value::Bytes` migrated to `Arc<Vec<u8>>` with
`parse_bytes_cursor` returning `(Arc<Vec<u8>>, usize)` so every
`BytesCursorTake(size)` becomes an `Arc::clone`; acceptance
scales to 1 GB × 64 < 2 s under `TAIDA_BIG_BYTES=1`).

Round 6 additions (all merged on `feat/c26`):

- wR (`30c7283`, `a146b76` cascade) — amends STABILITY §5.5 / §5.6
  and the CHANGELOG `@c.26` section to re-sync the FIXED set
  through Round 5, and originally staged the C26B-008 GHSA
  advisory template under `docs/advisory/`. The advisory scaffold
  (`docs/advisory/` + `scripts/advisory/`) was subsequently
  removed at Round 8 / wX2 (`62fd54d`) when C26B-008 was closed
  as not required given the zero install base at `@c.26`
  declaration; see the C26B-008 entry above for the closure
  rationale.
- wP (`6cf6648`, C26B-018 (A) foundation) — `Value::Str` migrated
  to `Arc<String>`. Pattern arms `Value::Str(s)` now yield `s:
  Arc<String>` which derefs transparently. `Value::clone()` on a
  string is an `Arc::clone`. `Value::str()` / `Value::str_take()`
  helpers added; all call sites updated. The char-index cache layer
  was tracked as a wU-class follow-up at the time of this commit;
  it subsequently landed at Round 8 / wU (`9e69f96`) — see the
  Round 8 additions below and the C26B-018 (A) entry above.
- wQ (`6f72f7c`, C26B-012 BuchiPack migration) — `Value::BuchiPack`
  migrated to `Arc<Vec<(String, Value)>>`. Pattern arms
  `Value::BuchiPack(fields)` now yield `fields:
  Arc<Vec<(String, Value)>>`. New helpers `Value::pack()` /
  `Value::pack_take()`. Write paths use `Arc::make_mut` or
  `Arc::try_unwrap` COW. PENDING_BYTES FIFO remains OPEN, tracked
  separately.
- wS (`547972c`, C26B-022 Native authority + C26B-011 signed-zero
  runtime + profiling defer) — lands the Native h2/h3 authority
  byte-ceiling fixture, the runtime path for signed-zero `-0.0`
  across interpreter and native (JS codegen rendering was PARTIAL
  at this point; completed at Round 7 / wV-a, see below), and
  defers the C26B-024 Native list/pack profile pass onto the new
  Arc baseline.

Parallel worktrees wP / wQ / wS operated on disjoint file sets
(`Value` variants + isolated fixtures); the Round 6 `EXPECTED_TOTAL_LEN`
rolled from 988,932 to 994,500 bytes driven by wS Native additions
(wP / wQ are Rust-only, no C-fragment delta).

Round 7 additions (all merged on `feat/c26`):

- wV-a (`d00e896`, C26B-011 JS signed-zero codegen completion) —
  closes the last 3-backend divergence on C26B-011. JS-side
  `__taida_float_render` now probes `Object.is(v, -0)` before
  `toFixed(1)` so signed zero renders as `"-0.0"`; `__taida_mul`
  stays on the Number multiplication path when either operand
  or the Number-path product is `-0` (the BigInt integer
  fast-path has no `-0` and would otherwise collapse the sign).
  `Expr::FloatLit` JS codegen adds a defensive `-0.0` / NaN /
  ±Infinity literal emitter (`(1/0)` / `(-1/0)` / `(0/0)`) so
  Rust `f64::to_string`'s `"inf"` / `"-inf"` tokens — which are
  invalid JS `Number` literals — are never surfaced in generated
  JS. Regression guard: `tests/c26b_011_signed_zero_parity.rs`.
- wW (`feb29f4`, C26B-013 docs amendment for Round 6) — promotes
  wP / wQ from OPEN to FIXED in STABILITY §5.6 and CHANGELOG
  `@c.26`, and records wS signed-zero runtime + the then-pending
  JS follow-up. Superseded by wZ (Round 8) for the JS-side FIXED
  promotion; wW's text is a faithful snapshot of the Round 6
  merged state at the time of commit.
- wX (`eba5200`, Rust 1.93 clippy + fmt sweep) — folds two
  pre-existing `collapsible_if` nests flagged by Rust 1.93's
  stricter lint (`src/pkg/provider.rs:250` and
  `src/pkg/publish.rs:463`) into `let-chain` form, and applies
  `cargo fmt` across four files with incidental formatting drift.
  No behaviour change; `-D warnings` stays green on the updated
  toolchain.

Round 7 worktrees wV-a / wW / wX are disjoint
(`src/js/runtime/` + `src/js/codegen.rs` / docs-only /
`src/pkg/` + formatter-only); the Round 7 `EXPECTED_TOTAL_LEN`
is unchanged from Round 6 (no C-fragment delta).

Round 8 additions (all merged on `feat/c26`):

- wY (`af5c443`, test-doc clippy cleanup ahead of `@c.26` GATE)
  — zero behaviour change. Addresses three pre-existing lint
  categories confined to newly-added C26 test files:
  `doc_list_item_overindented` (rustdoc, 3-space continuation
  flattened to 2-space hanging indent in
  `c26b_011` / `c26b_016` / `c26b_018` module docs);
  `ptr_arg` (clippy, `&PathBuf` → `&Path` in `run_js` /
  `run_native` / `build_native` test helpers where the buffer
  is never mutated); `zombie_processes` (clippy false-positive
  `#[allow]` on `spawn_and_wait_ready` in
  `c26b_022_native_authority.rs` — every caller pairs spawn
  with `drain_and_cleanup` which kills + `wait_with_output`s,
  but the pattern is split across helpers so the lint can't see
  the pairing). Test scope only — no `src/` changes, no
  `EXPECTED_TOTAL_LEN` impact, no parity fixture altered, no
  new assertion added or modified. D27 escalation checklist:
  3/3 NO.
- wZ (`ba720d3`, C26B-013 rolling docs amendment — Round 6 + 7
  catch-up) — promotes C26B-011 to full `[FIXED]` in the
  Cluster 5 entry above (the JS signed-zero path landed at
  wV-a), adds Round 7 and Round 8 sections to this changelog,
  and re-syncs STABILITY §5.6 and §5.5 to match. No code
  change. wZ was authored before wT / wU / wX2 committed, so
  its Round 8 subsection only pre-announced wY + itself; the
  wδ rolling amendment below extends that narrative to cover
  the remaining Round 8 landings without contradiction
  (superset extension only).
- wT (`81c4fc1`, C26B-024 Step 2 profiling + Step 3 Option A
  first pass — thread-local 4-field Pack freelist) — the hottest
  path in the Native runtime (`taida_lax_new` on every
  `list.get(i) ]=> x`, 112 B Pack alloc) is fronted by a bounded
  per-thread freelist (32 entries, `__thread` storage, falls
  through to `free()` on cross-thread release or cap overflow).
  Slot re-initialisation on reuse keeps the Lax wrapper
  invariants intact. Native Lax-churn micro-bench shows
  `sys/real -42%` on a 200k-wrapper workload (baseline real
  0.510s / sys 0.412s → freelist real 0.295s / sys 0.240s).
  Five new 3-backend parity tests in
  `tests/c26b_024_pack_freelist_parity.rs` are all GREEN. Step 4
  (`bench_router.td` hard-gate acceptance) + Step 1 (CI perf
  gate wiring) were tracked separately at the time of the wT
  commit — C26B-024 was marked **PARTIAL FIXED** in
  `.dev/C26_BLOCKERS.md` *at this Round 8 snapshot*. Step 4 was
  subsequently closed at Round 10 / wε (`78a70f4` / merge
  `baff13d`) and the blocker is now FIXED; see the C26B-024
  FIXED entry above and the Round 10 additions section below.
  The Native-side `core.c` growth at this Round 8 / wT landing
  drives `EXPECTED_TOTAL_LEN` 994,500 → 998,598
  (`F1_LEN` 247,780 → 251,878; F2 / other fragments / interp /
  JS / wasm profiles all unchanged at this point — the Round 10
  / wε follow-up rolls both numbers to 1,012,971 / 266,252).
  D27 escalation checklist: 3/3 NO — mold signatures, pinned
  error strings, and existing assertions are all untouched; five
  new parity tests are additive.
- wU (`9e69f96`, C26B-018 (A) char-index cache layer) — closes
  the last sub-task of the Cluster 4 Str super-linear fix and
  promotes C26B-018 (A) from OPEN to **FIXED**. `Value::Str`
  now carries
  `Arc<StrValue { data: String, char_offsets: OnceLock<Vec<usize>> }>`;
  the lazy offset table gives O(1) `Slice` / `CharAt` /
  `Str.length()` / `Str.get(idx)` after first touch, and
  binary search over the cache gives O(log n)
  `Str.indexOf` / `.lastIndexOf` by mapping byte offsets back
  to char indices. Deref + full trait forwarding
  (`PartialEq` / `Eq` / `PartialOrd` / `Ord` / `Display` /
  `Hash` / `Default` / `AsRef` / `Borrow` / `From`) preserves
  every byte-oriented call site through autoderef, including
  the addon ABI (`s.as_ptr()` / `s.len()`). Lock-free `OnceLock`
  matches the immutable-first model; the negative-index Lax
  cast is guarded by `idx.saturating_add(1)`. 13 unit tests in
  `src/interpreter/value.rs::tests` pin ASCII + UTF-8 char
  counting, cache round-trip, `Arc` sharing across clones, and
  the `Value::str_take` unique + shared paths. D27 escalation
  checklist: 3/3 NO — `Value::Str(Arc<StrValue>)` is an internal
  layout change, `StrValue` Deref is transparent, and no mold
  signature / pinned error string / existing assertion is
  altered.
- wX2 (`62fd54d`, C26B-008 CLOSED — advisory not required, zero
  install base) — reclassifies C26B-008 as out-of-scope for
  `@c.26` stable because Taida Lang has no confirmed install
  base as of the cycle; GHSA + CVE publication has no
  notification target and would generate spurious disclosure
  noise. Underlying fix shipped in `@c.15.rc3`
  (`canonical_release_source_is_taida_lang_org` regression
  pin). The advisory scaffold staged at Round 6 / wR
  (`docs/advisory/C25B-014-advisory.md` +
  `docs/advisory/README.md` + `scripts/advisory/publish-advisory.sh`)
  is removed here; the draft is recoverable from git history
  if an install base later emerges and the pre-`@c.15.rc3`
  window is confirmed exploitable against real users
  (re-open conditions pinned in
  `.dev/C26_BLOCKERS.md::C26B-008`). Rewritten pointer files:
  `.github/SECURITY.md`, `CHANGELOG.md`, `docs/STABILITY.md`
  §5.6. **This closure is a scope-out reclassification, not a
  DEFER** — it is aligned with the DEFERRED 全廃方針 (Phase 0
  Design Lock): either FIX now or escalate to D27, and a
  scope-out for which there is no fix to defer is neither.

### New blockers opened during Round 8 (both OPEN, fresh tracks)

The Round 8 landings surfaced two pre-existing test failures
that are orthogonal to wT / wU / wX2 and were already failing
on `af5c443` (wY baseline):

- **C26B-027** `[OPEN]` — `c25b_008_doc_examples_parse` parse
  regression at `docs/reference/net_api.md:258` / `:273`
  (introduced by Round 4 / wN net_api.md edit). Blocks the
  @c.26 GATE `cargo test --release` all-pass audit. Docs-only
  fix path (no `src/` change anticipated).
- **C26B-028** `[OPEN]` — `init_release_workflow_symmetry::test_jobs_match_core_contract`
  asymmetry: the test expects 4 jobs but `release.yml` grew
  to 6 after SEC-011 Sigstore + SLSA landed in `6a3189f`
  (Round 2 / wB). Either the test fixture or the canonical
  contract list needs to absorb the two new jobs. Blocks
  the @c.26 GATE workflow-symmetry audit.

Both are tracked under C26B-027 / C26B-028 in
`.dev/C26_BLOCKERS.md`; neither is introduced by wT / wU /
wX2, so Round 8 proceeds without blocking on them.

### wδ rolling amendment (Round 9, earlier in this CHANGELOG iteration)

wδ (C26B-013 rolling docs amendment — Round 8 merge narrative)
extends wZ's Round 8 subsection to cover wT + wU + wX2, flips
the C26B-018 (A) status above to its merged FIXED state (and
previously flipped C26B-024 to PARTIAL FIXED — the wθ rolling
amendment below supersedes that to FIXED), and records the
`EXPECTED_TOTAL_LEN` 994,500 → 998,598 delta driven by wT.
C26B-027 / C26B-028 were opened as new OPEN tracks so the
`@c.26` GATE audit trail stays complete. No code change; the
wZ narrative is extended monotonically — no prior text is
contradicted. D27 escalation checklist: 3/3 NO.

### Round 9 additions (all merged on `feat/c26`)

- **Round 9 / wα** (`7fd1500` fix → `e3aacd9` merge,
  **C26B-027 `[FIXED]`**) — `c25b_008_doc_examples_parse` now
  passes. The Round 4 / wN `net_api.md` edit had introduced two
  code fences at `docs/reference/net_api.md:258` / `:273` that
  referenced an undefined identifier; the fixtures were updated
  to the intended form so the parse regression baseline is
  restored. Docs-only path as anticipated; no `src/` change.
  D27 escalation checklist: 3/3 NO — no mold signature / error
  string / parity fixture touched.
- **Round 9 / wβ** (`8fe8d49` fix → `83b5f8a` merge,
  **C26B-028 `[FIXED]`**) — release-workflow symmetry restored.
  The canonical-contract list in
  `tests/init_release_workflow_symmetry.rs::test_jobs_match_core_contract`
  now absorbs the two SEC-011 jobs that landed in `6a3189f`
  (Round 2 / wB, Sigstore cosign + SLSA provenance), and the
  **SEC-011 invariants are pinned** into the test: the `sign`
  job depends on `build-release`, the `provenance` job depends
  on `sign`, the verify-on-install step is present, and the
  keyless-signing OIDC audience matches the `taida publish`
  workflow. Any future release-workflow edit that breaks one of
  those invariants now hard-fails the symmetry test rather than
  silently drifting. D27 escalation checklist: 3/3 NO — the
  SEC-011 surface was landed by Round 2 / wB; wβ adds the
  regression guard only.
- **Round 9 / wδ** (`e3cefc0` docs → `46210b5` merge,
  C26B-013 rolling docs amendment) — the wδ section above;
  docs-only catch-up for the Round 8 merge order.

Round 9 closes the two blockers (C26B-027 / C26B-028) that
Round 8 had opened. Round 9 `EXPECTED_TOTAL_LEN` is unchanged
from Round 8 (998,598 B) — docs / tests only, no native-runtime
C delta.

### Round 10 additions (all merged on `feat/c26`)

- **Round 10 / wε** (`78a70f4` perf → `baff13d` merge,
  **C26B-024 Step 4 full acceptance**) — promotes C26B-024 from
  `[PARTIAL FIXED]` to `[FIXED]`. The `bench_router.td` hard-gate
  `Native ≤ JS × 2` with `sys/real ≤ 30 %` is now met
  contractually: at N=200 × M=500 the Native path clocks
  real 0.34 s / sys 0.03 s / sys-ratio 9 % / 2.0× JS (down from
  real 2.05 s / sys 1.66 s / 81 % / 12.1× JS at the Round 8 wT
  baseline). The six-change runtime refactor (thread-local bump
  arena, tier-1 List + small-string freelists, heap-range
  tracker, 64-entry `mincore`-page cache, arena-aware `list_push`
  migration, arena-skip guards in `taida_release` /
  `taida_str_release`) drives `malloc` calls 2.97 M → ≈ 300
  (**-99.99 %**) and `mincore` syscalls 9.45 M → 20
  (**-99.9998 %**) on the same workload. `EXPECTED_TOTAL_LEN`
  998,598 → **1,012,971** (F1 251,878 → **266,252**); F2 / other
  fragments / interpreter / JS / all four wasm profiles are
  unchanged (F1 absorbs the full delta). Three new 3-backend
  parity tests in `tests/c26b_024_router_bench_parity.rs`
  (`router_bench_smoke_parity` / `list_push_arena_migration_parity`
  / `small_string_churn_parity`) pin bit-for-bit output across
  Interpreter / JS / Native under arena allocation, `list_push`
  arena-to-malloc migration, and small-string freelist churn.
  The five Round 8 / wT pack4 freelist tests stay green (the
  arena-skip guard preserves their invariants). D27 escalation
  checklist: 3/3 NO — public mold surface untouched, error
  contract unchanged, all 185 parity tests + 880+ total tests
  green (C25 / C24 / C23 / C21 regression guards and all four
  wasm profiles included; wasm profiles are untouched by
  Round 10). See the C26B-024 entry above for the full technical
  narrative.

Round 10 `EXPECTED_TOTAL_LEN` rolls from 998,598 B (Round 8
wT) to **1,012,971 B**; F1 absorbs the full +14,373-byte delta.
No other fragment, backend, or wasm profile is touched.

### Round 11 additions (all merged on `feat/c26`)

- **Round 11 / wη** (`4bc7369` ci → `5e03422` merge, **C26B-024
  Step 1 CI perf regression gate wiring**) — closes the final
  open Step of C26B-024. `.github/workflows/perf-router.yml`
  (122 lines, new) runs the `bench_router.td` gate on every
  PR / push / schedule / dispatch; the main branch auto-updates
  `.github/bench-baselines/perf_router.json` (new, machine-readable
  JSON with `native_js_ratio_max=3.0` / `sys_real_ratio_max=0.40`
  / `min_samples_required=5`). `scripts/bench/perf_router_gate.py`
  (200 lines) parses `PERF_ROUTER_*` emit lines + drives the
  sample-count state machine + EWMA baseline update. The
  opt-in `c26b_024_router_perf_gate` test is `#[ignore]`-gated;
  none of the three existing Round 10 parity tests were
  modified. Thresholds derived from local 16T measurements
  (Native / JS = 1.97, sys / real = 8.7 %) with 1.5× / 4×
  headroom for CI 2C variance (local ≠ CI lesson). Sampling
  phase is warn-only by design; hard-fail starts at sample
  count ≥ 5. Worktree contract was fully respected.
- **Round 11 / wθ** (`e22ef29` docs → `fe6f526` merge, **C26B-013
  rolling docs amendment — Round 9 + Round 10 narrative**) —
  catch-up section above.
- **Round 11 / wζ** (`4692fd8` in the `taida-lang/terminal`
  submodule, **C26B-012 `PENDING_BYTES` FIFO FIXED**) —
  landed outside the language repo because the change body is
  entirely inside the `terminal` addon
  (`.dev/official-package-repos/terminal/`). Replaces the old
  `static PENDING_BYTES: Mutex<VecDeque<u8>>` with
  `thread_local!(static PENDING_BYTES: RefCell<VecDeque<u8>>)`
  (Case A from `.dev/C25B019_PENDING_BYTES_DESIGN.md`). Regression
  guard: new `pending_bytes_queue_isolation_across_threads`
  stress test pins cross-thread byte isolation (two OS threads
  push distinct sequences, drain independently, no theft). The
  `ReadEvent[]()` public signature is unchanged; the host-side
  contract (always call from a dedicated blocking thread / single
  OS thread per stdin stream) is documented in
  `docs/guide/11_async.md` § `Async と addon の blocking I/O`.
  The terminal submodule still needs `origin` push + PR merge +
  `@a.x` release tag publish before downstream addon consumers
  can pick it up — user action.

### Round 11 wι review amendment (2026-04-25)

A review on 2026-04-25 downgraded the previous "agent-side
FULLY READY" wording to **HOLD**. Four items were surfaced:

- **C26B-029** `[FIXED during review]` — CI perf gate
  false-green hardening. `.github/workflows/bench.yml` had
  `continue-on-error: true` plus `|| true` on the NET
  throughput step; `scripts/bench/parse_net_throughput.py` could
  not parse the `NET6-3b-3` `throughput=… total_bytes=… elapsed=…`
  line format (treating missing samples as acceptable); and
  `scripts/bench/perf_router_gate.py` returned exit 0 when its
  `PERF_ROUTER_*` emit lines were absent. All three false-green
  holes fixed in the same review session (commit `cfb8b0b`).
- **C26B-030** `[FIXED during review follow-up]` — SEC-011
  install-side verify wiring gap. Release-side Sigstore signing
  + SLSA provenance shipped at Round 2 / wB, and
  `scripts/release/verify-signatures.sh` existed; however the
  actual `taida install` / `install.sh` consumption path did
  not invoke the verifier. Closed by:
  - **New `src/addon/signature_verify.rs`** module — resolves
    a per-URL `VerifyPolicy` (`Disabled` / `BestEffort` /
    `Required`), fetches `<artefact>.cosign.bundle` next to
    the prebuild, shells out to `cosign verify-blob` under the
    pinned identity regex (`^https://github.com/taida-lang/`)
    and OIDC issuer (`https://token.actions.githubusercontent.com`).
    `TAIDA_VERIFY_SIGNATURES` selects the policy; unset means
    `BestEffort` for first-party URLs and `Disabled`
    elsewhere. Integration tests inject a temporary fake `cosign`
    executable into `PATH` to exercise both pass and fail paths
    without needing the real binary on every CI runner; the
    production verifier has no env-var bypass.
  - **`src/pkg/resolver.rs::try_fetch_prebuild`** calls the
    verifier after the SHA-256 check; failures map to
    `PrebuildFailure::IntegrityMismatch` so the install
    aborts on any signature rejection.
  - **New `install.sh`** at repo root — the public installer.
    Downloads `taida-<TAG>-<TRIPLE>.tar.gz`, the matching
    `.cosign.bundle`, and `SHA256SUMS` + `SHA256SUMS.cosign.bundle`.
    Enforces SHA-256 against the signed sums file then runs
    `cosign verify-blob` on both the tarball and the sums.
    `TAIDA_VERIFY_SIGNATURES=required` mode fails the install
    if any bundle is missing or `cosign` is not on `PATH`.
  - Regression guard: `tests/c26b_030_sec011_install_verify.rs`
    (10 cases) pins the `Disabled` / `BestEffort` / `Required`
    decision table + fake-ok / fake-fail / fake-missing-cosign
    paths. Unit tests in the new module pin URL matcher
    tightness and policy-resolution edge cases.
- **C26B-002** `[REOPEN → FIXED during review follow-up]` —
  TLS construction 3-backend parity pin. The existing
  `test_net5_3b_tls_*` cases covered interpreter + JS only;
  the 2026-04-25 review flagged the missing native branch.
  Closed by five new 3-backend parity tests in `tests/parity.rs`
  (`test_net6_1c_c26b002_{1..5}_*`) that cover the cert-missing,
  key-only, plaintext-fallback, invalid-PEM-content, and
  unknown-protocol-token permutations across interpreter / JS /
  native. Live cert rotation + ALPN matrix remains in the
  C26B-005 soak runbook (live TLS handshakes are runtime-
  dependent; the construction-time contract being pinned here
  is what `cargo test --release` can deterministically
  reproduce).
- **C26B-005** `[REOPEN]` — 24 h soak PASS evidence remains
  open. Runbook landed at Round 2 / wA, but the 24 h run itself
  is a user action and no PASS record currently lives in the
  repo. The review amendment adds a new **fast-soak proxy**
  (`scripts/soak/fast-soak-proxy.sh`, 30-min to 3-hour short
  run) so developers can get a first-order leak / drift signal
  during the iteration loop; a proxy PASS does **not** close
  the C26B-005 acceptance (only a documented 24 h PASS does).
  `.dev/C26_SOAK_RUNBOOK.md` gained § 0.1 (acceptance
  ownership) and § 7.1 (fast-soak-proxy usage) to make the
  split explicit.

Review amendment `EXPECTED_TOTAL_LEN` unchanged (no `src/`
C-fragment bytes moved; the SEC-011 wiring is pure Rust).
All `cargo test --lib` (2539) + `cargo test --release --test parity`
(667) pass under the review amendments.

**Worktree contract — Round 10 post-mortem (1 line):** the
Round 10 / wε integration session briefly wrote a partial-leak
fragment outside its isolation worktree before being corrected;
the final merged state on `feat/c26` is the clean one and no
main-tree artefact persists. The wθ rolling amendment session
(this commit) explicitly re-verifies the worktree isolation
contract at entry and confines all edits to its own isolation
prefix.

### wθ rolling amendment (this commit)

wθ (C26B-013 rolling docs amendment — Round 9 + Round 10 merge
narrative) extends the Round 8 / wδ narrative to cover
Round 9 (wα / wβ / wδ) and Round 10 (wε). Specifically:

- Flips **C26B-024** above from `[PARTIAL FIXED]` to `[FIXED]`
  by appending the Round 10 / wε Step 4 full-acceptance block
  as a monotone superset over the Round 8 / wT narrative (no
  prior text is contradicted; the Step 2 + Step 3 Option A
  narrative is preserved verbatim and the Step 4 block
  extends it with the final acceptance numbers and the
  six-change runtime refactor detail).
- Promotes **C26B-027** and **C26B-028** from the Round 8
  OPEN list to FIXED via the Round 9 additions section above
  (the "New blockers opened during Round 8" section stays as
  the audit-trail record of their origin; the Round 9
  additions section records their closure).
- Records the `EXPECTED_TOTAL_LEN` 998,598 → **1,012,971**
  delta driven by Round 10 / wε (F1 251,878 → **266,252**;
  F2 / other fragments / interp / JS / wasm profiles
  unchanged).
- Adds an explicit **`@c.26` GATE-READY status marker**
  (section below).

No code change in this commit. The wδ narrative is extended
monotonically — no prior text is contradicted (the Round 8 /
wT Step 2 + Step 3 Option A achievement is preserved as the
first half of the C26B-024 FIXED narrative, and the Round 10
/ wε Step 4 is the second half). D27 escalation checklist:
3/3 NO — no mold signature / pinned error string / existing
parity assertion is altered by this docs amendment.

### `@c.26` GATE status (agent side, 2026-04-25 review update)

The previous wθ **GATE-READY** claim is downgraded to **HOLD**
following the 2026-04-25 review. The OPEN / REOPEN residuals
are now:

- **C26B-002** `[FIXED during review follow-up]` — 3-backend
  TLS construction parity pin (five new
  `test_net6_1c_c26b002_{1..5}_*` cases in `tests/parity.rs`).
- **C26B-005** `[REOPEN]` — 24 h soak PASS record remains a
  user action; runbook + fast-soak proxy are both landed but
  the PASS evidence lives outside the repo until the user
  runs it.
- **C26B-012** `PENDING_BYTES` FIFO — **user-side only** from
  this repo's perspective. Agent-side work is closed (wζ
  Round 11, commit `4692fd8` in the `terminal` submodule).
  The submodule still needs upstream push / PR / release tag
  before consumers see the fix.
- **C26B-013** rolling amendment — this section + §5.6.1
  (`docs/STABILITY.md`) + `.dev/C26_PROGRESS.md` tracker
  stayed out of sync with Round 10 / 11 before the 2026-04-25
  review; now re-synced under the Round 11 wι amendment.
- **C26B-029** `[FIXED during review]` — CI perf gate
  false-green hardening (two scripts + one workflow).
- **C26B-030** `[FIXED during review follow-up]` — SEC-011
  install-side cosign verify wiring (`src/addon/signature_verify.rs`
  + resolver hook + public `install.sh`).

The `@c.26` Phase 14 promotion gate itself remains a
**user-approved tag**; the agent does not cut `@c.26.rcM` /
`@c.26` under any condition. The stable-declaration gate
requires (from §5.6 and §6.1):

- All Critical / Must Fix closed **(agent side: DONE as of
  wθ)** + C26B-012 FIFO (user side) closed.
- `cargo test --release` all-pass, 0 red, 0 SLOW warnings.
- 3-backend parity across all fixtures (all 185 parity tests
  + 880+ total tests green as of Round 10 / wε).
- CI 2C wall-clock median ≤ 8 minutes.
- Parallelism efficiency ≥ 80 %.
- 24-hour soak test PASS (C26B-005; runbook landed at
  Round 2 / wA, the 24 h run itself is the manual user
  action).
- Downstream `bonsai-wasm` Phase 6 smoke (C26B-020
  acceptance).
- `SECURITY_AUDIT.md` open = 0; SEC-011 recorded complete
  (Sub-phase 7.4 FIXED at Round 2 / wB; the SEC-011 workflow
  invariants are pinned by the Round 9 / wβ symmetry test).
- Sigstore + SLSA-signed official addon release (SEC-011
  produced the publish-side signing and the install-side
  verify; the first signed release itself is the user
  action).

### Docs / infrastructure landed alongside

- `docs/STABILITY.md` — `Target: @c.26`, §1 `@c.25`-skip pin,
  §1.2 D26→D27 rename (prose-only; the pinned runtime error string
  keeps the legacy `D26` token for gen-C), §4.2 / §4.3 / §4.4
  generational language clarified, §5.1 / §5.4 / §5.5 ownership
  transferred to C26 blockers. §5.1 port-bind race marked FIXED,
  §5.5 adds a `readBytesAt` addendum, §5.6 is the informational
  C26 progress snapshot (non-contractual), re-synced through
  Round 10 / wε (C26B-024 FIXED, C26B-027 + C26B-028 FIXED,
  `@c.26` GATE-READY status marker added).
- `.dev/C26_SOAK_RUNBOOK.md` — new (C26B-005 scaffolding, wA
  Round 2).
- `docs/reference/net_api.md` (new) — `httpServe` `req` pack shape
  (1-arg / 2-arg table), span-aware mold reference, perf guidance
  (hot path `SpanEquals`, cold path `strOf`).
- `docs/reference/mold_types.md:717` — Stream lowering landed at
  C25B-001 Phase 3; `STREAM_ONLY_FIXTURES` is empty (4-backend
  parity now applies).
- `docs/reference/addon_manifest.md` / `docs/guide/13_creating_addons.md`
  — D26→D27 prose rename; the pinned runtime error string is
  preserved and its `D26` token documented as a gen-C surface
  artefact.
- `src/js/runtime/core.rs:1100` — Stream-lowering comment updated
  to reflect 4-backend parity.

### Out-of-scope for C26 (deferred to D27)

- Function-name capitalisation cleanup.
- WASM backend for addons (`AddonBackend::Wasm`) — gen-D only.
- Addon ABI v2 (`on_panic_cleanup`, termios-restore hook).
- Diagnostic renumbering (`E1xxx` rename / retire).
- `req.method` auto-`Str` promotion (C26B-016 Option A).
- Rewriting the legacy `wasm planned for D26` error-string token
  (pinned surface for gen-C).

See `.dev/D27_BLOCKERS.md` and
`MEMORY/project_d27_breaking_change_phase.md`.

---

## @c.25.rc7 (2026-04-23)

Quality-consolidation RC cycle. `stable` (label-less `@c.25`) is
**deferred** to a follow-up RC cycle because the NET stable viewpoint
(HTTP/2 parity, TLS configuration, port-bind-race eradication,
throughput regression guards, scatter-gather long-run correctness)
is not yet comprehensively covered in this track. `@c.25.rc7` closes
out the addon-ecosystem redefinition, consolidates parity residuals
left over from C24, and stages the runtime-perf / security / stability
work items enumerated in `.dev/C25_BLOCKERS.md`.

### C25B-030 — addon ecosystem redefinition (Phase 1, Critical)

The RC1-era "addon is Native only" freeze is formally **lifted**.
Addons now ship with two first-class backends:

- **Interpreter** — the reference implementation; facade + cdylib
  dispatch both execute dynamically.
- **Native** — Cranelift-lowered code that consumes the same facade
  surface through a compile-time static analyser.

A D26 breaking-change phase is reserved for the WASM backend. The
`AddonBackend::Js` entry is still deterministically rejected; it has
no dispatcher today and its revival is tracked for D26.

User-visible consequences:

- `taida build --target native` now accepts addon-backed imports for
  the full facade surface produced by `taida-lang/terminal`: relative
  `>>> ./X.td` children, public FuncDefs, private `_`-prefixed
  helpers reached through reachability, pack literals, scalar /
  list / arithmetic / template / mold / type bindings, and
  authoritative `<<<` export clauses. Nothing user-authored changes.
- The error message `"addon-backed package '...' is not supported on
  backend 'X' (RC1: native only)"` is retired. The replacement reads
  `"(supported: interpreter, native; wasm planned for D26). Run
  'taida build --target native' or use the interpreter."`, and carries
  the same guidance into every `taida build` / `taida run` diagnostic.
- The interpreter no longer masquerades as `AddonBackend::Native` for
  policy-guard purposes — it is registered honestly as
  `AddonBackend::Interpreter`, which is now `supports_addons = true`.
- TypeDef / EnumDef / MoldDef inside a facade file, non-relative
  `>>>` targets, and `<<< <path>` re-exports still produce
  deterministic rejections (messages point at
  `C25B-030 Phase 1E-γ pending`). Real `taida-lang/terminal` does
  not depend on any of these constructs so no public addon was
  affected.

Implementation structure (Phase 1A → 1H):

- **Phase 1A** — `src/addon/backend_policy.rs::supports_addons`
  widened to `matches!(Interpreter | Native)`; error text updated;
  unit tests re-shaped.
- **Phase 1B** — `src/interpreter/module_eval.rs::try_eval_addon_import`
  calls `ensure_addon_supported(AddonBackend::Interpreter, ...)`
  truthfully; the `feature = "native"` gate still controls whether
  the dlopen dispatcher is linked in, but it no longer lies to the
  policy.
- **Phase 1C** — `"RC1: native only"` purged from every consumer-
  facing string. Existing integration tests were migrated to the
  new message format.
- **Phase 1D** — `tests/c25b030_core_bundled_native_smoke.rs` pins
  that the core-bundled packages (`taida-lang/os` / `net` / `crypto`
  / `pool` / `js`) still compile natively. These never went through
  the facade loader — they resolve via hand-coded symbol tables in
  `src/codegen/lower/stmt.rs` — and the regression guard now
  protects that path from being broken by any future facade-loader
  change.
- **Phase 1E-α** — `src/codegen/lower/imports.rs::load_addon_facade_for_lower`
  extended with recursive `>>> ./X.td` relative-import walking. The
  child's `<<<` clause is authoritative when the parent imports
  without a `@(...)` list; the parent's `@(...)` list is the
  selective filter otherwise. Circular chains, missing child
  symbols, non-relative paths, and `<<< <path>` re-exports all
  produce deterministic compile errors naming the offending facade
  file.
- **Phase 1E-β** — facade FuncDefs harvested into a new
  `AddonFacadeSummary.facade_funcs` map and lowered as IR functions
  in `lower_program`'s 2nd pass under mangled link symbols
  `_taida_fn_facade_{pkg_hash}_{name}`. User imports of a public
  FuncDef resolve through `imported_func_links` at call sites.
  Assignment RHS widened to accept scalar literals, template
  strings, lists, arithmetic, function / method calls, field
  accesses, and mold / type instantiations (previously only `@(...)`
  packs and aliases were accepted).
- **Phase 1E-β-2 + β-3** — a reachability fixpoint promotes
  private `_`-prefixed helpers transitively pulled in by an
  exported FuncDef body or pack binding. Real
  `.dev/official-package-repos/terminal/` now compiles natively
  end-to-end (`BufferNew`, `Stylize`, `LineEditorNew`,
  `LineEditorStep`, `LineEditorRender`, `PromptOptions`, `KeyKind`,
  `EventKind`, `MouseKind`, `SpinnerNext`, `SpinnerRender`,
  `SpinnerState`, `ProgressBar`, `StatusLine`, `ReadEvent`,
  `ClearScreen`). See
  `tests/c25b030_phase_1e_facade_chain.rs::phase_1e_beta3_*`
  for the regression guards.
- **Phase 1F** — `tests/c25b030_phase_1f_facade_parity.rs` pins
  interpreter ↔ native parity across five scenarios (mixed facade
  with aliases / public packs / FuncDefs / private helpers /
  relative chains; guard-arity; authoritative `<<<` exports;
  cross-file private helper chains; pure-Taida-only packages).
  Fixing the first two scenarios surfaced a pre-existing cross-
  backend divergence: top-level bindings referenced **only** from
  a `TemplateLit` interpolation inside a FuncDef body (e.g.
  `sep <= "-"` + `join a b = \`${a}${sep}${b}\``) never reached
  the `GlobalGet(hash)` emission or the facade reachability
  walker, because the free-vars / reachability walkers did not
  understand that `TemplateLit` stores its interpolation
  expressions as a raw string the real lowering path re-parses.
  `collect_free_vars_inner` in `src/codegen/lower/stmt.rs` and the
  facade reachability walker in `src/addon/facade.rs` now split on
  `${...}` boundaries the same way `lower_template_lit` does, re-
  parse each interpolation, and walk the result through the
  existing identifier machinery. Parse failures fall back to a
  bare-identifier capture, matching the real lowering's behaviour.
  This closes a regression window that predated C25 entirely.
- **Phase 1G** — the static facade loader extracted into
  `src/addon/facade.rs` as a first-class `pub` module. The
  recursive `>>>` walker, the universe-map machinery, the
  reachability fixpoint, and the `TemplateLit`-aware reference
  collector now live in the shared module. Codegen
  (`src/codegen/lower/imports.rs::lower_addon_import`) adopts the
  shared `AddonFacadeSummary` verbatim and keeps only backend-
  local bookkeeping (mangled symbol registration, type-tag
  narrowing, pack-binding replay) on its side. The D26 WASM
  backend will consume the same module without duplicating the
  walker.
- **Phase 1G unit tests**: `src/addon/facade.rs::tests` —
  five tests covering the "no facade file" soft path, mixed-
  construct harvesting, TypeDef rejection with `Phase 1E-γ pending`,
  missing-child-symbol rejection, and the cross-file template
  reachability case that anchored the Phase 1F fix.
- **Phase 1G acceptance**: `AddonFacadeSummary` / `FacadeLoadError`
  are the two public types any future backend will import; the
  interpreter's addon facade path in
  `src/interpreter/module_eval.rs::load_addon_facade` is
  deliberately left on its dynamic-execution strategy because the
  interpreter exchanges live runtime values with user code and
  does not benefit from the static analyser.

### C25B-003 / C25B-004 — CI redesign and perf-gate scaffold (Phase 2)

The C24 CI optimisation track missed its `-30%` wallclock target
and actually regressed `+6.5%` in CI 2C 3-run median. Phase 2
replaces the methodology with "CI 2C 3-run median is the sole
source of truth; local 16T numbers are advisory only" and lands
the following:

- **Phase 2A** (`cb60696`) — `tests/parity.rs::test_net6_5b_release_gate_v6_test_counts`
  stops spawning `cargo test --list` as a subprocess. `build.rs`
  now scans `tests/parity.rs` and `src/interpreter/net_h2.rs` at
  build time and emits `PARITY_TEST_FN_NAMES` /
  `NET_H2_UNIT_TEST_COUNT` as consts into `$OUT_DIR/parity_release_gate.rs`.
  The test itself becomes an in-process constant comparison. The
  single-biggest SLOW warning in the CI 2C profile (~79s) is
  removed; the local measurement collapses to sub-millisecond.
- **Phase 2B** (`07c91ff`) — all CI jobs move from
  `actions/cache@v5` to `Swatinem/rust-cache@v2` with
  `shared-key: cargo-base` and `save-if: github.ref == 'refs/heads/main'`.
  This gives cross-job cache sharing without lock contention,
  correct save-on-success semantics, and automatic `target/`
  pruning. `fmt` stays un-cached since it does not touch `target/`.
- **Phase 2C** — `concurrency: cancel-in-progress` was already
  landed in C24 Phase 5 (commit `e285817`, C24B-008). No
  additional action required at this phase.
- **Phase 2D** (`159e355`) — a new `build-archive` CI job runs
  `cargo nextest archive --all-targets --archive-file
  taida-nextest.tar.zst` once, uploads the archive via
  `actions/upload-artifact@v4` with 1-day retention, and the
  `test` job pulls it down with `actions/download-artifact@v4`
  and executes `cargo nextest run --archive-file ...
  --workspace-remap . --profile ci`. The `test` job's build and
  link phase (~270s) is fully eliminated; nextest extracts 88
  pre-built test binaries (~961 MB `.tar.zst`) and runs them
  directly. `parity` / `e2e` / `check` / `clippy` / `fmt` stay
  on the Swatinem path because they depend on live `cargo`
  metadata or build CLI binaries that nextest archive does not
  cover.
- **Phase 2E — `tests/parity.rs` 33k-line binary split**: **SKIP**
  judgement. Risk / reward analysis concluded the `build.rs` +
  `include!` + `common` mod re-wiring required to split 33,161
  lines / 556 tests into `parity_core.rs` / `parity_net.rs` /
  `parity_net_tls.rs` / `parity_phase_c.rs` / `parity_quality.rs`
  is too dangerous for a quality-consolidation RC cycle. Phase
  2D already eliminated the rebuild cost that split would have
  amortised. Deferred to a follow-up RC cycle.
- **Phase 2F — RC-SLOW-2 per-fixture decomposition re-evaluation**:
  stay-the-course. The C24 `+6.5%` CI 2C regression was measured
  under a different rate-limiter (the 79s SLOW test was still
  live). With 2A removing the SLOW test and 2D removing the
  build phase, the +6.5% delta is expected to be absorbed by the
  combined `-30%`-to-`-40%` improvement. Partial-revert would
  give up local `-9.3%` for speculative CI gain. Re-evaluation
  trigger: if post-merge CI 3-run median of the `test` job
  exceeds 5 minutes, revisit in a separate PR.
- **C25B-004 (perf-gate body)**: a `benches/perf_baseline.rs`
  harness and `.github/workflows/bench.yml` scaffold were landed
  in Phase 2C (commit `4997fca`) with `continue-on-error: true`
  (warn-only). Hard-fail promotion to "`-10%` regression blocks
  merge" requires baseline accumulation across multiple main
  pushes and is post-stable scope.

Expected post-2D CI 2C wallclock: 12m 13s → 8-9m (the post-stable
8-minute target is within reach). Measured 3-run median is
captured in `.dev/C25_CI_PROFILE.md` once the PR lands.

### C25B-001 / C25B-028 / C25B-031 / C25B-032 / C25B-033 — Parity residuals (Phase 3)

- **C25B-001 (Stream lowering, Must Fix)** (`4e17e89`) —
  minimal single-item completed `Stream` lowering reaches native
  and wasm. `taida_stream_new` / `taida_stream_is_stream` /
  `taida_stream_to_display_string` land in
  `src/codegen/native_runtime/` and
  `src/codegen/runtime_core_wasm/` along with
  `taida_stdout_display_string` routing. The
  `STREAM_ONLY_FIXTURES` skip list in
  `tests/c23_str_parity.rs` loses its last Stream-gated
  fixture and the test joins the 4-backend parity mainline.
  `EXPECTED_TOTAL_LEN` drifts from this landing are closed out
  by `a9b6210` (see below). **Historical context (RC2.x →
  `@c.25.rc7`)**: prior to this landing, `Str[stream]()` was
  parity-pinned for Interpreter / JS only — native and wasm
  fixtures lived behind `STREAM_ONLY_FIXTURES` because the
  display-string routing was missing on those backends. With
  `4e17e89` + the `a9b6210` boundary resync, all four backends
  now produce byte-for-byte identical output for the
  Stream-gated fixtures, and `STREAM_ONLY_FIXTURES` is held
  empty by `tests/c23_str_parity.rs` going forward (any
  regression that re-introduces a backend-specific Stream
  divergence breaks the parity test).
- **C25B-028 (jsonEncode Gorillax parity, Must Fix)** (`48d26da`) —
  `jsonEncode(Gorillax[42]())` was emitting three different
  shapes across backends (interpreter =
  `{"__error":{},"__value":42,"hasValue":true}`, native =
  `{"hasValue":true}`, wasm = `{"hasValue":1}`). Monadic-pack
  detection (`Lax` / `Gorillax` / `RelaxedGorillax` /
  `Result`) now visits `__error` / `__value` / `__default`
  / `__predicate` / `throw` fields in native
  `json_serialize_pack_fields`, wasm
  `_wc_json_serialize_pack_fields`, and JS
  `__taidaSortKeys`. Wasm field-type registry learns `hasValue`
  as Bool so `true` renders as `true` rather than `1`. All four
  backends now emit the interpreter shape byte-for-byte.
- **C25B-031 (Must Fix)** — `Slice[s, pos_var, end]()` with
  `pos` as a bound `Int` variable returned the whole string on
  native / wasm while interpreter correctly returned the slice.
  Native / wasm Slice lowering now resolves positional IntVar
  arguments identically to the interpreter, and three parity
  fixtures pin the fix.
- **C25B-032 (Should Fix)** (`4696429`) — `| _ |> <call-that-throws>`
  inside an `|==` handler's own function failed to propagate;
  the throw escaped as "Unhandled error". The arm-body throw
  propagation path now rejoins the same mechanism used for
  inline bodies across all three backends (interpreter / JS /
  native).
- **C25B-033 (Should Fix)** (`a26f0c3`) — JS codegen emitted
  `function Join(...) { ... }` twice whenever the user named a
  FuncDef `Join` (or any of 98 prelude-reserved PascalCase
  identifiers), causing `SyntaxError: Identifier 'Join' has
  already been declared` on node ESM. A `PRELUDE_RESERVED_IDENTS`
  sorted-slice + `mangled_user_func_name` helper mangles only
  the colliding names to `_td_user_<name>`. Non-colliding
  PascalCase (e.g. `MyCustomFunc`) stays verbatim — the
  Taida surface guarantee that function names are free is
  preserved. Mangling applies at five emission sites
  (declaration, trampoline, TCO inner, direct call, pipeline
  fallback); diagnostics keep raw names.
- **EXPECTED_TOTAL_LEN / historical-boundary resync**
  (`a9b6210`) — native runtime byte totals and WASM historical
  boundary markers caught up with the C25B-001 Stream lowering
  and other Phase 5 runtime additions so that
  `tests/native_runtime_size.rs` and the C13.4 merge guards go
  back to green. Details in the commit body.

### C25B-002 / C25B-017 — Flaky eradication (Phase 4)

Flaky eradication work in this phase is narrowly scoped to the
audit / regression-guard layer; the root-cause fixes for the
NET flakes are explicitly deferred (see § Deferred NET stable
viewpoint below).

- **C25B-017 (Nice to Have)** (`f5aeb44`) — parser
  error-recovery boundary audit. A test suite surveys the
  `parse_error_ceiling` / `parse_cond_branch` recovery path
  against the C20-1 ROOT-4 / ROOT-5 rejection contract and pins
  the current recovery shape. Any future drift in `current_token`
  / `peek_kind` consistency after a recovered error is caught
  by this suite. Formalising the state machine as a transition
  graph (the original "quality track" part of FB-31) is
  deferred.
- **C25B-002 (Must Fix)** — the `flaky_h2_parity` port-bind
  race remains covered only by the retry shim inherited from
  C24. Root-cause eradication (OS-assigned ports with
  `getsockname()` handover, or an in-process `axum` / `hyper`
  test harness) is explicitly deferred to the subsequent RC
  cycle; see § Deferred NET stable viewpoint.

### C25B-020 ~ C25B-026 / C25B-029 — Runtime perf (Phase 5)

Phase 5 lands a coordinated set of interpreter / native / wasm
perf fixes. All interpreter-side changes ship with 4-backend
parity updates in the same commit; no commit leaves parity in a
broken intermediate state.

- **Phase 5-A — math mold family on interpreter + JS (C25B-025
  partial)** (`86d5743`) — 17 math molds (`Sqrt`, `Pow`,
  `Exp`, `Ln`, `Log`, `Log2`, `Log10`, `Sin`, `Cos`, `Tan`,
  `Asin`, `Acos`, `Atan`, `Atan2`, `Sinh`, `Cosh`, `Tanh`)
  land on the interpreter (`mold_eval.rs`) and JS runtime
  (`core.rs`). Previously `Sqrt[4.0]() -> 4.0` (silent first-
  argument return); the transcendentals were registered for
  type inference but had no implementation. `mold_returns.rs`
  registers the full surface as Float-returning.
- **Phase 5-B — O(1) regex cache (C25B-024)** (`4d794f2`) —
  the `VecDeque<((pattern, flags), Regex)>` linear-scan cache
  becomes a `HashMap<(String, RegexFlags), Regex>` with O(1)
  lookup. No API change; `regex`-using hot loops (lexers,
  tokenisers) save the 64-entry pattern compare per call.
- **Phase 5-C / 5-D / 5-E — ValueKey + HashSet fast path for
  Set / HashMap / Unique (C25B-021 / C25B-022 / C25B-023)**
  (`f721c6d`, cross-type fix in `166f0c3`) — `Set.union` /
  `Set.intersect` / `Set.diff` / `HashMap.merge` / `Unique`
  were all `Vec<Value>::contains` linear-scan structures with
  O(N*M) behaviour. A new `ValueKey` wrapper (float NaN /
  Function / Closure fall back to equality comparison,
  everything else hashes) feeds per-operation
  `HashSet<ValueKey>` / `HashMap<ValueKey, _>` fast paths.
  A 1000-element × 2 Set union now completes in under 50 ms.
  `HashMap.merge` drops from O(N*M*K) (nested retain with
  per-entry key-field linear scan) to O(N+M) via a pre-built
  key index. Phase 10 GATE pre-review (session 20) found the
  initial `ValueKey` put `Int(n)` and `EnumVal(_, n)` in
  separate key domains, which broke `Value::eq`'s cross-type
  rule (`setOf(@[0]).union(setOf(@[Color:Red()])).length()`
  returned 2 instead of 1); `hash_value_into` now normalizes
  them to the same fingerprint and `exact_eq` carries the
  Int↔EnumVal rule, restoring fast-path parity with the
  linear-scan contract. Unique mold additionally falls back
  to `Value::eq` on any fingerprint collision.
- **Phase 5-F — env `HashMap<String, Rc<Value>>` migration
  (ABANDONED)** — the refcount-the-env probe demonstrated that
  `env.snapshot()` call sites hit a `Rc<Value> -> Value`
  conversion that itself deep-clones, erasing the perf win on
  any capture-heavy path. Abandoned; the investigation report
  is recorded in `.dev/C25_PROGRESS.md::session 7`. The real
  C25B-029 fix is in 5-F2 below.
- **Phase 5-F2-1 — `Value::List` interior `Arc<Vec<Value>>`
  migration (C25B-029)** (`ac95b09`) — the interpreter's
  `Value::List(Vec<Value>)` becomes `Value::List(Arc<Vec<Value>>)`;
  `Value::list(items)` / `Value::list_take(arc)` helper
  constructors land; ~215 consumer sites migrate mechanically.
  Read-only paths (touch-chains that drive TUI renderers) drop
  from O(N) Value::clone to O(1) Arc refcount bump. The
  C25B-029 guard in `tests/c25b_029_interpreter_bind_clone.rs`
  tightens from 30 s to 5 s.
- **Phase 5-F2-2 Stage B + C — Append / Prepend env-take fast
  path (C25B-021 / C25B-029)** — initial commit (`f043586`)
  introduced an `env.take_from_current_scope` + `Arc::make_mut`
  fast path to drop `Append` / `Prepend` from O(N²) to O(N).
  Phase 10 GATE pre-review (session 20) discovered the fast
  path violated the mold contract: `Append[xs, v]()` mutated
  the source binding (e.g. `xs.length()` changed after a
  subsequent mold call consumed the returned value), since
  `take_from_current_scope` stole the only Arc and
  `Arc::make_mut` mutated in place. The optimization was
  **reverted** in `166f0c3`: `take_from_current_scope` is
  removed, the trampoline still calls `current_args.clear()`
  (harmless independent of the fast path), and Append / Prepend
  return to the COW fallback (`as_ref().clone()` → push →
  rebind, i.e. O(N) per call / O(N²) per N-append loop). Perf
  guards in `tests/c25b_021_append_linear.rs` are relaxed to
  `N=5000 / 2 s` and `N=5000 / 3 s` with an in-file rationale;
  the C25B-029 ceiling in
  `tests/c25b_029_interpreter_bind_clone.rs` is relaxed from
  500 ms back to 3 s. Semantic regression tests pinning the
  binding-preservation contract land alongside. True O(1)-per-
  append amortization (persistent vector / pure-functional
  `BuilderEscape`) is deferred to Phase 5-F2-3+ or D26.
- **Phase 5-G — wasm-wasi linear memory growth strategy
  (C25B-026)** (`6c6ccbb`) — four runtime helpers
  (`wasm_arena_enter` / `wasm_arena_leave(saved)` /
  `wasm_arena_used` / `wasm_arena_roundtrip_test`) land in
  `src/codegen/runtime_core_wasm/01_core.inc.c`. A bump-
  allocator watermark snapshot/restore gives O(1) release of
  all allocations inside an enter/leave scope. WebAssembly
  cannot shrink linear memory back to the OS, but the arena
  reuse stops `memory.grow` from being called in long-running
  `@[Float]` / LLM forward loops. `wasm-ld`
  `--initial-memory=` / `--max-memory=` are driven by
  `TAIDA_WASM_INITIAL_PAGES` / `TAIDA_WASM_MAX_PAGES`
  environment variables. The four helpers are exported on
  `wasm-wasi` and `wasm-full` profiles; `wasm-min` and
  `wasm-edge` drop them via `--gc-sections`. Three regression
  tests pin net-delta = 0 over 1000 × 32 × 64 B churn, verify
  the memory section byte-level for custom page counts, and
  guard the exports. User-authored lower-level auto-insertion
  of enter/leave pairs (escape-analysis-safe scope
  recognition) is deferred to a future phase.
- **Phase 5-I — math mold family on native + wasm (C25B-025)**
  (`1fdf6f8`) — completes C25B-025 by landing the 17 math
  molds on the remaining two backends. Native wraps libm
  (already linked via `-lm`); glibc libm is shared with the
  Rust interpreter so native is bit-for-bit parity with the
  interpreter on x86_64-linux / aarch64-linux. WASM is
  `-nostdlib` so libm is not available: `Sqrt` uses the
  `f64.sqrt` opcode (bit-exact); `Pow` fast-paths integer
  exponents via repeated squaring (bit-exact) and falls back
  to `exp(y * ln(x))` for non-integers; `Exp` / `Ln` use
  range-reduction plus truncated Taylor / atanh series;
  `Sin` / `Cos` use Cody-Waite reduction plus a direct 13-term
  Taylor (so `Cos[0.0]` stays bit-exact `1.0`); `Asin` /
  `Acos` route through `atan(x/sqrt(1-x²))`; `Atan` /
  `Atan2` use `tan(π/8)` range-reduction plus a 20-term
  Maclaurin; `Sinh` / `Cosh` / `Tanh` use exp-based formulas
  with small-|x| Taylor series. `EXPECTED_TOTAL_LEN` native
  974,273 → 976,168 (+1,895) and wasm 318,189 → 333,024
  (+14,835) are resynced. Parity is `assert_eq!` on
  interpreter / native / wasm-sqrt-pow and
  `numerically_close(rel_tol=1e-10)` on wasm transcendentals
  (the freestanding series carry ~1 ULP drift; tolerance
  catches sign / quadrant / factor-of-two bugs while
  permitting truncation error).
- **Deferred under Phase 5**:
  - **5-F2-3** — migrating `Value::BuchiPack(Vec<(String, Value)>)`
    to `Arc<Vec<(String, Value)>>` was scoped out once
    5-F2-2 absorbed the C25B-029 impact surface. The
    remaining perf win is read-side only (touch-chains
    routing through BuchiPack); priority is low. The work is
    kept inside C25 (not D26) but is not a GATE blocker.
  - **5-H — large-file bytes I/O (C25B-020)** — the
    `readBytesAt(path, offset, len)` surface and
    zero-copy `BytesCursor` slice-view redesign for
    >64 MB reads did not land in this RC cycle. The
    work is kept inside C25, scheduled against the
    downstream bonsai-wasm Phase 6 unblock window.

### C25B-006 / C25B-014 / C25B-015 / C25B-018 — Security + body-error + panic hook (Phase 6)

- **C25B-006 (Must Fix)** (`548ad4f`) — security audit drops to
  "open = 0". `.dev/taida-logs/SECURITY_AUDIT.md` gets a 2026-04-23
  C25 re-triage header that classifies every SEC-001 ~ SEC-011
  finding as either ACCEPTED (by design, 3 items), DEFERRED
  (C26 security follow-up, 7 items), or DEFERRED (post-`@c.25`
  supply-chain, 1 item). Every `Status: OPEN` is replaced with
  a disposition. A root `.github/SECURITY.md` lands with a
  disclosure policy, an accepted-risk surface contract
  (`execShell`, `os` file I/O, `tcpListen 0.0.0.0`), and a
  D26 capability-model pointer. A `deny.toml` for
  `cargo-deny v2` pins the current Cargo.lock licence set; a
  `.github/workflows/security.yml` runs `cargo-audit` and
  `cargo-deny` on push, PR, and a weekly schedule. Both tools
  are `continue-on-error: true` for the `@c.25.rc7` cycle
  (warn-only); hard-fail promotion is a C26 gate.
- **C25B-014 (Should Fix)** (`78f748c`) — a GitHub Security
  Advisory draft for the pre-`@c.15.rc3` `taida upgrade`
  supply-chain issue (the CLI hardcoded a personal GitHub fork
  as the release source) lands at
  `.dev/security_advisories/GHSA-DRAFT-taida-upgrade-supply-chain.md`
  (CVSS 8.1 High, CWE-494 + CWE-829, affected `< @c.15.rc3`,
  patched in `@c.15.rc3`, fixed by commits `56c89e0` +
  `b2fb2e5`). `CHANGELOG.md @c.15.rc3 § Security` gets a
  placeholder pointer. Publishing the advisory, obtaining the
  GHSA ID, requesting a CVE, and posting a pinned issue /
  README banner remain owner actions.
- **C25B-015 (Should Fix)** — the body-error cleanup audit
  promised by FB-29 lands as `tests/c25b_015_body_error_cleanup_parity.rs`
  (four tests, three 3-backend fixtures covering frame unwind
  across three call levels, closure frame release, and a
  `|==` handler that itself throws). Interpreter (scope
  cleanup), JS (`try/finally`), and native (setjmp/longjmp
  based) all produce the same stdout. A pre-existing bug
  (`| _ |> throwBoom("boom")` inside a same-function `|==`
  handler does not propagate) was isolated during the audit and
  filed as C25B-032, fixed in Phase 3.
- **C25B-018 (Nice to Have)** (`cdf0d2c`) — a best-effort
  panic / fatal-signal terminal restore lands as
  `src/panic_cleanup.rs`. `install_panic_cleanup_hook`
  chains a `std::panic::set_hook`; `install_signal_cleanup_handlers`
  registers SIGHUP / SIGTERM / SIGQUIT / SIGABRT handlers via
  `libc::signal`. Both are `OnceLock`-guarded (idempotent).
  On a panic or fatal signal the handlers emit an ANSI reset
  sequence to stderr (cursor show, alt-screen leave, mouse /
  bracketed-paste disable, DECSTR soft reset), then re-raise
  the signal with the default disposition to keep the real
  exit code. Both hooks install from `src/main.rs::main()`
  before the SIGPIPE-ignore setup. Full termios restore via
  `tcsetattr` is scope-deferred to D26 ABI v2 (the addon host
  vtable has no `on_panic_cleanup` slot today).

### C25B-007 — Addon publish workflow completion (Phase 7)

`taida publish` / `taida init --target rust-addon` were already
land (`@c.14.rc1` + RC2.6-3 track). Phase 7 validates:

- The existing `src/pkg/publish.rs` (1002 lines) and
  `src/pkg/init.rs` (1282 lines), the
  `crates/addon-rs/templates/release.yml.template` scaffold,
  and `tests/e2e_rc26_gate.sh` all pass (38 integration tests
  + 266 pkg-lib tests).
- An E2E smoke (`taida init --target rust-addon` → identity
  qualify → `taida publish --dry-run`) was exercised live.
- `docs/guide/13_creating_addons.md` gains a new § 0
  "Getting started with `taida init --target rust-addon`"
  section (66 lines) covering the workflow end-to-end.
- Two cross-reference drifts in the same doc
  (`§7 → §8 Migration`, `§9 below`) are corrected.

Externally-publishable official addons: `taida-lang/terminal` is
the only one. `taida-lang/os` / `net` / `crypto` / `pool` /
`js` are bundled via the `CoreBundledProvider` path and do not
pass through `taida publish`. The release workflow template
symmetry is pinned by `tests/init_release_workflow_symmetry.rs`
(five tests).

### C25B-005 / C25B-008 / C25B-019 — Docs (Phase 8)

- **C25B-005 (Should Fix)** (`ff672fc`) — diagnostic-code
  audit. `docs/reference/diagnostic_codes.md` gains the `E16xx`
  band (15 codes: `E1601` ~ `E1608`, `E1610` ~ `E1614`,
  `E1616` ~ `E1618`), the `E17xx` band (1 code:
  `E1701`), `E1410` (InheritanceDef incompatible field
  redefinition), and `E1609` / `E1615` as explicit
  `(reserved)` gaps. `tests/c25b_005_diagnostic_audit.rs`
  adds six guard tests: every emitted code is documented,
  every documented code is either emitted or reserved,
  reference uses canonical `Exxxx` formatting, band rules
  list all new categories, each reference path points at a
  real doc, and the emit-site scan sanity-finds a known-good
  code.
- **C25B-008 (Should Fix)** (`ff672fc`) — parse-only doctest
  harness. `tests/c25b_008_doc_examples_parse.rs` (three guard
  tests), `tests/c25b_008_doc_examples_probe.rs` (an ignored
  probe for baseline refresh), and
  `tests/c25b_008_doc_parse_baseline.txt` (baseline manifest
  pinning 68 intentional parse failures) together scan 563
  ```` ```taida ```` blocks across `docs/guide/*.md` (14 chapters)
  and `docs/reference/*.md` (13 chapters). Current parse
  success is 495 / 563 (87.9%); the 68 failures are all
  intentional fragments ("this is an error" counter-examples,
  bare type signatures, partial-line snippets). A new
  `// @doctest: skip` block marker is reserved for future
  opt-out (currently unused).
- **C25B-019 (Nice to Have)** — `PENDING_BYTES` FIFO design
  captured in `.dev/C25B019_PENDING_BYTES_DESIGN.md` (three
  options compared: `thread_local!`, transaction boundary,
  per-call owned buffer; leading candidate is `thread_local!`).
  The implementation lives in the `taida-lang/terminal` addon
  repository and lands in a terminal-side RC cycle under the
  stability contract pinned by this document. Taida-lang core
  does not touch the terminal submodule in this RC cycle.

### C25B-009 ~ C25B-013 / C25B-016 — Stability policy (Phase 9)

- **C25B-010 (Must Fix)** (`7e9d964`) — `docs/STABILITY.md`
  lands (seven sections, 460+ lines). § 1 pins the
  `<gen>.<num>.<label?>` version grammar and explicitly bans
  semver-shaped numbers. § 1.2 reserves D26 for breaking
  changes (function name capitalisation cleanup, WASM
  backend extension for addons, addon ABI v2, diagnostic
  renumbering). § 2 enumerates the stable surface (the ten
  operators, the prelude, `E1xxx` diagnostic codes, the
  `taida` CLI, and file-layout contracts). § 3 marks
  `src/` internals, the on-disk format of compiled
  artifacts, exact diagnostic wording, performance, and
  `.dev/` as explicitly non-stable. § 4 pins the addon
  surface including the `targets = ["native"]` forward-compat
  rule for the future D26 WASM backend. § 5 enumerates the
  NET stable viewpoint, the addon WASM backend, the async
  redesign (C25B-016), the terminal async FIFO (C25B-019),
  and performance as deferred items. § 6 codifies how
  breaking changes / additions / bug fixes / deprecations
  are introduced.
- **C25B-009 (Nice to Have)** (`7e9d964`) — binary size /
  startup time budget.
  `scripts/binary_size_baseline.json` pins the `@c.24.rc1`
  baseline (bytes=28,602,232, text=22,708,840, data=653,353,
  bss=13,612, startup_ms=0.45, measured locally at tip
  `972f6ee`). `scripts/measure_binary_size.sh` wraps
  `size(1)` plus a Python `time.perf_counter_ns` startup
  probe. `.github/workflows/binary_size.yml` runs the
  measurement on push-to-main and PRs; >10% regression
  fails the job (`continue-on-error: true` for
  `@c.25.rc7`, hard-fail is post-stable).
- **C25B-011 (Nice to Have)** (`7e9d964`) — coverage
  visibility. `.github/workflows/coverage.yml` runs
  `cargo-llvm-cov --lib --lcov` weekly (Monday 04:00 UTC)
  plus `workflow_dispatch`. Per-module hit / total / pct is
  summarised; `src/interpreter/` carries an advisory 80%
  visibility target (not enforced). lcov + HTML reports
  persist as 30-day artefacts. No external service
  dependency. PR-time coverage is deferred; PR-time
  instrumentation would triple CI cost.
- **C25B-012 (Should Fix)** (`7e9d964`) — crash regression
  corpus grows from 10 to 15 fixtures. The five additions
  stress Stream lowering parity
  (`cfx_c25b001_stream_take_minimal.td`), finite float
  addition / toString parity
  (`cfx_c25b013_float_finite_parity.td`), three-frame
  `|==` unwind
  (`cfx_c25b015_handler_body_throw_nested.td`),
  jsonEncode Gorillax key ordering
  (`cfx_c25b028_json_encode_gorillax_shape.td`), and
  nested pack read under a loop
  (`cfx_c25b029_record_deep_read.td`), plus a sixth fixture
  for TemplateLit global refs
  (`cfx_c25b030_template_lit_global_ref.td`) that also
  isolated C25B-033. All 15 fixtures pass on the
  interpreter / native / JS trio. `cargo-fuzz` integration
  is post-stable.
- **C25B-013 (Nice to Have)** — WASM SIMD / float edge-case
  parity audit: partial. The minimal "finite float addition
  + toString" parity is pinned in the crash corpus above;
  comprehensive NaN / ±Infinity / denormal 4-backend
  pinning is deferred to a follow-up track.
- **C25B-016 (Nice to Have)** (`7e9d964`) — async lambda
  closure lifetime: audit verdict is "no current
  regression". The FB-30 problem text pointed at a
  `closure: current_env.bindings.clone()` path that the
  current `src/interpreter/eval.rs:1354` has replaced with
  `closure: Arc::new(self.env.snapshot())`. A sync
  regression suite lands as
  `tests/c25b_016_async_lambda_closure_lifetime.rs` (three
  tests: three-level lambda captures, lambda returned
  through a BuchiPack, separate snapshots do not alias) so
  that a future async redesign that regresses Arc-snapshot
  semantics is caught.

### Deferred NET stable viewpoint (why `rc7` and not label-less `@c.25`)

Several NET-adjacent items will remain open at the end of this
track and must be closed in a follow-up RC cycle before the
label-less `@c.25` tag is appropriate:

- **HTTP/2 parity across interpreter / native / wasm** — scatter-
  gather response handling, flow-control edge cases, and real-
  world client conformance not yet locked.
- **TLS construction** — cert chains / ALPN / verification modes
  that the current `taida-lang/net` facade covers only partially.
- **HTTP parity under port-bind race** — hyper / tokio port-bind
  failures under concurrent tests still rely on the retry shim.
- **Port-bind race eradication** — `flaky_h2_parity` is still
  papered over with a retry shim rather than eliminated
  (C25B-002 root-cause fix).
- **Throughput regression guard** — CI has no automated perf
  benchmark that blocks regressions; the Phase 2C scaffold
  (`benches/perf_baseline.rs`) runs warn-only.
- **Scatter-gather long-run** — the `httpServe` path has not been
  stressed with multi-hour runs yet.

These are tracked as C25B-002 (port-bind race) and surrounding
items in `.dev/C25_BLOCKERS.md`. The subsequent RC cycle will fold
the throughput gate (C25B-004 scope) alongside the NET stabilisation
work.

## @c.23.rc6

Single-scope follow-up that finishes the `Str[...]()` mold family
4-backend parity work flagged by the `@c.22.rc5` (PR #36) Codex review
as FB-34 and expanded by a follow-up audit into three root-cause
blockers. The Taida surface is unchanged — `Str[...]()` still returns
`Lax[Str]`, the Lax / ぶちパック contracts are intact, and the
interpreter's `Str[x]()` semantics stay put as the source of truth.
What moves is the JS / Native / WASM-wasi rendering of non-primitive
`Str[x]()` results plus the WASM primitive/Lax display, all of which
had been drifting away from the interpreter.

### Interpreter is still the reference; the other three backends now line up behind it

- **C23B-001 (WASM primitive / Lax `Str[...]()`) — FIXED**:
  `src/codegen/runtime_core_wasm/02_containers.inc.c` now has
  `_taida_float_to_str_mold`, which strips the trailing `.0` that
  `taida_float_to_str` emits for integer-valued floats. That
  matches the interpreter's `f.to_string()` contract
  (`Str[3.0]() -> "3"` / `Str[-5.0]() -> "-5"`) without disturbing
  the `Value::to_display_string`-style `"3.0"` rendering used by
  `stdout(Float)`. `src/codegen/runtime_core_wasm/01_core.inc.c` also
  adds a `WASM_TAG_STR` arm to `_wasm_pack_to_string` and
  `_wasm_pack_to_string_full`, so the Lax's `__value` / `__default`
  fields on a `Str` mold now render as explicitly-quoted char* (`""`,
  `"abc"`) instead of falling back to the `_wasm_value_to_debug_string`
  integer-pointer path, which previously leaked raw data-section
  offsets (`1250`, `1254`, etc.) into the output.
- **C23B-002 (Native / WASM non-primitive `Str[...]()`) — FIXED**:
  `src/codegen/lower_molds.rs` no longer falls through to
  `taida_str_mold_int` for anything that is not a compile-time-known
  Float / Str / Bool / Int literal. Non-primitive arguments (List /
  ぶちパック / Lax / Result / …) now route through a new generic
  `taida_str_mold_any`, defined in
  `src/codegen/native_runtime/core.c` and
  `src/codegen/runtime_core_wasm/02_containers.inc.c`. The helper
  re-uses the existing `taida_stdout_display_string` /
  `_wasm_stdout_display_string` entries — the same full-form pack
  rendering that `stdout` produces — so
  `Str[@[1,2,3]]() -> "@[1, 2, 3]"`,
  `Str[@(a <= 1)]() -> "@(a <= 1)"`, and
  `Str[Int[3.0]()]() -> "@(hasValue <= true, __value <= 3, __default <= 0, __type <= "Lax")"`
  line up exactly with the interpreter. No new magic bits; all tag
  plumbing (`TAIDA_TAG_STR` / `WASM_TAG_STR` on `__value` / `__default`
  via `_lax_tag_vd` / `taida_lax_tag_value_default`) is the same
  mechanism `C21B-seed-07` introduced.
- **C23B-003 (JS `Str_mold`, with reopen) — FIXED**:
  `src/js/runtime/core.rs` no longer uses `String(value)` in `Str_mold`.
  A new `__taida_display_string` helper mirrors the interpreter's
  `Value::to_display_string()` contract case-by-case — `Int`, `Float`,
  `Bool` use the natural JS number / boolean formatting; arrays render
  as `@[...]`; plain objects render as `@(field <= value, ...)` skipping
  `__` internals; typed packs (Lax / Result / Async) render their full
  form including `__`-prefixed internals and, for Lax, honour the
  `__floatHint` that C21B-seed-04 / C21B-seed-07 propagate for Float-
  origin bindings. `Str[@(a <= 1)]()` and `Str[Int[3.0]()]()` now
  match the other three backends byte-for-byte instead of rendering
  `[object Object]` or the short-form `Lax(3)`.

  The reopen resolution extends `__taida_display_string` to every
  `__type`-carrying runtime object (HashMap / Set / Stream / TODO /
  Gorillax / RelaxedGorillax / Molten) so their interpreter-shaped
  display strings stay intact instead of falling through to the
  plain-pack branch (which was leaking JS method source bodies as
  pack fields). `Str[hashMap().set("a", 1)]()` and `Str[setOf(@[1, 2, 3])]()`
  now produce the interpreter's `BuchiPack(__entries/__items, __type)`
  full form on JS. The same reopen adds the two missing native / wasm
  pieces to keep 4-backend parity end-to-end:

  * `src/codegen/native_runtime/core.c` gains
    `taida_hashmap_to_display_string_full` /
    `taida_set_to_display_string_full` and routes
    `taida_stdout_display_string` through them, so
    `Str[hashMap()...]()` / `Str[setOf(...)]()` no longer yield the
    short-form `HashMap({"a": 1})` / `Set({1, 2, 3})` (the
    `.toString()` format). It also registers the `__error` field name
    (which `taida_pack_to_display_string_full` needs for Gorillax
    packs), triggers the registration from `taida_gorillax_new` /
    `taida_gorillax_err`, stamps the Gorillax `__error` slot with
    `TAIDA_TAG_PACK`, and teaches the full-form helper to render a
    PACK-tagged `0` slot as `@()` (interpreter
    `Value::Unit.to_debug_string` parity). `Str[Gorillax[v]()]()` now
    renders `@(hasValue <= true, __value <= v, __error <= @(),
    __type <= "Gorillax")` on native instead of collapsing to `@()`.
  * `src/codegen/runtime_core_wasm/01_core.inc.c` mirrors the native
    change with `_wasm_hashmap_to_display_string_full` /
    `_wasm_set_to_display_string_full` and routes
    `_wasm_stdout_display_string` through them.

  Wasm Gorillax still stores `isOk` (not `hasValue`) in its first
  pack slot — a pre-existing divergence tracked as **C23B-004**
  (separate track after `@c.23.rc6`) and explicitly scoped out of
  the 4-backend fixture via `WASM_SKIP_FIXTURES` in
  `tests/c23_str_parity.rs`. Stream molds are interpreter + JS only
  (native / wasm lowering unsupported) and gated via
  `STREAM_ONLY_FIXTURES` in the same test.

- **C23B-003 reopen 2 (nested typed runtime object recursion) — FIXED**:
  The first reopen addressed the top-level rendering of HashMap / Set
  / Gorillax but left the *nested* render paths using the short-form
  debug helper. `hashMap().set("k", hashMap().set("a", 1))` and
  friends therefore collapsed the inner HashMap to
  `HashMap({"a": 1})` on every non-interpreter backend. The reopen-2
  fix teaches every full-form helper to recurse through itself when
  descending into field / entry values:

  * `src/js/runtime/core.rs` — when `__taida_format` encounters a
    `__type`-tagged object it now delegates to `__taida_display_string`,
    which already has the per-`__type` full-form dispatch. Previously
    `__taida_format` fell through to `String(v)` and called the
    runtime object's short-form `.toString()` prototype.
  * `src/codegen/native_runtime/core.c` — new
    `taida_value_to_debug_string_full` mirrors the short-form
    `taida_value_to_debug_string` but dispatches HashMap / Set /
    BuchiPack to their full-form helpers, then the existing
    `taida_hashmap_to_display_string_full`,
    `taida_set_to_display_string_full`, and
    `taida_pack_to_display_string_full` call the new variant instead
    of the short-form helper for their nested values. A List branch
    is also added to `taida_stdout_display_string` so top-level
    `Str[@[hashMap()...]]()` uses the new variant on its items.
  * `src/codegen/runtime_core_wasm/01_core.inc.c` — symmetric changes
    add `_wasm_value_to_debug_string_full`, route the three
    `_wasm_*_display_string_full` helpers through it, and give
    `_wasm_stdout_display_string` a List branch that does the same.

  The interpreter reference (`Value::BuchiPack.to_display_string()`
  walks field values via `to_debug_string()` which recurses back
  into `to_display_string()` for non-Str values) is now honoured
  identically on all four backends for nested HashMap-in-HashMap,
  Set-in-HashMap, List-of-HashMap, and BuchiPack-carrying-HashMap.

- **C23B-003 reopen 3 (WASM empty pack `@()` detection) — FIXED**:
  The reopen-2 fix taught every full-form helper to recurse, but the
  wasm `_wasm_value_to_debug_string_full` still gated pack rendering
  behind `_looks_like_pack`, which requires `field_count >= 1` to
  avoid false-positives against List / HashMap / Set header layouts.
  Empty packs (`@()` / `Value::Unit`) allocated by
  `taida_pack_new(0)` carry `field_count == 0` and therefore fell
  through to the integer fallback — rendering a raw heap pointer
  (e.g. `73088`) instead of `"@()"`. Native never hit this because
  its `TAIDA_PACK_MAGIC` header lets `taida_is_buchi_pack` match
  `fc == 0` directly; JS and the interpreter already rendered empty
  objects as `@()`. The reopen-3 fix adds a dedicated wasm detector:

  * `src/codegen/runtime_core_wasm/01_core.inc.c` gains
    `_looks_like_empty_pack`, which accepts any pointer that (a) is
    in the bump-allocator's live range (`__heap_base <= addr <
    bump_ptr` — the same invariant `_wasm_is_string_ptr` uses), (b)
    reads a single zero int64_t, and (c) is not simultaneously a
    List / HashMap / Set header. The bump-range guard is essential:
    a pure memory-peek detector false-positives on small integer
    outputs such as `5050` (tail-recursion Fibonacci) and `8080` (in
    `localhost:8080` string interpolation), both caught by
    `tests/wasm_full.rs::wasm_full_parity_all_examples` while
    developing the fix. The detector is wired into four display
    helpers — `_wasm_value_to_debug_string_full` (nested pack fields
    / hashmap entries / set items), `_wasm_value_to_debug_string`
    (short-form fallback), `_wasm_value_to_display_string`
    (short-form display fallback), and `_wasm_stdout_display_string`
    (top-level `stdout(@())`). Each check sits between the richer
    detectors (`_looks_like_pack`, `_is_wasm_hashmap`, `_is_wasm_set`)
    and the raw-pointer fallback, so existing typed-pack rendering
    is never shadowed and integer pointers never win over the empty-
    pack rendering.

  `Str[@()]()`, `Str[@(u <= @())]()`,
  `Str[hashMap().set("u", @())]()`, and `Str[@[@()]]()` now all emit
  the interpreter string on wasm. `stdout(@())` also renders `@()`
  directly instead of the bump-allocator offset.

- **C23B-003 reopen 4 (WASM tag-based empty-pack identification + richer compile-time Int check) — FIXED**:
  The reopen-3 detector used a heap-range + zero-slot heuristic
  (`__heap_base <= addr < bump_ptr && *(int64_t*)addr == 0`) to
  recognise empty packs. A later review surfaced a HIGH-severity
  false-positive: dynamic Int expressions routed through the generic
  `Str[...]()` path (`taida_str_mold_any`) hand untagged int64_t
  values to the display helpers, and if the integer happens to fall
  inside the bump arena on an 8-byte-aligned zero chunk, the
  heuristic fires and renders the integer as `"@()"`. The canonical
  repro — `a <= 36000; b <= 37088; stdout(Str[a + b]())` — emitted
  `__value <= "@()"` on wasm where interpreter / JS / native all emit
  `"73088"`. The reopen-4 fix replaces the heuristic with a
  positive-identification magic sentinel and hardens the lowering
  side so as many dynamic Int shapes as possible never reach the
  runtime heuristic:

  * **Tag-based identification.**
    `src/codegen/runtime_core_wasm/01_core.inc.c` `taida_pack_new(0)`
    now allocates two int64_t slots instead of one, writing
    `[field_count=0, WASM_EMPTY_PACK_MAGIC]` where
    `WASM_EMPTY_PACK_MAGIC = 0x5441494450414B55LL` (the seven-byte
    printable string "TAIDPAKU" — TAIDA + PACK + Unit). Non-empty
    packs are unchanged — their `field_count >= 1` already
    disambiguates them, and `pack[1]` continues to hold
    `field0_hash`. `_looks_like_empty_pack` now tests exactly
    `data[0] == 0 && data[1] == WASM_EMPTY_PACK_MAGIC` (plus wasm32
    pointer + 8-byte-alignment guards). The heap-range / bump_ptr /
    List-HashMap-Set negation checks are gone — integer values can
    no longer false-match the detector no matter what is sitting in
    memory at that offset.
  * **Compile-time Int fast-path widening.**
    `src/codegen/lower_molds.rs` `Str` dispatch now consults
    `Lowering::expr_is_int` (from `src/codegen/lower/infer.rs`)
    instead of a local syntactic `expr_is_int_literal`. That
    recognises int-typed bindings via `int_vars`, arithmetic on
    Int operands via `BinaryOp::{Add,Sub,Mul}`, int-returning
    methods (`length` / `indexOf` / `lastIndexOf` / `count`), and
    int-returning user functions via `int_returning_funcs`. Those
    shapes now short-circuit to `taida_str_mold_int` at compile
    time, so the dynamic-Int values never enter the runtime
    heuristic stack at all — defence-in-depth on top of the
    detector-level fix. `expr_is_int`'s visibility widens from
    `pub(super)` to `pub(crate)` (no logic change) so the sibling
    `lower_molds.rs` module outside the `lower/` submodule can
    call it.

  Four regression fixtures pin the fix:
  `str_from_dynamic_int` (`Str[a + b]()` with `a + b = 73088`),
  `str_from_dynamic_int_zero` (`Str[5 - 5]()`),
  `str_from_dynamic_int_negative` (`Str[-x]()` on an int_var), and
  `str_from_dynamic_int_funcall` (`Str[double(36544)]()` with
  `double n = n * 2 => :Int`). All four now render the integer
  value on every backend byte-for-byte. JS / Native were already
  correct (JS uses `__type`-based dispatch, Native uses
  `TAIDA_PACK_MAGIC` for empty packs — neither relies on address
  heuristics), so the reopen-4 work is wasm-scoped.

  Follow-up audit during the reopen-4 work uncovered a separate
  pre-existing wasm heuristic bug — `_looks_like_list` accepts any
  pointer whose `data[0]` (cap) is in `8..=65536` and `data[1]`
  (len) is in `0..=cap`, which false-matches on Int values such as
  73088 stored uninterpretted inside a List / ぶちパック. That
  stack-overflows `Str[@[73088]]()` and friends on wasm. Originally
  filed as **C23B-005 (TRACKED)** for a later release track.

- **C23B-005 reopen + widen + C23B-006 (WASM collection detectors
  false-positive on untagged large Ints) — FIXED**: a deeper audit
  during the @c.23.rc6 review showed the bug class extended far
  beyond `_looks_like_list`: `_is_wasm_hashmap` relied on a 4-byte
  `"HMAP" = 0x484D4150` marker at `data[3]` and would recursively
  re-render any HashMap value slot holding a large Int (e.g.
  `hashMap().set("x", 73088)` collapsed 73088 to an empty HashMap),
  and `_is_wasm_set` layered on top of `_looks_like_list` and
  inherited the same stack-overflow. All four WASM collection
  detectors (`_looks_like_list`, `_is_wasm_set`, `_is_wasm_hashmap`,
  `_looks_like_pack`) have been unified onto a single tag-based
  positive-identification scheme:

  * **Wide 8-byte printable-ASCII magic sentinels** replace every
    prior structural heuristic (cap-range for List, 4-byte
    `"HMAP"` / `"SET\0"` for HashMap / Set, `fc-range + first_hash`
    for Pack). The new constants — `WASM_LIST_MAGIC` ("TAIDLST"),
    `WASM_SET_MAGIC` ("TAIDSET"), `WASM_HM_MAGIC` ("TAIDHMP"),
    `WASM_PACK_MAGIC` ("TAIDPKK") — are stamped by the
    corresponding allocation paths (`taida_list_new`,
    `taida_set_new`, `taida_hashmap_new` /
    `_wasm_hashmap_new_with_cap` / `taida_hashmap_set` resize,
    `taida_pack_new(fc>=1)`) and carry 64 bits of entropy so they
    cannot arise from user arithmetic.
  * **Dual-magic identification** for List / Set / HashMap. Every
    allocation stamps the magic at BOTH a head position (`data[3]`)
    AND a shape-dependent trailing position (`data[WASM_LIST_ELEMS
    + cap]` for lists and sets, `data[WASM_HM_HEADER + cap * 3]`
    for hashmaps). Detectors verify both, giving 128 bits of
    entropy. This closes the last residual attack path where a
    user-supplied large Int stored inside one collection's value
    slot happened to equal the base pointer of a *different*
    collection in the same bump arena — single-magic identification
    would still succeed because that real collection carries the
    head magic legitimately, but the trailing magic at the
    cap-dependent offset cannot simultaneously align for both the
    fake (integer as pointer) and real (pointer to collection)
    interpretations. Pack uses a single trailing magic plus
    `data[0]` in `1..=100`; the `fc`-range constraint supplies the
    provenance the dual scheme would otherwise provide.
  * **Tag-aware element rendering** (`_wasm_render_elem_tagged_debug`
    and `_wasm_render_elem_tagged_debug_full`). The list's
    `elem_type_tag` (slot 2), the hashmap's `value_type_tag`
    (slot 2), and the pack's per-field `field_tag` are threaded
    into every collection-member rendering loop. Primitive Int /
    Float / Bool / Str members dispatch directly via the tag
    without going through any structural detector — defence-in-depth
    on top of the dual-magic check. `taida_hashmap_set_value_tag`
    and `taida_list_set_elem_tag` are hardened to downgrade the
    stored tag to UNKNOWN (-1) on type conflict (e.g.
    `hashMap().set("name", "Asuka").set("age", 14)` now leaves
    `hm[2] = -1` instead of silently overwriting with the last
    inserted value's type). Heterogeneous containers fall back to
    structural dispatch, which is safe thanks to the dual-magic
    detectors.
  * **`_is_wasm_hashmap` / `_is_wasm_set` in
    `src/codegen/runtime_full_wasm.c`** were mirroring the old
    4-byte markers and have been retargeted to the same wide
    sentinels so the wasm-full profile matches the wasm-wasi
    behaviour. Forward declarations for `_wf_is_valid_ptr` and the
    magic constants appear early in the file so the detector
    helpers can reference them without reordering.
  * **`_wc_is_hashmap` / `_wc_is_set` in
    `src/codegen/runtime_core_wasm/04_json_async.inc.c`** now
    delegate to the hardened `_is_wasm_hashmap` / `_is_wasm_set`
    in fragment 1, eliminating the duplicate (and now-stale)
    4-byte marker checks that JSON / async code previously relied
    on.

  Five regression fixtures pin the fix across all four backends:
  `str_from_hashmap_with_large_int` (C23B-006 direct repro),
  `str_from_set_with_large_int` (`setOf(@[73088])` stack overflow
  repro), `str_from_list_with_large_int` (`Str[@[73088, 42000]]()`),
  `str_from_pack_with_large_int` (`Str[@(x <= 73088)]()`), and
  `str_from_nested_collection_with_large_int` (nested: HashMap
  containing `@[73088]`). Every backend (Interpreter / JS / Native /
  WASM-wasi) renders them byte-for-byte identically. JS and Native
  were already correct (JS uses `__type`-based dispatch, Native
  uses `TAIDA_PACK_MAGIC` / `TAIDA_LIST_MAGIC` / `TAIDA_HMAP_MAGIC`
  positive identification that already satisfies the provenance
  requirement), so the reopen lands wasm-scoped.

  > **Why not per-entry type tags instead?** Adding per-entry tags
  > to HashMap would require a layout change and cascade through
  > every reader in the runtime. The dual-magic approach achieves
  > equivalent safety without touching the hot-path load of
  > `hm[WASM_HM_HEADER + slot * 3 + 2]`. Tag-aware rendering on top
  > reuses the already-tracked per-container elem / value / field
  > tags that the lowering installs for homogeneous containers,
  > giving us the fast path without the layout cost.

- **C23B-007 (WASM tag re-promotion into heterogeneous containers)
  — FIXED**: the `taida_list_set_elem_tag` /
  `taida_hashmap_set_value_tag` downgrade path that C23B-005 installed
  reused the UNKNOWN (-1) sentinel to mean both "not set yet" and
  "downgraded from type conflict". That made the downgrade reversible:
  a subsequent `.push()` / `.set()` carrying a fresh primitive tag
  would see `existing == -1`, treat the container as unset, and
  re-promote it to that new tag. The renderer would then force every
  member through that tag's fast path — strings emerged as raw
  pointer integers (`@[1, "a", 2]` → `@[1, 1127, 2]`), string-valued
  HashMap entries showed as pointer Ints (`.set("a", 1).set("b",
  "x").set("c", 2)` → `value <= 1058` for "x"). Resolution:

  * Split the sentinel. `WASM_TAG_HETEROGENEOUS = -2` (and symmetric
    `TAIDA_TAG_HETEROGENEOUS = -2` on native) joins `WASM_TAG_UNKNOWN
    = -1` as a terminal state. `taida_list_set_elem_tag` /
    `taida_hashmap_set_value_tag` now follow the four-case latch:
    `HETEROG → keep`, `UNKNOWN → stamp`, `equal → no-op`,
    `different → HETEROG`. Once HETEROG, never re-promote.
  * Renderers do not need changes: `_wasm_render_elem_tagged_debug` /
    `_full` already fall through to the structural dispatcher
    (`_wasm_value_to_debug_string(_full)`) for any non-primitive
    tag, so the new -2 lands on the per-element structural path
    automatically. Native `taida_list_to_display_string` /
    `taida_hashmap_to_display_string_full` already use structural
    per-element dispatch.
  * `src/codegen/lower/expr.rs::lower_list_lit` had been stamping the
    element tag only once, trusting a "checker guarantees homogeneity"
    comment that no longer held (the interpreter accepts
    `@[1, "a", 2]` verbatim). We now call `taida_list_set_elem_tag`
    for every element. Homogeneous list literals still converge to
    the primitive tag on the first call (subsequent calls are no-ops);
    heterogeneous literals latch to HETEROG as soon as the second
    disagreeing tag appears.

  Four fixtures pin the repros and adjacent shapes:
  `str_from_mixed_list` (the C23B-007 direct List repro),
  `str_from_mixed_hashmap` (the HashMap three-value-type variant),
  `str_from_mixed_set` (Sets share the list header, so the fix
  applies), and `str_from_nested_mixed` (outer heterogeneous list
  wrapping an inner heterogeneous list — both levels latch
  independently). All four render byte-for-byte identically on
  Interpreter / JS / Native / WASM-wasi.

- **C23B-008 (Multi-entry HashMap display emits bucket order, not
  insertion order) — FIXED**: `taida_hashmap_to_display_string_full`
  (native) and `_wasm_hashmap_to_display_string_full` (wasm) iterated
  the open-addressing bucket array. Interpreter and JS represent
  HashMap as a `Vec<(k, v)>` (interpreter) / `Array` (JS) of insertion
  order, so `hashMap().set("a", 1).set("b", 2)` came out as "b", "a"
  on native / wasm. The same drift affected `taida_hashmap_entries`,
  `taida_hashmap_keys`, `taida_hashmap_values`, `taida_hashmap_merge`,
  `taida_hashmap_to_string` (short form), the native JSON serializer
  `json_serialize_typed`, and the wasm JSON `_wc_json_serialize_typed`
  — anywhere iteration order was observable. Resolution:

  * Both runtimes now append an insertion-order side-index to every
    HashMap allocation: `[next_ord, order_array[cap]]`. `next_ord` is
    a monotonic ordinal counter (never decremented, even after
    `.remove()`). `order_array[i]` stores the bucket slot of the i-th
    insertion, or `-1` for a hole left by `.remove()`. The layout is
    appended after the trailing magic (wasm) or the entry array
    (native); existing header / entry offsets are unchanged, so the
    dual-magic detection from C23B-005 / C23B-006 keeps working
    unchanged.
  * New offset macros centralise the math:
    `WASM_HM_ORD_HEADER_SLOT` / `WASM_HM_ORD_SLOT` on the wasm side,
    `TAIDA_HM_ORD_HEADER_SLOT` / `TAIDA_HM_ORD_SLOT` /
    `TAIDA_HM_TOTAL_SLOTS` on native. `_wasm_hashmap_new_with_cap` /
    `taida_hashmap_new_with_cap` bump their allocations by `1 + cap`
    slots, calloc-zero; `next_ord` starts at 0.
  * `taida_hashmap_set` records `order_array[next_ord++] = slot` on
    a new insertion; updates of an existing key leave the ordinal
    untouched (first-insertion-wins, matching the interpreter's
    `Vec` update). The tombstone-reuse path records the ordinal the
    same way. `taida_hashmap_remove` nulls the matching
    `order_array[i]` slot to `-1`; `next_ord` stays put.
  * `taida_hashmap_resize` now walks the OLD order array in
    insertion order and re-inserts each surviving entry into the
    new table, rebuilding the new side-index as it goes.
    `taida_hashmap_set_internal` (native) returns the new bucket
    slot (or `-1` for an update in place) so the caller can record
    the ordinal. The wasm `taida_hashmap_set` inline resize path
    applies the same order-preserving rebuild.
  * `taida_hashmap_clone` grows its allocation to include the new
    side-index and copies it verbatim (same bucket layout → same
    slot indices remain valid).
  * Display / iteration helpers on both runtimes —
    `taida_hashmap_to_display_string_full`,
    `taida_hashmap_to_string`, `taida_hashmap_entries`,
    `taida_hashmap_keys`, `taida_hashmap_values`,
    `taida_hashmap_merge`, plus the native / wasm JSON serializers
    — now walk `order_array[0..next_ord]` and skip holes /
    tombstoned buckets.
  * JS required no runtime change: `__taida_createHashMap` already
    stored `__entries` as an Array (insertion-ordered by
    construction), and `__taida_display_string` /
    `__taida_format` walked it in Array order.
  * Interpreter is the source of truth and was untouched.

  Four fixtures pin the behaviour:
  `str_from_multi_entry_hashmap` (the direct two-entry repro),
  `str_from_large_hashmap` (16 keys, crosses the wasm 70% / native
  75% resize boundary — proves the resize rebuild preserves
  insertion order), `str_from_hashmap_after_remove` (remove the
  middle entry, verify the hole is skipped and the surrounding
  entries stay in their original order), and
  `str_from_hashmap_update_preserves_order` (re-`.set()` an existing
  key, verify the ordinal stays put so the final order is still
  first-insertion-wins).

- **C23B-008 reopen (HashMap.merge() overlap-key ordinal divergence)
  — FIXED**: the reopen-5 fix above pinned HashMap display / iteration
  to insertion order, but a follow-up review (reopen-7) showed that
  `.merge()` itself still diverged from the interpreter on overlap
  keys. The interpreter (`src/interpreter/methods.rs:787-822`) does
  `merged.retain(|e| e.key != other_key); merged.push(other_entry)`
  for each `other` entry, which MOVES every overlap key to other's
  position with other's value. JS called `merged[idx] = oe`, and
  native / wasm cloned self then called `taida_hashmap_set` per
  `other` entry — both variants update in place and preserve self's
  ordinal for overlap keys. Repro: self = `[a, b]`, other = `[c, b, d]`;
  interpreter emits `[a, c, b, d]`, the three broken backends emitted
  `[a, b, c, d]`. Resolution:

  * All three backends (native / wasm / JS) now build a fresh result
    map and fill it in the order interpreter would emit — step 1
    walks self in self-order and copies entries whose key is absent
    from other; step 2 walks other in other-order and appends every
    entry (all guaranteed new to the fresh map). Value retention
    flows through the normal `taida_hashmap_set` / `__taida_createHashMap`
    code paths.
  * `taida_hashmap_set` is NOT modified — its update-in-place ordinal
    preservation is still required for plain `.set("k", v)` chains
    (interpreter's `Vec<(k, v)>` update semantics keep the position
    of a re-`.set()` key). Only `.merge()` needed the retain-then-push
    variant, and it achieves that by avoiding `.set()` on overlap
    keys entirely (they never reach the fresh result as self entries).
  * Six fixtures pin the behaviour:
    `str_from_hashmap_merge_overlap` (the direct review repro —
    `a=[a,b].merge([c,b,d])` ⇒ `[a,c,b,d]`),
    `str_from_hashmap_merge_non_overlap` (degenerate no-overlap path,
    self-order + other-order),
    `str_from_hashmap_merge_full_overlap` (every self key in other,
    result = other in other-order with other's values),
    `str_from_hashmap_merge_empty_self` (empty self ⇒ result = other),
    `str_from_hashmap_merge_empty_other` (empty other ⇒ result =
    self), `str_from_hashmap_merge_resize` (16-entry merge with two
    overlap keys crossing the 0.75 load-factor resize on the fresh
    result map — exercises `taida_hashmap_resize` + side-index
    rebuild during the fill loop).
  * The reopen-7 audit also flagged an independent divergence in
    `HashMap.entries()` across backends — interpreter returns
    `@(key <= …, value <= …)`, JS returns `@(first <= …, second <= …)`
    (wrong field names), and native / wasm render the pair pack as
    empty `@()` (pair content does not reach the renderer). This is
    tracked as **C23B-009** and resolved inside the same release (see
    below).

- **C23B-009 (HashMap.entries() 4-backend divergence) — FIXED**: an
  independent divergence from the reopen-7 audit. The documented
  contract (`docs/reference/standard_library.md:238`) and the
  interpreter (`src/interpreter/methods.rs:761-783`) both use
  `@[@(key, value)]` for `.entries()`. Previously:

  * **JS** (`src/js/runtime/core.rs:2555` `entries()`): emitted
    `Object.freeze({ first: e.key, second: e.value })` — a legacy
    convention inadvertently shared with `zip()` / `Zip[]()` (those
    stay `first`/`second` because the interpreter itself does; only
    `.entries()` was wrong). Fix: rename to `{key, value}`. The
    `hashMap(entries)` constructor (line ~2600) still accepts
    `.first` / `.second` fallback for back-compat with user-built
    pair lists.
  * **Native** (`src/codegen/native_runtime/core.c::taida_hashmap_entries`):
    stamped the correct FNV-1a hashes (`HASH_KEY` /
    `HASH_VAL`) and tags but never called
    `taida_register_field_name` for them. When
    `taida_pack_to_display_string_full` looked them up it got NULL
    and silently skipped every field, emitting `@()`. Fix: idempotent
    registration of `"key"` / `"value"` inside the entries helper
    (guarded by a static flag so the cost is paid once per program).
  * **WASM** (`src/codegen/runtime_core_wasm/01_core.inc.c::taida_hashmap_entries`):
    symmetric to native's missing registry entry — plus the wasm
    implementation didn't stamp per-field tags on the pair pack, so
    even if the names had been found the values would have rendered
    through the untagged fallback path. Fix: register `"key"` /
    `"value"` idempotently, stamp `WASM_TAG_STR` on `key`, propagate
    the hashmap's `value_type_tag` onto `value`, and flag the outer
    list's `elem_type_tag = WASM_TAG_PACK` so every pair dispatches
    through `_wasm_render_elem_tagged_debug_full`.
  * **Interpreter**: unchanged (source of truth).

  Repro:
  ```taida
  m <= hashMap().set("a", 1).set("b", 2)
  stdout(Str[m.entries()]())
  ```
  All four backends now emit
  `@(hasValue <= true, __value <= "@[@(key <= "a", value <= 1), @(key <= "b", value <= 2)]", __default <= "", __type <= "Lax")`.

  The shape is non-breaking: JS's `{first, second}` field names were
  never part of the documented API, and the hashMap() constructor's
  back-compat fallback still reads them so user code that built pair
  lists via `@(first <= k, second <= v)` continues to work. Pre-fix
  audit covered `.keys()`, `.values()`, `.size()`, `.has()`,
  `.merge()`, `Set.toList()`, `List.first()`, `zip()` / `Zip[]`, and
  `enumerate()` — all parity-clean on interpreter/JS; `zip()` /
  `enumerate()` silently return empty on native / wasm (pre-existing
  divergence, not on the `Str[...]()` family path, outside the C23B
  scope).

### Regression guard

- `tests/c23_str_parity.rs` drives forty-six fixtures under
  `examples/quality/c23b_str_parity/` — `str_from_float_int_form`,
  `str_from_float_frac_form`, `str_from_bool`, `str_from_str`,
  `str_from_list`, `str_from_pack`, `str_from_lax`, the
  C23B-003-reopen additions `str_from_hashmap`, `str_from_set`,
  `str_from_gorillax`, `str_from_stream`, the reopen-2
  additions `str_from_nested_hashmap`, `str_from_nested_set`,
  `str_from_list_of_hashmap`, `str_from_pack_with_hashmap`, the
  reopen-3 additions `str_from_empty_pack`,
  `str_from_pack_with_empty_pack`, `str_from_hashmap_with_empty_pack`,
  `str_from_list_with_empty_pack`, the reopen-4 additions
  `str_from_dynamic_int`, `str_from_dynamic_int_zero`,
  `str_from_dynamic_int_negative`, `str_from_dynamic_int_funcall`,
  the C23B-005 reopen + C23B-006 additions
  `str_from_hashmap_with_large_int`, `str_from_set_with_large_int`,
  `str_from_list_with_large_int`, `str_from_pack_with_large_int`,
  `str_from_nested_collection_with_large_int`, the C23B-007
  additions `str_from_mixed_list`, `str_from_mixed_hashmap`,
  `str_from_mixed_set`, `str_from_nested_mixed`, and the C23B-008
  additions `str_from_multi_entry_hashmap`, `str_from_large_hashmap`,
  `str_from_hashmap_after_remove`,
  `str_from_hashmap_update_preserves_order`, and the C23B-008
  reopen-7 merge-semantics additions
  `str_from_hashmap_merge_overlap`, `str_from_hashmap_merge_non_overlap`,
  `str_from_hashmap_merge_full_overlap`,
  `str_from_hashmap_merge_empty_self`,
  `str_from_hashmap_merge_empty_other`,
  `str_from_hashmap_merge_resize`, and the C23B-009
  `.entries()` additions `str_from_hashmap_entries`,
  `str_from_hashmap_entries_empty`,
  `str_from_hashmap_entries_single`,
  `str_from_hashmap_entries_after_remove` — through all four
  backends where each backend supports the mold. The interpreter
  fixture test also pins the `.expected` files so a future
  interpreter change cannot silently drift them away from the
  source of truth. `WASM_SKIP_FIXTURES` excludes
  `str_from_gorillax` from the wasm run (tracked as C23B-004) and
  `STREAM_ONLY_FIXTURES` excludes `str_from_stream` from both
  native and wasm (Stream lowering unsupported on those backends).

### Byte-length assertions

- `src/codegen/runtime_core_wasm/mod.rs` `EXPECTED_TOTAL_LEN`:
  248,033 → 251,707 → 254,479 → 259,848 → 265,494 → 267,429 →
  283,669 → 292,933 → 293,560 → 295,319 (+3,674 initial, +2,772 in
  C23B-003 reopen, +5,369 in C23B-003 reopen 2, +5,646 in C23B-003
  reopen 3, +1,935 in C23B-003 reopen 4, +16,240 for C23B-005 reopen
  + C23B-006, +9,264 for C23B-007 + C23B-008, +627 for C23B-008
  reopen-7: rewrote `taida_hashmap_merge` from clone-then-set to
  the interpreter's retain-then-push algorithm — fresh result map,
  self entries whose key ∉ other in self-order, then every other
  entry in other-order; +1,759 for C23B-009: wasm
  `taida_hashmap_entries` now idempotently registers `"key"` /
  `"value"` in `_wasm_field_registry`, stamps per-field tags
  (`WASM_TAG_STR` on key, hashmap value_type_tag on value) and
  flags the returned list's `elem_type_tag = WASM_TAG_PACK`).
- `src/codegen/native_runtime/mod.rs` `EXPECTED_TOTAL_LEN`:
  935,805 → 936,859 → 943,160 → 950,197 → 958,672 → 960,607 → 961,515
  (+1,054 initial, +6,301 in C23B-003 reopen, +7,037 in C23B-003
  reopen 2 for `taida_value_to_debug_string_full` + List dispatch
  in `taida_stdout_display_string`, +8,475 for C23B-007 + C23B-008,
  +1,935 for C23B-008 reopen-7: the native `taida_hashmap_merge`
  symmetric rewrite; +908 for C23B-009: native
  `taida_hashmap_entries` idempotent registration of `"key"` /
  `"value"` via `taida_register_field_name`).
  `F1_LEN` moves 218,772 → 226,482 → 228,417 → 229,325 (+7,710 for
  C23B-007 / C23B-008 absorbing the `TAIDA_TAG_HETEROGENEOUS`
  define + `TAIDA_HM_ORD_*` macros + the `set_elem_tag` /
  `set_value_tag` latch bodies + the HashMap insertion-order
  scaffolding in `_new_with_cap` / `_set` / `_resize` / `_remove` /
  `_clone` / `_keys` / `_values` / `_entries` / `_merge` /
  `_to_string`, plus +1,935 for C23B-008 reopen-7 on `_merge`, plus
  +908 for C23B-009 on `_entries`). F2 moves 150,412 →
  151,177 (+765, absorbing the
  `taida_hashmap_to_display_string_full` and
  `json_serialize_typed` HashMap walk switches).

### What we did NOT touch

- No change to `src/interpreter/mold_eval.rs`, `src/interpreter/eval.rs`,
  or the wider interpreter — C23 treats the interpreter as the
  reference and only modifies codegen / runtime helpers / JS runtime.
- No change to `src/types/mold_returns.rs`'s Pack classification
  (C21B-seed-07 land is left intact).
- No change to `src/codegen/driver.rs`, `src/codegen/lower/*.rs`
  (except `src/codegen/lower_molds.rs`, which is the only C23 scope
  file in that tree), or any C21 / C22 work-stream entry point.
- `.dev/official-package-repos/terminal/` is untouched.

## @c.22.rc5

Two concurrent tracks land together in a single RC bump. Track A
(formerly the `@c.21.rc4` draft) restores 3-backend Float semantics
and opens the WASM SIMD path. Track B (formerly the `@c.22.rc1`
draft) restores I/O symmetry and hardens the CLI against pipe-chain
termination. Both tracks share the same merge, so the RC counter
advances monotonically from `@c.20.rc4` to `@c.22.rc5` — the `22`
reflects the active generation, the `rc5` keeps the incremental
index `taida upgrade` and label-based selectors rely on. The two
drafts' change-notes are kept as subsections below for traceability.

### Track A — 4-backend Float parity + WASM SIMD path open

Restore 3-backend Float semantics and open the WASM SIMD path that
bonsai-wasm Phase 0 identified as a one-shot blocker for writing
matmul-shaped numeric code in Taida. The observable behaviour changes
cluster into four mutually-reinforcing fixes (C21-1 through C21-5) plus
the `-msimd128` profile split (C21-3) that this release tag is pinned
on. The Taida surface — `3.0` vs `3`, `Float[x]()` / `Int[x]()`,
`@[Float]`, `:Float`, arithmetic operators — is unchanged; every fix
lives inside codegen / runtime helpers.

### Interpreter is the reference, the other three backends line up behind it

- **C21-1 (regression guard, Phase 1)**: `examples/quality/c21b_float_fn_boundary/triple.{td,expected}`
  (`triple(4.0) => 12.0`) and `dot_product.{td,expected}`
  (`dotProductAt(@[1.0,2.0], @[3.0,4.0], 0, 2, 0.0) => 11.0`) pin the
  minimal Float-function-boundary behaviour across Interpreter / JS /
  Native / WASM-wasi with `tests/c21_float_fn_boundary.rs`. This fixture
  would have caught every Phase 2-5 bug before it reached bonsai-wasm.
- **C21-2 (Wasm Float hot loop, Phase 2)**: `@[Float]`-element
  `a.get(i) ]=> av` now propagates the element type through
  `track_unmold_type`, so `av * bv` lowers to `taida_float_mul` instead
  of `taida_int_mul`. Fixes the "internal dot product silently computes
  0" class of bug observed in bonsai-wasm's hot loop.
- **C21-4 (Float → Str ABI, Phase 4)**: the native Cranelift path
  bitcasts `ConstFloat` into the boxed `value_ty` on emit (fixes the
  `define_function failed: Compilation error: Verifier errors` that
  blocked every `=> :Float` function in native builds). The native
  `taida_float_to_str` now matches Rust's `f64::Display`
  (shortest-round-trip `%.*g` + `strtod` loop + integer-form `X.0`);
  both native and WASM `taida_io_stdout_with_tag` / `_stderr_with_tag`
  route the `FLOAT` tag through that formatter so `stdout(triple(4.0))`
  no longer leaks the i64 bit-pattern.
- **C21-5 (JS `Int[x]` / `Float[x]` parity, Phase 5)**: JS
  `Number.isInteger(3.0) === true` closed the door on the naive
  `typeof+isInteger` checker; we now carry a compile-time
  `is_float_origin_expr` / `is_int_origin_expr` analysis in
  `src/js/codegen.rs`, specialize `stdout` / `debug` / `stderr` /
  `.toString()` call sites for Float-origin arguments
  (`__taida_stdout_f`, `__taida_debug_f`, `__taida_to_string_f`,
  `__taida_float_render`) and fold `Int[floatLit]()` / `Float[intLit]()`
  statically. Arithmetic paths are untouched — zero deopt for the hot
  case, compile-time fold covers every literal / single-bind case that
  used to diverge from Interpreter / Native.

### `-msimd128` profile split (C21-3, this tag's pin)

`WASM_CLANG_FLAGS` was a single profile-agnostic constant that lacked
`-msimd128`. That silently closed the SIMD door at the clang layer for
every `wasm-*` target, so even after C21-2 taught the wasm codegen to
emit f64 Float operations LLVM's auto-vectorizer could not consider
`v128.*` lowerings. `src/codegen/driver.rs` now splits the flags:

- `WASM_CLANG_FLAGS_COMMON` (`--target=wasm32-unknown-wasi`, `-nostdlib`,
  `-O2`, `-c`) stays the same.
- `wasm_clang_flags_for(profile)` appends `-msimd128` for `Wasi`,
  `Edge`, `Full` — and **nothing** for `Min`, so consumers who pick
  `--target wasm-min` for minimal-runtime compatibility still get a
  `.wasm` that does not request the `simd128` feature.
- `WasmRuntimeCache` is profile-aware: `rt_core` / `rt_wasi` / `rt_edge`
  / `rt_full` now take a `WasmProfile`, their cache keys hash the
  per-profile flag vector so a wasm-min `rt_core.o` is never served to
  a wasm-wasi build (and vice versa), and the stale-entry cleanup
  preserves every live profile's key for the same source.

Result: on `examples/quality/c21b_wasm_simd/matmul_small.td`
(sum-of-squares of 8 Floats), the disassembled `.wasm` now contains
`v128.*` and `i8x16.*` instructions under `wasm-wasi` while `wasm-min`
stays at zero SIMD opcodes. `tests/c21_wasm_simd.rs` locks both
directions in place. On bonsai-wasm's `bench/matmul.td` smoke the same
shift is visible (v128 count: `0 → 27`, f64 count: `5 → 10`), which was
the goal that motivated C21 in the first place.

### Tests

- `tests/c21_float_fn_boundary.rs` — 8 cross-backend tests (Interpreter
  reference + JS / Native / WASM-wasi parity × 2 fixtures).
- `tests/c21_wasm_simd.rs` — 3 tests: `wasm-wasi` disassembly must
  contain Float ops and at least one SIMD-family opcode; `wasm-min`
  disassembly must contain zero SIMD opcodes; matmul_small runs
  correctly under wasmtime and prints `204.0`.
- `src/codegen/driver.rs::tests::test_cache_key_differs_on_source_change`
  gains a fourth key comparing `wasm-min` vs `wasm-wasi` with identical
  source + clang version, asserting that the profile-specific flag
  change alone produces a distinct cache key.

### Out of scope

- Taida-level `@[Float<f32>]` / `@[Float<f64>]` quantifier additions are
  a language-surface change and stay deferred.
- Manual `v128.*` intrinsic exposure from Taida source remains
  out-of-bounds (Taida-first design — auto-vectorize only).
- JS closure-crossing dynamic Float/Int discrimination is still
  best-effort; the compile-time analysis covers every single-bind case,
  dynamic `map`/`fold` callbacks fall back to `Number.isInteger`. This
  is a language-spec limitation of JS `Number`, not a C21 regression.

### Fixed

- JS local binding Float-origin propagation (C21B-seed-04 reopen,
  Phase 5 re-fix): the initial Phase 5 landing only covered terminal
  sites whose argument was a `FloatLit` / Float-origin arithmetic /
  `=> :Float` user-fn call. A subsequent review confirmed that
  `x <= 3.0; stdout(Float[x]())` / `stdout(x)` / `stdout(x.toString())`
  still diverged in JS because `is_float_origin_expr(Expr::Ident)`
  returned `false`. `src/js/codegen.rs` grows a scope-aware tracker
  (`float_origin_vars` / `int_origin_vars` / `float_list_vars`) that is
  pushed / popped across function boundaries. Typed parameters
  (`x: Float`, `a: @[Float]`), annotated bindings (`x: Float <= ...`),
  Float-origin RHS bindings (`x <= 3.0`, `x <= floatExpr`), and unmold
  targets rooted in Float lists (`a.get(i) ]=> av`) now carry the tag.
  `Float[x]()` routes to a new `Float_mold_f` runtime helper that tags
  the resulting `Lax` with `__floatHint: true`; the stdout / format
  path renders `__value` / `__default` through `__taida_float_render`
  when the tag is present. Arithmetic paths stay untouched (no deopt).
  `tests/c21_js_float_binding.rs` pins both REOPEN repros plus a
  one-level-deeper `triple(4.0) → local → Float[y]()` case.

- Native / WASM Float Lax parity for local bindings (C21B-seed-07,
  Phase 4 補修): the Phase 5 re-fix split out a pre-existing Native /
  WASM divergence — `x <= 3.0; stdout(Float[x]())` printed
  `3.958204945e-315` on native and subnormal garbage on wasm-wasi
  because `mold_returns.rs` declared `Float[x]()` as returning a bare
  `Float` (tag 1), so `lower_stdout_with_tag` routed the Lax pointer
  through the `TAIDA_TAG_FLOAT` fast path which `memcpy`'d the pointer
  bits as an f64. `Int[x]()` printed the short `Lax(3)` `.toString()`
  form instead of the interpreter's full
  `@(hasValue <= true, __value <= 3, __default <= 0, __type <= "Lax")`.
  The fix spans three layers: (1) `src/types/mold_returns.rs` re-
  classifies `Int` / `Float` / `Bool` / `Str` as `Pack` (the actual
  runtime return type, a `taida_lax_new(...)` result); (2) native
  `src/codegen/native_runtime/core.c` adds a
  `taida_lax_tag_value_default` helper so every
  `taida_{int,float,bool,str}_mold_*` function stamps the per-field tag
  on the Lax's `__value` / `__default` slots, and
  `taida_pack_to_display_string` / `_full` honor that tag before
  falling back to the global registry, and
  `taida_io_stdout_with_tag` / `_stderr_with_tag` route any runtime-
  detected BuchiPack through `taida_stdout_display_string` (the
  `_full` entry) so interpreter-parity `@(hasValue <= …, __value <=
  …, __default <= …, __type <= "Lax")` emerges; (3) wasm
  `src/codegen/runtime_core_wasm/{01_core,02_containers}.inc.c` land
  symmetric changes: Lax field-name registration on first
  `taida_lax_new` call, per-field tag dispatch in the new
  `_wasm_pack_to_string_full`, tight `_is_pack_for_stdout` guard that
  excludes Lists / HashMaps / Sets / Async objects (so
  `stdout(@[1,2,3])` still renders as a list, not as `@()`). All four
  pre-existing `Float[x]()` Lax divergences in
  `examples/quality/c21b_float_fn_boundary/{float_local_binding,
  float_fn_result_local}.td` now match the interpreter on 4 backends.
  `tests/c21_js_float_binding.rs` adds four new parity assertions
  (Native / WASM × the two previously-JS-only fixtures), reflecting
  the scope-expanded pin.

### Track B — stream I/O + SIGPIPE tolerance

Restore observable I/O symmetry in the interpreter and harden the CLI
against pipe-chain termination. Post-C20 smoke (Hachikuma Phase 11
TUI-First) exposed that `stderr` / `stdin` already flushed eagerly
while `stdout` / `debug` silently accumulated into an internal Vec
and only surfaced after `eval_program` returned — breaking progress
output, spinners, printf-debugging, and TUI rendering. The same
audit found that `taida run file.td | head -N` exited 141 because
the process carried the default SIGPIPE disposition.

### Interpreter — `stdout` / `debug` two-mode API

`Interpreter` now carries a `stream_stdout: bool` flag and exposes
two public constructors:

- `Interpreter::new()` keeps the legacy buffered behaviour. REPL,
  the in-process `eval_with_output` test harness, and the JS
  codegen embedding path all stay on this mode unchanged, so every
  existing call site works without modification.
- `Interpreter::new_streaming()` switches `stdout` / `debug` to
  `writeln!(io::stdout().lock(), "{}", line)` + `flush().ok()`.
  `taida <file>` / `taida run <file>` now use this variant so
  progress output hits the terminal line-by-line, matching the
  POSIX-standard behaviour every other CLI tool ships with.

Taida surface is unchanged: `stdout(...)` / `debug(...)` still
accept the same arguments, still return the written byte count as
`Int`, still append the implicit trailing newline, and still
suppress the "final value auto-display" when stdout has been
emitted to. No Taida source needs to be edited.

Design note on `debug` routing: an earlier IMPL_SPEC draft routed
stream-mode `debug` to stderr for symmetry with `stderr()`. That
would have broken 3-backend parity — JS (`console.log`) and Native
(`taida_debug_*` → `printf`) already emit to stdout, and the
`test_native_compile_parity` harness diffs captured stdout across
backends. The interpreter was the outlier, so it lines up with the
other two: stream-mode `debug` writes to stdout as well. The
observable symptom (progress / printf-debug surfaces in real time)
is still fixed; only the stream differs from the original plan.

### CLI — SIGPIPE tolerance

`fn main()` now installs `signal(SIGPIPE, SIG_IGN)` once at startup
(unix only, behind `#[cfg(unix)]`). Combined with the `writeln!` +
`flush().ok()` pattern above, `taida run script.td | head -N` now
exits cleanly — the `stdout` builtin silently absorbs `EPIPE` and
returns a zero byte count, matching the standard `ripgrep` / `bat`
convention. Process-wide signal disposition is touched in exactly
one place; individual `stdout` / `debug` call sites do not
re-install handlers.

### Tests

- `tests/c22_stdout_stream.rs` (5 tests) pins the Rust API parity
  between `Interpreter::new()` and `Interpreter::new_streaming()`,
  including the byte-count return and the implicit trailing newline.
- `tests/c22_sigpipe.rs` (4 tests) exercises `taida | head`, stdout
  close, stream-mode `debug` routing to stdout, and the mixed
  stdout/debug ordering contract.
- `tests/c22_stdout_stream_parity.rs` (6 tests) drives the three
  backends (Interpreter / JS / Native) subprocess-style against
  `examples/quality/c22_stdout_stream/progress_loop.{td,expected}`
  and `debug_stream.{td,expected}` so a future regression in
  either backend's stdout routing is caught immediately.

### Out of scope

- REPL remains on buffered mode; its return-value indentation
  machinery is Vec-dependent and will move in a dedicated track.
- Raw-mode / alt-screen auto-leave on panic, SIGHUP, SIGTERM is
  parked as a follow-up (future FB entry).
- Addon-level raw-write path (`terminal.Write`) and the SIGWINCH
  install-order race moved to the TM track (TMB-016 / TMB-017) so
  `taida-lang/terminal` and the main repo can ship independently.

## @c.20.rc4 (in progress)

Complete the Hachikuma Phase 8-10 / Phase D follow-up track: parser
silent-bug elimination, stdin UX alignment across three backends, and
a list-of-record shape for `HttpRequest` headers so dash-bearing names
like `x-api-key` are finally reachable from Taida.

### Parser — new diagnostic `E0303`

`name <= | cond |> A | _ |> B` written across multiple physical lines
on the right-hand side of `<=` silently greedy-absorbed the next
statement as a continuation arm (C19B-009 / ROOT-5). One-line
`|== error: Error = <expr>` dropped every subsequent top-level
definition from the loaded module (C19B-008 / ROOT-4). Both shapes
checker-green'd and broke at runtime.

Now:

- One-line `|== error: Error = <expr>` parses as a single-expression
  handler body and leaves surrounding definitions intact (equivalent
  to the multi-line block form).
- Multi-line multi-arm `| cond |> A | _ |> B` on the right-hand side
  of `<=` is rejected with
  `[E0303] rhs of \`<=\` cannot contain a multi-arm conditional`.
  Use `If[cond, then, else]()`, extract a helper, or wrap the
  conditional in parentheses (`name <= (| ... |> ...)`), which
  restores the top-level parsing context.
- Single-line rhs guards (`name <= | a |> 1 | _ |> 2` on one
  physical line) remain a legal one-shot shape.
- Top-level / function-body `| cond |> body` is untouched.

### New: `stdinLine(prompt?) => :Async[Lax[Str]]`

Hachikuma Phase D's Japanese interview flow exposed ROOT-7: kernel
cooked-mode Backspace deletes one byte at a time, corrupting
multibyte UTF-8 when users edit their typing. C20-2 introduces a
dedicated prelude API that routes through a UTF-8-aware line editor
on all three backends:

- Interpreter: `rustyline` (MIT/Apache-2.0), default editor with
  codepoint-wide Backspace, arrow keys, and Ctrl-U line clear.
- JS: `node:readline/promises` + `rl.question()`; TTY mode enables
  the full editor, pipe mode falls back to line-buffered reads.
- Native: a trimmed derivative of linenoise (BSD-2-Clause, see
  `LICENSES/linenoise.LICENSE`) — termios raw mode, UTF-8
  codepoint-aware Backspace, pipe input drops through to `getline`.

```taida
stdinLine("お名前: ") ]=> line
stdout("こんにちは、" + line.getOrDefault("旅人"))
```

Shape and discipline:

- Return type is **`Async[Lax[Str]]`** across all three backends. The
  Async wrapper exists so the JS path (async-only readline) and the
  Interpreter / Native paths (sync editors) share one surface. Callers
  **must** unmold with `]=>` to obtain the inner `Lax[Str]`; `<=`
  binding leaves the Async in place.
- Any failure (EOF, pipe close, Ctrl-C, Ctrl-D on empty line, missing
  `node:readline/promises`, `termios` error, …) collapses to
  `Lax(null, "")` so the default-value guarantee is preserved —
  `.getOrDefault("")` and `.isEmpty()` both keep working.
- Prompt is optional; non-Str prompts are display-stringified before
  being written, matching the ROOT-14 parity rule already applied to
  `stdin`.
- Out of scope: history, tab completion, multi-line edit. A future
  `taida-lang/readline` addon will layer those features on top.

### `stdin` — three-backend parity (no new API)

`stdin(prompt?)` now behaves identically on Interpreter, JS, and
Native:

- Returns `""` on EOF / read error everywhere (Interpreter used to
  throw `IoError`; JS and Native already silently returned empty).
  Callers that need failure awareness should use the new
  `stdinLine => :Lax[Str]` API (see next section).
- Prompt is optional on every backend including the type checker
  (`stdin()` is now valid; previously `[E1507]` rejected it).
- JS decodes stdin via a streaming `TextDecoder('utf-8', { stream })`
  over a 4 KiB chunk buffer — multibyte codepoints survive chunk
  boundaries instead of collapsing to U+FFFD.
- JS stringifies non-Str prompts via `String(prompt)` inside the
  try/catch so `stdin(1)` / `stdin(@(...))` no longer crashes Node
  with `ERR_INVALID_ARG_TYPE`.
- Native replaces the fixed `char[4096]` stack buffer with
  `getline(3)` on POSIX / a `fgets` realloc loop on Windows, so long
  pasted lines are read completely instead of bleeding the tail into
  the next `stdin` call.

### `HttpRequest` — list-of-record headers

Dash-bearing HTTP headers (`x-api-key`, `anthropic-version`,
`content-type`, ...) are no longer reachable via buchi-pack
identifier keys. C20 adds a second accepted shape:

```taida
resp <= HttpRequest["POST", "https://api.example.com/v1/echo"](
  headers <= @[
    @(name <= "x-api-key", value <= "secret-k"),
    @(name <= "anthropic-version", value <= "2023-06-01"),
  ],
  body <= "{}",
) ]=> await
```

Both shapes are supported on all three backends:

- Legacy: `headers <= @(ident <= "value")` — identifier becomes the
  wire header name as before.
- New: `headers <= @[@(name <= "...", value <= "...")]` — any UTF-8
  is legal in the wire name.

Also:
- JS `HttpRequest[method]()` (fewer than 2 type args) now fails at
  `taida build --target js` time with
  `HttpRequest requires at least 2 type arguments`, matching the
  Interpreter and Native rejection path instead of emitting
  syntactically invalid JavaScript.
- Native lowering's undocumented third-type-arg body fallback
  (`HttpRequest["POST", url, body]()`) has been removed. Interpreter
  and JS always consulted the `body <= ...` field only, so this shape
  silently sent a body on Native while the other two backends sent an
  empty string — a cross-backend parity trap (C20B-012 / ROOT-15). No
  in-tree Taida code relied on the legacy shape; migrate to
  `HttpRequest["POST", url](body <= "...")`.

### User-defined functions called via mold syntax (C20B-014 / ROOT-17)

User-defined functions invoked as `Fn[arg1, arg2]()` now dispatch to
the function instead of silently returning a `@(__value, __type)` mold
wrapper. This closes a 2.1.3-era regression that silently passed
`taida check` but crashed Hachikuma's TUI at every one of 81 call
sites (`CursorMoveTo[r, c]()`, `PadWidth[t, w]()`,
`TruncateWidth[t, w]()`, …). The diagnostic surface aligns:

- **Interpreter** (`src/interpreter/eval.rs`): before the generic
  mold-wrap path, `MoldInst` now detects `Value::Function` in scope
  with no matching `MoldDef` and dispatches to `call_function` with
  `type_args` treated positionally.
- **Native lowering** (`src/codegen/lower_molds.rs`): before the
  `unsupported mold type` error, the `_` arm consults
  `self.user_funcs` and lowers through `lower_func_call` for known
  user functions. Previously `Fn[args]()` failed at build time.
- **Checker** (`src/types/checker.rs`): the `MoldInst` fallback now
  returns the function's registered return type instead of
  `Type::Unknown`, so downstream type inference matches runtime
  behaviour. Named `()` fields on a user-fn mold-syntax call are
  rejected with new diagnostic `[E1511]` — user functions have no
  named-field ABI.
- **JS** is unchanged. Its existing
  `__taida_solidify(Fn(args))` generic fallback already dispatched to
  the user function correctly; the regression test pins that the
  behaviour matches the Interpreter's new shape.

Both `Fn[args]()` and `Fn(args)` are valid and produce identical
results across all three backends.

### Tests

- `tests/c20_parser_silent_bugs.rs` (parser unit, 8 cases)
- `tests/c20_stdin_parity.rs` (3 backends × 4 fixtures + checker
  no-prompt + JS non-Str prompt guard, 14 cases)
- `tests/c20_stdinline_parity.rs` (3 backends × 3 fixtures + 3 checker
  cases, 12 cases — pins `Async[Lax[Str]]` surface, EOF failure, UTF-8
  round-trip)
- `tests/c20_http_dash_header.rs` (3 backends × 2 header shapes +
  JS arity guard, 7 cases)
- `tests/c20b_014_mold_user_fn_call.rs` (3 backends + 2 checker cases,
  5 cases — pins user-fn mold-syntax dispatch and `[E1511]` rejection)
- `examples/quality/c20_parser/*` (2 pins)
- `examples/quality/c20_stdin/*` (4 pins)
- `examples/quality/c20_stdinline/*` (3 pins)
- `examples/quality/c20_mold_user_fn/*` (1 pin)

## @c.19.rc4

Add TTY-passthrough variants of the process-execution APIs so Taida
programs can launch interactive TUI applications (nvim, less, fzf,
git commit, etc.). Closes the Hachikuma Phase 3 P-3-13 (B-006) external
editor integration blocker.

### Features

#### New: `runInteractive(program, args)` / `execShellInteractive(command)`

`taida-lang/os` gains two new functions that match `run` / `execShell`
argument-wise but hand the parent process's stdin / stdout / stderr
directly to the child instead of capturing them through pipes. This is
the mode you want when the child is a terminal UI that needs to read
keystrokes and draw on the TTY.

```taida
>>> taida-lang/os => @(runInteractive)

// Drop the user into $EDITOR, then pick up the exit code afterwards.
r <= runInteractive("nvim", @["/tmp/draft.md"])
stdout("editor exit: " + r.__value.code.toString())
```

Return type: `Gorillax[@(code: Int)]`.

Key differences vs. the captured variants:

| API                   | stdio          | Inner shape                          | Intended use           |
|-----------------------|----------------|--------------------------------------|------------------------|
| `run`                 | pipes (captured) | `@(stdout: Str, stderr: Str, code: Int)` | programmatic output parsing |
| `execShell`           | pipes (captured) | `@(stdout: Str, stderr: Str, code: Int)` | shell expansion + output parsing |
| `runInteractive`      | inherited TTY  | `@(code: Int)`                       | TUI apps (nvim, fzf, …) |
| `execShellInteractive`| inherited TTY  | `@(code: Int)`                       | TUI apps + shell glob |

Signal death uses the `128 + signal` POSIX convention on all three
backends. Windows is best-effort (POSIX is the first-class target).

#### Affected backends

All three backends ship parity implementations:

- **Interpreter** (`src/interpreter/os_eval.rs`): `Command::status()`
  with default (inherited) stdio.
- **JS** (`src/js/runtime/os.rs`): `child_process.spawnSync(..., { stdio: 'inherit' })`.
- **Native** (`src/codegen/native_runtime/os.c`): `fork()` + `execvp`
  with **no** `dup2` in the child.

The 3-backend contract is pinned by `tests/c19_interactive_exec.rs` and
`examples/quality/c19_interactive_exec/*.td`.

### Non-goals (scope-out, for future tracks)

- async / non-blocking interactive exec (the child owns the foreground TTY)
- pty allocation (`openpty` / `forkpty`) — belongs in a future `taida-lang/tty` addon
- automatic raw-mode save / restore on behalf of `terminal` addon users
  (the caller is responsible for `rawModeLeave` / `rawModeEnter` around the handoff)
- stdin write-through API for interactive children

### Backward compatibility

Pure additive change: existing `run` / `execShell` behaviour and return
shape are byte-identical to `@c.18.rc4`. `OS_SYMBOLS[0..35]` keeps the
pre-C19 ordering; the two new entries live at indices 35 and 36.

### Follow-up hardening (C19B, same release)

Two gaps surfaced in the C19 code-review HOLD are fixed in the same
release:

- **C19B-001 — Native `execvp` failure is now an `IoError`.** Before the
  fix, Native collapsed child-side `execvp` failure into `_exit(127)`,
  indistinguishable from a program that merely exited with 127. The
  parent now reads the child's `errno` through a CLOEXEC self-pipe and
  emits a proper `IoError{code, kind, message}` on ENOENT / EACCES /
  etc. — matching Interpreter and JS. Normalized `err.errno` sign on JS
  so all three backends report the positive POSIX errno (e.g. `2` for
  ENOENT).
- **C19B-002 — Checker pins `Gorillax[@(code: Int)]`.** Before the fix,
  `runInteractive(...).__value.stdout` silently passed `taida check`
  because os symbols fell through to `Type::Unknown`. The checker now
  registers typed signatures for `runInteractive` /
  `execShellInteractive`, resolves `.__value` through the Gorillax
  envelope, and rejects access to any field not in the pinned `@(code:
  Int)` inner shape. Captured `run` / `execShell` remain Unknown-typed
  and non-interfering, as promised by the C19 design.
- Native `Gorillax` had a long-standing field-hash mismatch (`__error`
  stored under `HASH___DEFAULT`). Fixed by introducing `HASH___ERROR`
  and threading it through `taida_gorillax_{new,err,relax}`, which
  unblocks `r.__error.<field>` on the Native backend for the first time.

The new failure-path parity is pinned by
`examples/quality/c19_interactive_exec/os_interactive_enoent.{td,expected}`
and the three `c19_run_interactive_enoent_*` tests in
`tests/c19_interactive_exec.rs`.

## @c.18.rc4

Close 4 of the 6 Enum limitations identified by Hachikuma Phase F (2026-04-16).
The 5th (JSON mold Enum validate) shipped in `@c.16.rc4`; the 6th (function
boundary contract dependency) auto-resolves once Enum types cross module
boundaries.

### Features

#### New: Cross-module Enum type resolution (Hachikuma #1 / #6)

`>>> ./m.td => @(Color)` followed by `Color:Red()` in the importer no longer
triggers `[E1608] Unknown enum type`. The type checker now parses the
exporting module and registers its EnumDefs into the importer's type
registry. Aliased imports (`>>> ... => @(Color: Paint)`) are honoured.

**New diagnostic `[E1618]`**: Variant-order mismatch across module boundary.
When an importer keeps a local `Enum => Color = ...` redefinition, its
variant order must match the exporting module's exactly — otherwise ordinals
silently diverge and `jsonEncode` / ordering comparison break. The checker
now catches this at compile time with a clear mismatch diagnostic.

(E1618 was allocated because E1610 is already used for cyclic-inheritance
detection; see the inline docstring in `src/types/checker.rs`.)

#### New: `jsonEncode` emits variant-name Str

`jsonEncode(@(state <= HiveState:Running()))` now returns
`{"state":"Running"}` on all three backends, symmetric with the C16
`JSON[raw, Schema]()` decoder which already accepts the variant-name Str.
Round-trip is now guaranteed:

```taida
rec <= @(state <= HiveState:Running())
raw <= jsonEncode(rec)                 // {"state":"Running"}
JSON[raw, Status]() ]=> rec2
rec2.state == HiveState:Running()      // true
```

**Migration**: If pre-C18 code depended on the ordinal Int wire format
(`{"state":1}`), wrap the Enum value in `Ordinal[]` before encoding:

```taida
payload <= @(state_id <= Ordinal[rec.state]())
jsonEncode(payload)                    // {"state_id":1}
```

The internal representation (`Value::Int(ordinal)` / `int32` in native)
and the `.toString()` contract (returns ordinal Str) are unchanged —
`jsonEncode` is the only observable behaviour that switches.

#### New: `Ordinal[enum]()` mold — explicit Enum → Int

The sanctioned path for converting an Enum value to its declared ordinal:

```taida
Enum => HiveState = :Creating :Running :Stopped

Ordinal[HiveState:Running()]()         // 1
Ordinal[HiveState:Stopped()]() > 0     // true — Int space comparison
```

Replaces the fragile `.toString()`-parsing `initResumeStateOrdinal` helper
from Hachikuma. Arity-1 only; non-Enum arguments produce a typed
runtime error.

The inverse direction (`FromOrdinal[Color, 1]()`) is C18 scope-out.

#### New: Same-Enum ordering comparisons

Same-Enum ordering (`<` / `>` / `>=`) now uses the declared ordinal order:

```taida
HiveState:Creating() < HiveState:Running()    // true
HiveState:Running() >= HiveState:Creating()   // true

ready s =
  | s >= HiveState:Running() |> "yes"
  | _ |> "no"
=> :Str
```

Cross-Enum ordering and Enum↔Int ordering continue to emit `[E1605]` —
use `Ordinal[]` (above) to bridge to Int explicitly. The declared order
of an Enum is now a semantic contract; treat variant reorderings as
breaking changes.

### Notes

- Enum definition syntax (`Enum => Name = :A :B :C`) and the
  "最初のバリアントがデフォルト" rule are unchanged.
- Enum internal representation (`Value::Int(ordinal)` in the interpreter,
  `int32` in native, tagged `__taida_enumVal` wrapper in JS for
  `jsonEncode` toJSON support) is additive; existing code paths that
  read Enum values as Int ordinals continue to work.
- `.toString()` still returns the ordinal Str; the variant-name Str is
  exclusively the `jsonEncode` / wire-format representation.
- All 3 backends (Interpreter / JS / Native) pass the 4 new parity
  tests in `examples/quality/enum_*.td`.

## @c.17.rc4

### Fixes

- **Installer: auto-detect tag re-publish and refresh the cached store
  entry.** Before C17, `taida install` keyed the local store
  (`~/.taida/store/<org>/<name>/<version>/`) only by the tag string.
  When a package maintainer retagged (`taida publish --retag` /
  delete + recreate) the same version against a new commit, the
  consumer's cache kept the old tarball indefinitely -- a subsequent
  `taida install` was a silent no-op, and `taida run` would later
  break on the stale facade. C17 writes a `_meta.toml` provenance
  sidecar alongside every extracted store package recording the
  resolved commit SHA, the tarball SHA-256, and a UTC fetch timestamp.
  On every subsequent install the sidecar is compared against the
  remote tag's commit SHA (via the GitHub git/refs API). When the SHAs
  disagree, the store entry is invalidated and re-extracted
  automatically. Offline / unverifiable states emit a stderr warning
  but never silently skip. The install success-path stdout (package
  list, lockfile line, exit code) is unchanged.

### Features

- **`taida install --no-remote-check`**: skips the remote commit-SHA
  lookup and trusts the existing store sidecar. Intended for offline
  or rate-limited environments. Mutually exclusive with
  `--force-refresh` (rejected at argument parsing time).
- **`taida install --force-refresh` now also wipes the store entry**
  for each package before re-extracting. The legacy addon-cache
  invalidation (`~/.taida/addon-cache/`) is unchanged; store
  invalidation is additive.
- **`taida cache clean --store`**: prune `~/.taida/store/` after
  showing a pre-flight summary. Requires TTY confirmation or `--yes`
  so scripts cannot wipe the store accidentally.
- **`taida cache clean --store-pkg <org>/<name>`**: prune a single
  package from the store (all versions). Scope is narrow enough that
  no confirmation is requested.
- **`taida cache clean --all` now includes the store** in addition to
  the WASM runtime cache and the addon prebuild cache.

### Internal

- `src/pkg/store.rs` gains a pinned 5-row stale-detection decision
  table (`classify_stale`), `resolve_version_to_sha` (GitHub git/refs
  with annotated-tag dereferencing), `invalidate_package` /
  `read_package_meta` primitives, and a `StorePruneReport` for
  cache-clean operations.
- `src/pkg/provider.rs::StoreProvider` carries `force_refresh` /
  `no_remote_check` flags and implements the decision table via
  `apply_stale_decision` before reusing a cached entry.
- `src/pkg/resolver.rs` adds `StoreRefreshFlags` and
  `resolve_deps_with_flags` / `resolve_deps_locked_with_flags`.

### Tests

- 22 unit tests in `src/pkg/store.rs` for decision-table rows, the
  JSON object/string extractors used to parse the git/refs response,
  and an in-process mock server that exercises annotated-tag
  dereferencing.
- 7 new integration tests across `tests/installer_store_staleness.rs`,
  `tests/installer_force_refresh.rs`, `tests/installer_offline.rs`
  backed by a shared `tests/mock.rs` HTTP server that serves both the
  archive tarball and the API endpoints.

## @c.16.rc4

### Fixes

- **`JSON[raw, Schema]()` now validates Enum-typed fields**. Previously,
  `Schema` containing an `Enum` field (`Status: :Active :Inactive
  :Pending`) would silently accept any JSON string — e.g.
  `"status": "Bogus"` — and pass it through as a plain `Str`. This
  broke the "暗黙の型変換なし" philosophy at the JSON boundary:
  downstream code saw an Enum-typed field holding a value outside
  the declared variant set. The fix:
  - `JsonSchema::Enum(name, variants)` is now a first-class schema
    variant alongside `Primitive` / `TypeDef` / `List`.
  - On match, the variant's ordinal (`Int`) is returned (unchanged
    Enum internal representation).
  - On mismatch, key-missing, or `null`, the field becomes
    `Lax[Enum]` with `hasValue=false`, `__value=Int(0)`, and
    `__default=Int(0)` (first variant — the existing "最初のバリアント
    がデフォルト" rule reused as the Lax fallback). Callers must
    handle the boundary explicitly via `hasValue`,
    `| .hasValue |> ... | _ |> ...`, or `getOrDefault(Variant)`
    (`|==` is the `throw`-catching operator and does NOT branch on
    Lax — see `docs/reference/operators.md`).
  - Enum definition syntax (`Enum => Name = :A :B :C`) and the
    first-variant-default rule are unchanged.
  - All three backends (Interpreter / JS / Native) produce
    byte-identical output on `examples/quality/json_enum_validate.td`.

### Migration

If pre-C16 code relied on the silent Str pass-through for Enum
fields, the field now comes back as `Lax[Enum]` and a direct
property access like `result.status.toString()` will surface the
Lax metadata instead of the raw string. Update the access site to
one of:

```taida
// 1. Resolve with a fallback
u.status.getOrDefault(Status:Pending())

// 2. Branch on hasValue
| u.status.hasValue |> handleValid(u.status.__value)
| _                 |> handleInvalid()
```

See `docs/guide/03_json.md` (Enum 型フィールドの検査) and
`docs/reference/mold_types.md` (JSON モールディング型) for the full
rules.

## @c.15.rc3

### Security

- **Supply chain: `taida upgrade` canonical source**. Earlier CLIs
  hard-coded `shijimic/taida` — a personal development fork — as
  the `taida upgrade` release source. Anyone with control of that
  single personal account (by compromise, sale, rename, or deletion)
  could replace published binaries and `SHA256SUMS` with attacker-
  controlled versions, and every `taida upgrade` invocation
  worldwide would silently trust them. The constant is now
  `taida-lang/taida` (the canonical org), and a mandatory regression
  test (`canonical_release_source_is_taida_lang_org`) pins it so
  a future edit must go through a compiler failure and explicit
  review. Stale documentation references to `shijimic/taida` as
  "the core repo" in `docs/reference/cli.md` and the scaffold doc
  comments in `src/pkg/init.rs` were also corrected.
- **User migration**. `@c.13.rc3` and earlier CLIs still look at
  `shijimic/taida` and cannot see `@c.14.rc3+` releases on the
  canonical org. Affected users must reinstall through `install.sh`
  or download a `@c.15.rc3+` archive from
  `github.com/taida-lang/taida/releases` directly. Once on
  `@c.15.rc3+`, `taida upgrade` will see canonical releases as
  expected.
- **Advisory**: `GHSA-xxxx-xxxx-xxxx`
  (<https://github.com/taida-lang/taida/security/advisories/GHSA-xxxx-xxxx-xxxx>) —
  placeholder, to be replaced with the real GHSA ID once published
  under C25B-014. The drafted advisory body lives at
  `.dev/security_advisories/GHSA-DRAFT-taida-upgrade-supply-chain.md`
  until the owner submits it and this note is rewritten in place.

### Fixes

- `taida upgrade --version @c.14.rc3` (and any `--gen c --label rc3`
  query) from a pre-fix CLI returned `version @c.14.rc3 not found in
  releases` because the fork (where the CLI was looking) never
  received that release. After this fix, the upgrade path points at
  the canonical org directly and the lookup resolves.

## @c.14.rc3

### Breaking changes

- **`taida publish` tag-push-only redesign**: `taida publish` is now
  a minimal CLI that only validates the manifest identity, computes
  the next version from an API diff, creates a local git tag, and
  pushes that tag to `origin`. It no longer runs `cargo build`,
  computes SHA-256 digests, writes `addon.lock.toml`, rewrites
  `packages.tdm` or `native/addon.toml`, commits, pushes to `main`,
  or calls `gh release create`. All of those responsibilities moved
  to CI (`.github/workflows/release.yml`). See
  `docs/reference/cli.md#taida-publish` for the new surface and
  `docs/guide/13_creating_addons.md#8-migration-from-pre-c14-addons`
  for the step-by-step migration.
- **Release author semantics unified**: addon releases are now
  created exclusively by `github-actions[bot]` via the addon's own
  `release.yml`. Pre-C14 releases created by the CLI user (human
  account) no longer occur. This matches the core Taida release
  workflow.
- **`packages.tdm` qualified identity required**: `taida publish`
  now rejects bare `<<<@<version>` (no `owner/name`). The
  directory-name fallback that older surfaces permitted is gone —
  every publishable package must declare
  `<<<@<version> <owner>/<name>`. The `taida-lang/terminal` PR #2
  migration commit (`db9637d`) is the reference.
- **`addon.toml` placeholder SHA + lockfile fallback**: `taida
  install` now detects the canonical placeholder SHA
  (`sha256:` + 64 zeros) in `[library.prebuild.targets]` at a tag's
  `native/addon.toml` and falls back to the release-asset
  `addon.lock.toml` for the authoritative hash. This lets addons
  ship a lockfile-only design on `main` while still publishing
  verified cdylibs for tags whose `addon.toml` left the placeholder
  value in place.

### New CLI surface

```
taida publish [--label LABEL] [--force-version VERSION] [--retag] [--dry-run]
```

- `--label LABEL` attaches a pre-release label to the resolved next
  version (`a.4` + `--label rc` → `a.4.rc`).
- `--force-version VERSION` overrides the auto-detected version
  entirely. Skips the API diff snapshot.
- `--retag` force-replaces an already-pushed tag. Skips the API diff
  snapshot.
- `--dry-run` prints the publish plan without touching git.

Automatic version bump (Phase 2a — symbol-level export set diff):

| API change                                  | Bump                               |
|---------------------------------------------|------------------------------------|
| Initial release (no previous tag)           | `a.1` (fixed)                      |
| Symbol removed or renamed                   | Generation (`a.3` → `b.1`)         |
| Symbol added or internal-only               | Number (`a.3` → `a.4`)             |

### New workflow template

`crates/addon-rs/templates/release.yml.template` is the canonical
C14 addon release workflow. It is symmetric with the core Taida
`.github/workflows/release.yml` on all load-bearing axes (4-job
`prepare → gate → build → publish` structure, `github.token`-based
`gh release create`, Taida tag regex, 5-platform build matrix).

- `taida init --target rust-addon` scaffolds the template with two
  placeholders (`{{LIBRARY_STEM}}`, `{{CRATE_DIR}}`).
- Existing addons must migrate manually. See
  `docs/guide/13_creating_addons.md#8-migration-from-pre-c14-addons`.

### Reference release

`taida-lang/terminal@a.1` is the first addon to ship through the
C14 pipeline and serves as the ground-truth reference implementation:

- Release author: `github-actions[bot]`
- 8 assets: 5 × `libtaida_lang_terminal-<triple>.{so,dylib,dll}`,
  `addon.lock.toml`, `prebuild-targets.toml.txt`, `SHA256SUMS`
- CI run: https://github.com/taida-lang/terminal/actions/runs/24495250052
  (all 8 jobs green, ~90s end-to-end)
- Release page: https://github.com/taida-lang/terminal/releases/tag/a.1

### Migration (summary)

For existing addon authors:

1. Add qualified identity to `packages.tdm`:
   `<<<@<version>` → `<<<@<version> <owner>/<name>`.
2. Replace `.github/workflows/prebuild.yml` with the C14
   `release.yml` template (4 jobs, 5-platform matrix, CI-owned
   release creation).
3. Remove obsolete CLI flags from scripts:
   - `taida publish --target rust-addon` → `taida publish`
   - `taida publish --dry-run=plan` → `taida publish --dry-run`
   - `taida publish --dry-run=build` → removed (no local build)
   - `TAIDA_PUBLISH_SKIP_RELEASE=1` → removed (CLI never creates
     releases)
4. Accept that release author is now `github-actions[bot]` in all
   downstream automation / documentation.
5. (Optional) Re-tag existing releases with
   `taida publish --force-version <existing-version> --retag` to
   re-publish them under the new author / asset layout.

Full step-by-step migration: `docs/guide/13_creating_addons.md`
§8. Migration blockers resolved in this cycle: `TMB-013` (identity
on terminal), `TMB-014` (release author on terminal), plus `C14B-001`
through `C14B-006`, `C14B-011`, `C14B-012` (taida-core side).

### Internal

- `src/pkg/publish.rs`: 2,762 → 807 lines. Deleted:
  `prepare_publish`, `PublishRollback`, `rewrite_export_version`,
  `rewrite_prebuild_url_if_needed`, `build_addon_artifacts`,
  `compute_cdylib_sha256`, `create_github_release`,
  `git_commit_tag_push`, `check_dirty_allowlist`,
  `compute_publish_integrity`, `proposals_url`,
  `proposals_repo`. Added: `PublishPlan`, `plan_publish`,
  `render_plan`, `tag_and_push`, `validate_manifest_identity`,
  `require_identity_matches_remote`, `check_gh_auth`,
  `bump_number`, `bump_generation`, `attach_label`,
  `next_version_from_diff`, `latest_taida_tag`, `DiffSkipReason`.
- `src/pkg/api_diff.rs`: new module. Reuses the existing Taida
  parser (`crate::parser::parse`) to snapshot export symbols from
  `taida/*.td` at HEAD and at a tag's tree. `ApiDiff::{Initial,
  None, Additive, Breaking}` classifies the set diff. Phases 2b
  (function signatures), 2c (type pack fields), 2d (`addon.toml`
  arity) are deferred to @c.14.rc2+.
- `src/addon/prebuild_fetcher.rs::is_placeholder_sha` — detects the
  canonical placeholder SHA (`sha256:` + 64 zeros) so the resolver
  can route to lockfile fallback deterministically.
- `src/pkg/resolver.rs::ShaSource` / `choose_sha_source` — pure
  decision table between `AddonToml`, `LockfileFallback`, and
  `NoPrebuild`, pinned by 5 unit tests.
- `crates/addon-rs/templates/release.yml.template` — new 4-job
  template, structural symmetry with core pinned by
  `tests/init_release_workflow_symmetry.rs`.
- `src/main.rs::run_publish` simplified from ~500 lines to ~140
  lines.
- Deleted tests (no longer reflect the real CLI):
  `tests/publish_cli.rs`, `tests/publish_rust_addon.rs`,
  `tests/publish_install_roundtrip.rs` (~1,492 lines total).
- New tests: `tests/publish_tag_push.rs` (7),
  `tests/publish_identity_validation.rs` (4),
  `tests/publish_force_version.rs` (5),
  `tests/publish_retag.rs` (3),
  `tests/publish_api_diff_skip.rs` (3),
  `tests/api_diff.rs` (10),
  `tests/init_release_workflow_symmetry.rs` (5).

### Tests

- lib unit: 2,382 / 2,382 green
- publish integration: 22 / 22 green (force_version 5, retag 3,
  tag_push 7, identity_validation 4, api_diff_skip 3)
- api_diff integration: 10 / 10 green
- init workflow symmetry: 5 / 5 green
- Red tests: 0

## @c.13.rc3

### Language changes

- **Expression-block tail binding**: the last statement of a `| |>` arm
  body, a function body, or a `|==` error-ceiling body may now be a
  binding (`name <= expr`, `expr => name`, `expr ]=> name`, or
  `name <=[ expr`). The bound value becomes the block's result, so a
  redundant trailing `name` line is no longer required. Accepted in
  all three backends (Interpreter / JS / Native).

- **Bind-and-forward in pipelines**: a single-direction `=>` pipeline
  may now contain intermediate `=> name` steps. The current value is
  bound to `name` *and* forwarded to the next step, and later steps
  may reference `name`. Previously these produced
  `[E1502] Undefined variable`.

- **Discard-binding rejection extended**: underscore-prefixed discard
  targets (`=> _x`, `_x <= ...`, `]=> _x`, `_x <=[ ...`) are now
  rejected at any position inside an arm body, function body, `|==`
  handler body, or method body with `[E1616]`. Previously only arm
  bodies enforced this — function / `|==` / method bodies silently
  accepted discard bindings.

See `docs/guide/07_control_flow.md` for the full rule and shorthand
forms.

### Internal

- `src/codegen/lower/`, `src/interpreter/net_eval/`, and
  `src/codegen/native_runtime/` were split along responsibility
  boundaries. No user-visible behaviour change — only source layout
  differs.

### Migration

- Code that ended arm / function / `|==` bodies with an expression
  continues to compile unchanged.
- Code that appended a redundant `name` line to satisfy the old
  restriction may drop that final line without semantic change.
- Code that used discard bindings (`=> _x` etc.) in function or `|==`
  bodies must be updated: either rename the target (dropping the
  leading underscore) or remove the binding entirely if the value is
  genuinely unused.

## @c.12.rc3 (in progress)

In-flight release tracking the @c.12.rc3 milestone (`FUTURE_BLOCKERS.md`
全 12 本消化）. See `.dev/C12_PROGRESS.md` for the live progress tracker.

### Breaking Changes Summary (C12B-037)

@c.12.rc3 bundles **four independent breaking changes** that land in the
same release. A single user codebase upgrading from @b.11.rc3 to
@c.12.rc3 may see multiple compile-time errors at once; this section
collects them in the recommended migration order so you know what to
fix first.

**Impact ranking (most-to-least likely to hit code)**:

1. **Phase 2 — `.toString(radix)` removed** (`[E1508]`)
   - Scope: any call site that uses the JS-style radix argument such as
     `n.toString(16)` or `n.toString(2)`.
   - Migration: replace with `ToRadix[n, base]().getOrDefault("")`.
     See `docs/reference/mold_types.md §ToRadix`.
   - Detection: `taida check` reports `[E1508] .toString() takes no
     arguments`. Fix first — it's purely mechanical.

2. **Phase 5 — `stdout` / `stderr` return `Int` instead of `Value::Unit`**
   - Scope: any `s <= stdout(...)` binding whose downstream code
     assumed `s` was `Unit` or a `Result`. Most real code used
     `stdout(...)` as a statement and is unaffected.
   - Migration: existing `stdout(x) => _` patterns still work (they
     discard the `Int` byte count). If you bound the result, you can
     now perform arithmetic on it: `bytes <= stdout("hi"); stdout(bytes + 1)`.
   - Detection: no compile error for the common discard pattern; only
     code that asserted on the type of the return may need updating.

3. **Phase 4 — `| cond |>` arm bodies must end in a pure expression**
     (`[E1616]`)
   - Scope: arm bodies that contained a discarded side-effect statement
     (e.g. `writeFile(...) => _wr`), a bare function-call statement, or
     a trailing let-binding with no result expression.
   - Migration:
     - Discarded side-effect statement → wrap in an `If[cond, then,
       else]()` mold or hoist the side effect out of the arm.
     - Trailing let binding → add a final expression line (the bound
       name itself works).
     - Let-bindings in non-terminal positions (`doubled <= double(n);
       addOne(doubled)`) are still allowed — the discipline only
       targets side-effect statements.
   - Detection: parser `[E1616]` points to the offending statement with
     its span. See `docs/guide/07_control_flow.md` for the full table
     of accepted / rejected elements.

4. **Phase 3 — non-tail mutual recursion is a compile error** (`[E1614]`)
   - Scope: any function pair (or larger cycle) where at least one edge
     of the call graph cycle is *not* in tail position. Tail-only
     mutual recursion (`isEven` / `isOdd`) continues to work.
   - Migration: refactor the non-tail call to a tail call (often by
     threading an accumulator), or replace the recursion with an
     explicit loop via `Fold` / `Filter` / `Map` molds.
   - Detection: `taida check` / `taida verify` report `[E1614]`
     identifying the offending edge. Formerly this failure surfaced at
     runtime as `Maximum call depth (256) exceeded`.

**Recommended fix order**: 1 → 2 → 3 → 4. `.toString(radix)` and the
`stdout` / `stderr` return-type change are the mechanical ones and can
be resolved without touching control flow. Phases 4 and 3 may surface
in the same function, so landing the pure-expression discipline first
often clarifies the call-graph before the tail-position analysis runs.

**Per-package dry-run (internal official-package-repos)**:

| Package | Phase 2 hits | Phase 3 hits | Phase 4 hits | Phase 5 hits |
|---------|--------------|--------------|--------------|--------------|
| `terminal` | 0 | 0 | 0 | 0 |

(dry-run run 2026-04-15 against `.dev/official-package-repos/`; only
the `terminal` package is tracked in the internal repo tree and it was
authored after all four breaking changes were designed, so no
migration is required for it. `taida-lang/net` / `taida-lang/os`
prelude code lives inside the compiler and is updated in-tree as part
of this release).

External packages should expect <10 compile errors per 1,000 LoC of
Taida code based on RC-era metrics.

### Post-Gate Blocker Fixes (2026-04-15)

Gate review surfaced 8 Must Fix blockers (`C12B-029/030/031/032/033/034/035/040`).
All resolved in this session:

- **C12B-029** — Native `Regex(...)` now fails fast at construction time
  for unsupported flags (`/[^ims]/` characters) and invalid patterns,
  throwing `:Error` with `type=ValueError` that matches the Interpreter
  and JS error shape. 3 parity tests added covering all three backends.
- **C12B-030** — Native regex pattern rewriter gains `\xHH` / `\x{HH..}` /
  `\uHHHH` / `\u{HH..}` hex/Unicode escape support (UTF-8 encoded).
  Documented subset: `\b` / `\B` and the `s` flag remain
  Interpreter/JS-only on Native POSIX ERE.
- **C12B-040** — JS regex implementation split: `Regex(...)` constructor
  previously validated without the `u` flag but `__taida_compile_regex`
  appended `u` at runtime, so `Regex("\x{41}")` / `Regex("\_")`
  constructed successfully and then threw `Invalid escape` on the first
  `.replace` / `.match` / `.search`. Fixed by routing both construct-
  time validation and runtime compilation through a shared
  `__taida_rewrite_pattern` helper that converts `\x{HH..}` / `\u{HH..}`
  to JS-native `\uHHHH` (or UTF-16 surrogate pair for supplementary
  planes) and drops the `u` flag, preserving identity-escape leniency
  for parity with the Rust `regex` crate and POSIX ERE. 4 new parity
  tests cover construct + first-use round-trips for bracketed hex,
  bracketed Unicode, identity escape, and `.match`/`.search` compile
  paths.
- **C12B-031** — `str.match(...)` / `str.search(...)` now require a
  `:Regex` argument at type-check time (`[E1508]`). Previously Str
  literals silently diverged across backends (Interpreter/JS runtime
  throw, Native empty fallback). 4 checker tests added.
- **C12B-032** — `BodyEncoding::Empty` is now a struct variant
  `Empty { had_content_length_header: bool }` so the internal HTTP/1.1
  framing layer can distinguish explicit `Content-Length: 0` from an
  absent Content-Length header. The handler-visible BuchiPack surface
  remains flat (`contentLength: 0`, `chunked: false`) for v1
  compatibility; the new bit flows through
  `parse_request_head` → `ConnReadResult` → `RequestBodyState::new`.
- **C12B-033** — `.dev/C12_PROGRESS.md` gate status line corrected from
  "Final Gate 準備完了" to explicitly acknowledge Phase 9 PARTIAL and
  the presence of OPEN blockers at time of write.
- **C12B-034** — **wasm memory safety fix**: `taida_io_stdout_with_tag`
  / `taida_io_stderr_with_tag` no longer blindly cast a non-Bool
  `val` to `char*`. Non-Bool, non-Str tags route through
  `taida_polymorphic_to_string` so `print_any(42)` on wasm emits `42`
  instead of reading linear memory at address 42. New fixture
  `examples/compile_c12b_034_wasm_nonbool_param.td` locks the
  3-backend + 3-wasm-profile parity (`42 / hello / true / false`).
- **C12B-035** — Phase 2 migration note in `docs/guide/01_types.md`
  and `CHANGELOG.md` corrected: `n.toString(radix)` migrates to
  `ToRadix[n, base]().getOrDefault("")` (returns `Lax[Str]`), not
  the previously-listed `Str[Int[s, 16]()..]()` which performs the
  opposite direction (hex-string → decimal-string).

### Post-Gate Should Fix Completion (2026-04-15 follow-up)

Two Should Fix blockers originally carried over as OPEN/HOLD were
completed in a follow-up session after the user rejected the
"C13 postpone" plan and requested in-scope completion:

- **C12B-021** — FB-18 scope completion (Result type completeness).
  `writeFile` / `writeBytes` / `appendFile` / `remove` / `createDir`
  now return `Result[Int]` (inner value = byte count / removed-entry
  count / newly-created flag) instead of `Result[@(ok, code, message)]`.
  `Exists[path]()` now returns `Result[Bool]` instead of a bare
  `Bool`, distinguishing permission-denied from "path absent". All
  three backends (Interpreter / JS / Native) + the wasm-wasi runtime
  (`runtime_wasi_io.c`) were updated in lockstep. 3 new parity tests
  (`test_c12b_021_writefile_result_int_parity`,
  `test_c12b_021_writefile_failure_is_error_parity`,
  `test_c12b_021_exists_result_bool_parity`) lock the cross-backend
  contract; the pre-existing `test_file_bytes_read_write_three_way_parity`
  was migrated to the new `.isSuccess()` idiom. BREAKING: callers
  that read `.__value.ok` / `.__value.code` / `.__value.message`
  must switch to `.isSuccess()` / `.isError()` and read the new
  `.__value` Int directly; the error envelope is unchanged.

- **C12B-036** — Regex compile cache across all three backends.
  The Interpreter gains a thread-local FIFO cache (`REGEX_CACHE`,
  capacity 64) in `src/interpreter/regex_eval.rs`; the Native
  runtime gains a process-wide FIFO cache (`g_regex_cache`,
  capacity 16) in `src/codegen/native_runtime.c`; the JS runtime
  already had an equivalent `__taida_regex_cache` (capacity 64)
  from the C12B-040 work, preserved as-is. Hot-loop
  `str.replaceAll(Regex(...), ...)` now skips `regcomp` /
  `RegexBuilder::new` / `new RegExp` after the first call, while
  keeping the PHILOSOPHY I "no silent undefined" invariant
  (invalid patterns are still rejected at construction time).
  4 new parity tests (`test_c12b_036_regex_replace_all_hot_loop_parity`,
  `test_c12b_036_regex_cache_distinguishes_flags_parity`,
  `test_c12b_036_regex_search_stateless_parity`,
  `test_c12b_036_regex_match_stateless_parity`) plus 4 interpreter
  unit tests pin the cache behaviour.

### Post-Gate Nice to Have Fixes (2026-04-15)

Two Nice to Have / pre-existing blockers tractable without scope
creep were resolved in the same session:

- **C12B-020** — `expr => _` pipeline discard is now accepted on
  Native (`Lowering error: unsupported pipeline step` resolved) and
  JS (prior codegen emitted `__p = _;` which was a ReferenceError).
  Both backends now treat `Placeholder` as a no-op pipeline step,
  matching the Interpreter. 2 new parity tests
  (`test_c12b_020_stdout_discard_pipeline_parity`,
  `test_c12b_020_pipeline_discard_followed_by_stmt_parity`) lock
  the 3-backend contract.
- **C12B-022** — Native `TypeIs[v, :Int/:Str/:Bool/:Num]()` on a
  function parameter no longer returns a stale compile-time
  assumption (the Int branch previously always answered `true` for
  untyped Idents). The lowerer now emits
  `taida_primitive_tag_match(tag, expected)` whenever the operand
  is in `param_tag_vars`, and the caller-side
  `emit_call_arg_tags_full` additionally propagates the `Int=0`
  default tag for callees detected (via
  `param_type_check_funcs` / `body_uses_typeis_on_ident`) to use
  `TypeIs` on their parameters. WASM runtime gains the mirror
  helper; `EXPECTED_TOTAL_LEN` advances to 237,823. 3 new parity
  tests (`test_c12b_022_typeis_int_param_parity`,
  `test_c12b_022_typeis_str_param_parity`,
  `test_c12b_022_typeis_bool_param_parity`) pin the runtime
  semantics across all backends.

- **C12B-023 (v2 bypass closure)** — Root fix for the Regex silent-UB
  forgery vector. The initial C12B-023 fix (needed_funcs-based
  wasm validator) was bypassed by hand-constructing
  `@(__type <= "Regex", pattern <= "a", flags <= "")` and feeding
  the pack to `_poly` dispatchers (v1). The v1 typechecker follow-up
  (literal-string match on `__type <= "Regex"`) was in turn bypassed
  by variable binding (`tag <= "Regex"; @(__type <= tag, ...)`),
  function-arg routing (`inner t = @(__type <= t, ...)`),
  conditional (`if(c, "Regex", "X")`) and string concatenation
  (`"Re" + "gex"`). The root fix (v2): user-authored
  `Expr::BuchiPack` / `Expr::TypeInst` literals may no longer assign
  **any** `__`-prefixed field name, regardless of the value
  expression. `__`-prefix is reserved for compiler-internal tags
  (`__type`, `__value`, `__default`, `__error`, `__tag`,
  `__items`, `__transforms`, `__status`, `__body_stream`,
  `__body_token`, ...). Field **reads** (`.`-access) remain allowed
  for introspection. `[E1617]` now fires at the checker level and
  blocks compilation on all 7 profiles (interp / js / native /
  wasm-min / wasm-wasi / wasm-edge / wasm-full). BREAKING for any
  user code that wrote `__`-prefixed fields in packs (none detected
  in `examples/`, `docs/`, `tests/` under `cargo test`
  `test_all_examples_pass_typecheck`). 16 new tests:
  `typecheck_examples.rs` gains 8 bypass-route reject tests +
  `test_c12b_023_typeinst_reserved_field_rejected`; each wasm
  profile gains 2–3 variable-bound / concat bypass reject tests;
  `parity.rs::test_net4_nb10_ws_upgrade_fake_req_rejected_3way`
  now pins compile-time rejection of forged `__body_*` packs
  across all 3 backends (shift-left from runtime rejection).

### Improvements

#### `expr_type_tag` Mold-Return Single Source of Truth (FB-27 / Phase 1)

- `src/types/mold_returns.rs` now centralises the mold-name → return-type
  tag table. `src/codegen/lower.rs::expr_type_tag()` and
  `src/types/checker.rs::infer_mold_return_type()` both consult this table.
- Resolves the B11-2f silent regression where Str-returning molds
  (`Upper`, `Trim`, `Join`, etc.) lost their tag when crossing a
  user-function boundary and rendered through Pack heuristics.
- 4 dedicated parity tests added (`test_c12_1_*_parity`).
- Note: `convert_to_string` fallback removal in `taida_io_stdout_with_tag`
  is intentionally deferred to C12-7 (paired with the wasm runtime
  split — wasm-min size gate currently holds at 11KB without the split).

#### `.toString()` Universal Method (FB-10 / Phase 2)

- `.toString()` is now an officially supported universal method on all
  value types (Int / Float / Bool / Str / List / BuchiPack / Lax / Result
  / HashMap / Set / Async / Stream / etc.). Returns `:Str` directly
  (not wrapped in `Lax`).
- Closes FB-10 silent runtime crash where `Concat["...", n.toString()]`
  raised `Concat: arguments must both be list or both be Bytes`. The
  proper string-concat path is `"..." + n.toString()` — see
  `docs/guide/01_types.md`.
- Backend coverage gaps closed:
  - **Interpreter**: List and BuchiPack now have `.toString()` entries.
  - **JS**: `.toString()` calls on plain objects are routed through the
    new `__taida_to_string` runtime helper so untyped packs render as
    `@(field <= value, ...)` instead of JS's default `[object Object]`.
  - **Native**: Already worked — coverage locked in by parity tests.
- Checker rejects `.toString(arg)` with `[E1508]` even when the call is
  nested inside a builtin argument such as `stdout(n.toString(16))`.
  A narrow visitor (`check_tostring_arity_in_expr`) walks builtin args
  for arity violations only, so unrelated type-inference behaviour for
  builtin args is preserved.
- 4 parity tests + 5 checker tests added.
- Migration: code that previously relied on JS's `Number.prototype
  .toString(radix)` (e.g. `n.toString(16)`) is now a compile error.
  Use `ToRadix[n, base]().getOrDefault("")` (returns `Lax[Str]`,
  unwrap with `getOrDefault`) — see `docs/reference/mold_types.md §ToRadix`
  and `docs/guide/01_types.md`. `Str[Int[s, 16]().getOrDefault(0)]()`
  does **not** perform int → hex and was listed in error in an earlier
  draft.

#### Mutual-Recursion Static Detection (FB-8 / Phase 3)

- **Breaking change**: non-tail mutual recursion (a cycle in the call
  graph where at least one edge is not in tail position) is now a
  compile-time error `[E1614]` instead of a runtime
  `Maximum call depth (256) exceeded` crash. Closes FB-8.
- Tail-only mutual recursion (e.g., the canonical `isEven` / `isOdd`
  pair) continues to compile and run on all three backends — the
  Interpreter and JS backends use the existing mutual-TCO trampoline,
  and the Native backend executes regular calls.
- New internal modules:
  - `src/graph/tail_pos.rs` — per-function tail-position analyzer that
    walks the AST and emits `CallSite { callee, is_tail, span }` for
    every direct `FuncCall`. Conservatively treats pipeline stages,
    lambda bodies, and error-ceiling handler bodies as non-tail of the
    outer function.
  - `src/graph/verify.rs::check_mutual_recursion` — new verify check
    that runs `GraphExtractor::extract(program, GraphView::Call)`,
    enumerates cycles via `query::find_cycles`, and rejects any cycle
    containing a non-tail edge. Registered in `ALL_CHECKS` as
    `"mutual-recursion"`.
- `TypeChecker::check_program` now runs this check at the end of the
  pass so that `taida check`, `taida build`, and the compile pipeline
  all surface the error with `[E1614]`. The diagnostic prints the full
  cycle path (`A -> B -> ... -> A`), the offending call site, and a
  hint pointing at `docs/reference/tail_recursion.md`.
- Migration: if you relied on non-tail mutual recursion, convert the
  recursion to an accumulator-passing style (see the new
  "非末尾の相互再帰はコンパイルエラー" section in
  `docs/reference/tail_recursion.md`). The provided error message
  includes the exact file and line of the offending non-tail call.
- 8 verify unit tests + 7 tail-position unit tests + 5 checker tests +
  4 parity tests added. New example
  `examples/compile_c12_3_mutual_tail.td` exercises the tail-only pass
  across the 3-way parity grid and is covered by all three wasm profile
  parity gates.

#### `taida-lang/net` Package Scope Freeze Declaration (FB-20 / Phase 10)

- `taida-lang/net` is now formally frozen as an HTTP-focused server
  package at the v7 HTTP/3 + QUIC transport bootstrap completion point
  (Phase 12 RELEASE GATE GO, 2026-04-07). The server-side HTTP core
  (h1 / h2 / h3) is the completion definition for this package.
- Declaration only — no user-visible surface or runtime change. The
  `httpServe` API, `HttpRequest` / `HttpResponse` contract, and the
  no-silent-fallback policy remain exactly as shipped in v7.
- Six post-H3 extension candidates are explicitly held out of the active
  track and moved to an integration note for future reopen:
  1. HTTP/3 client
  2. WebTransport
  3. QUIC datagram
  4. `httpServe.protocol` Str → Enum migration
  5. Strengthened compile-time capability gating (JS / WASM unsupported)
  6. True zero-copy pursuit (bounded-copy discipline remains the rule)
- Legacy OS passthrough (`dnsResolve` / `tcp*` / `udp*` / `socket*`)
  will not be restored — those primitives remain the responsibility of
  `taida-lang/os`.
- Design notes: `.dev/NET_PROGRESS.md` (post-v7 freeze marker) and
  `.dev/taida-logs/docs/design/net_post_h3.md` (PHILOSOPHY-aligned
  rationale for each of the 6 candidates and the reopen flow).
- Docs only — no code, test, or runtime behaviour changed by this item.

#### `Value::Unit` Elimination on stdout / stderr (FB-18 / Phase 5)

- **Breaking change**: `stdout(...)` and `stderr(...)` now return the
  UTF-8 byte count of the written payload as `Int`, not `Value::Unit`.
  This brings the builtin I/O functions into alignment with
  **PHILOSOPHY I** (「null/undefined の完全排除 — 全ての型にデフォルト
  値を保証」): `Value::Unit` is no longer observable from the Taida
  surface through these calls, and the common idiom
  `bytes <= stdout("hi")` now binds `bytes = 2` instead of `Unit`.
- The byte count excludes the implicit trailing newline. Multi-argument
  `stdout(a, b, c)` counts the concatenated payload length (matches the
  interpreter's `parts.join("")` rendering).
- Source-compatibility: Taida programs that used `stdout(...)` as a bare
  statement — the overwhelmingly common case — are unchanged. The Int
  return value is simply discarded by the statement semantics. The
  `stdout(x) => _` explicit-discard idiom continues to work on the
  Interpreter and JS backends (Native rejects that pipeline form today
  with a pre-existing `Lowering error: unsupported pipeline step`, see
  C12B-019).
- Native main entry (`native_runtime.c`): the C `main()` now discards
  the return value of `_taida_main()` and exits `0`. Previously the
  last statement's value was surfaced as the process exit code —
  harmless while stdout returned `Unit (== 0)`, but a latent bug that
  would have broken every script ending in a non-zero stdout byte count
  after this migration.
- Type checker: `stdout` / `stderr` return type promoted from
  `Type::Unit` to `Type::Int`. `exit` keeps `Type::Unit` since it never
  returns normally. 5 checker tests pin the new table entries.
- Backend coverage:
  - **Interpreter** (`src/interpreter/prelude.rs`): `Value::Int(bytes)`
    where `bytes == joined.len()` (Rust `String::len()` UTF-8 bytes).
  - **JS runtime** (`src/js/runtime.rs`): `__taida_stdout` / `__taida_
    stderr` accumulate `__taida_utf8_byte_length(rendered)` across all
    args and return the total.
  - **Native runtime** (`src/codegen/native_runtime.c`): `taida_io_
    stdout` / `taida_io_stdout_with_tag` / `taida_io_stderr` /
    `taida_io_stderr_with_tag` all return `strlen(payload)` cast to
    `taida_val` (int64_t).
  - **wasm-* runtimes** (`src/codegen/runtime_core_wasm.c`): same
    contract — returns the `wasm_strlen` of the rendered payload.
- Scope discipline: this Phase only touches the functions that actually
  returned `Value::Unit` to Taida surface today. `writeFile` currently
  returns `Result[@(ok, code, message, kind)]` and `Exists` returns
  `Bool` — neither is a Unit leak, so they are tracked under a separate
  follow-up (C12B-020) rather than forced into the same migration.
- 6 parity tests + 5 checker tests + 1 interpreter migration test added.
  New fixture `examples/compile_c12_5_side_effect_returns.td` covers
  ASCII / empty / Int / Bool payloads plus the arithmetic-on-return
  pattern that would have errored pre-C12-5. All 3 wasm profile parity
  grids (min / wasi / full) include the fixture and their expected
  counts bumped by 1.

#### Flaky Test Fix (FB-24 / Phase 8)

- `src/addon/prebuild_fetcher.rs` no longer shares a single
  `.taida-test-temp/` directory across the three `file_scheme_*` tests.
  `make_relative_temp_file` now returns a `RelativeTempDir` RAII guard
  that owns a per-test, uniquely-named directory under CWD and removes
  it whole on drop, so parallel tests cannot race on `create_dir_all` /
  `remove_file` ordering.
- The helper deliberately does **not** use `tempfile::TempDir` because
  `download_from_file` enforces a relative-path-only policy on
  `file://` URLs (RC15B-101); `tempfile::TempDir::path` canonicalises
  to an absolute path.
- The adjacent flakiness in
  `pkg::publish::tests::test_create_github_release_*` (tracked as
  C12B-018 — reproduces on `main` as 2/5 runs failing) is now fixed by
  a process-wide `ENV_MUTEX` inside the `tests` module that serialises
  any test touching `GH_BIN` / `TAIDA_PUBLISH_RELEASE_DRIVER`.
- Verified 20/20 passes for each of three configurations: fetcher-only,
  publish-only, and both filters run simultaneously.
- Test-infra only — no production code or public API change.

#### `| |>` Arm-Body Pure-Expression Discipline (FB-17 / Phase 4)

- **Breaking change**: a condition-arm body (`| cond |> ...`) must now
  be a sequence of **let-bindings** followed by **exactly one final
  result expression**. Non-terminal statements must be one of:
  `name <= expr`, `expr ]=> name`, `name <=[ expr`. Any other
  statement kind (bare function call, discarded pipeline
  `expr => _name`, nested definition, `|==` error ceiling, `>>>` /
  `<<<`) in a non-final position is rejected at parse time with
  `[E1616]`. The final statement must also be an expression — a
  trailing let-binding with no result expression is rejected too.
- Closes FB-17 (`| |>` の文脈渗漏): previously, discarded side-effect
  statements like `writeFile(".hk_write_check", "test") => _wr`
  could silently hide inside what read like a conditional branch,
  breaking the language's invariant that `| |>` is a pure
  expression (`PHILOSOPHY I` / `IV`: a condition arm is a single
  graph node, not a do-block).
- Single-line arm form (`| cond |> expr`) is unaffected — by
  construction it is a pure expression.
- Migration: move discarded side effects out of the arm body.
  Pre-arm setup (`setup() => _`) belongs on a statement line
  preceding the `| |>` expression; in-arm let-bindings that you
  actually consume remain legal. For two-branch expressions the
  `If[cond, then, else]()` mold (B11 Phase 5) is the short form.
  See the new "純粋式の原則" section in
  `docs/guide/07_control_flow.md` for worked migrations.
- 7 parser unit tests + 4 parity tests + new
  `examples/compile_c12_4_arm_pure_expr.td`.

#### `param_tag_vars` Propagation to `stdout` / `stderr` (FB-1 / Phase 11)

- Closes the canonical FB-1 reproducer tracked as C12B-017:
  `print_any v = stdout(v); print_any(true)` (and
  `print_any(TypeIs[42, :Int]())`) now correctly renders as
  `true` / `false` on the Native backend instead of `1` / `0`.
- The stdout / stderr dispatch in `src/codegen/lower.rs` now consults
  `param_tag_vars` for `Ident` arguments whose compile-time tag is
  `UNKNOWN`. When the parameter was tagged at the call site via
  `emit_call_arg_tags`, the runtime tag IrVar is forwarded to
  `taida_io_stdout_with_tag` / `taida_io_stderr_with_tag`, which
  dispatches `TAIDA_TAG_BOOL` to the canonical `true`/`false`
  formatter and falls through to `taida_polymorphic_to_string`
  for every other tag on Native.
- Body-based Bool inference was added for user functions: when a
  function has no explicit `-> Bool` return annotation but its body
  last expression is recognised as Bool by `expr_is_bool()` (e.g.
  `is_int v = TypeIs[v, :Int]()`), the function is registered in
  `bool_returning_funcs`. This lets `b <= is_int(42); stdout(b)`
  preserve the Bool tag through a local `<=` binding.
- **Intentionally out of scope** in Phase 11 (deferred to C12-7 wasm
  runtime split):
  - Extending the tagged path to arbitrary user `FuncCall` args
    (would break `compile_c12_3_mutual_tail` / `compile_mutual_recursion`
    on wasm-full because the wasm `_with_tag` entry point treats
    non-Bool tags as `char*`).
  - Full 4-pattern tag_prop refactor (conditional arm join /
    pipeline intermediate / Lax unmold / runtime callback) as
    originally described in `C12_DESIGN.md` Workstream K. The
    canonical FB-1 reproducer no longer regresses, so the
    additional refactor is best paired with the wasm runtime
    polymorphic dispatch cleanup in C12-7.
- 5 parity tests (`test_c12_11_*_parity`) + new
  `examples/compile_c12_11_tag_prop.td` fixture (wasm-min / wasm-wasi /
  wasm-full all exercise the grid).

#### Regex Type + Str Method Overloads (FB-5 Phase 2-3 / Phase 6)

- New prelude constructor `Regex(pattern, flags?)` returns a typed
  BuchiPack with `pattern <= Str`, `flags <= Str`, `__type <= "Regex"`.
- Str methods are now overloaded by first-argument type. Passing a
  Regex value dispatches through a regex engine; passing a Str keeps
  the B11 Phase 1 fixed-string semantics unchanged:
    - `str.replace(Regex(p), rep)` / `str.replaceAll(Regex(p), rep)`
    - `str.split(Regex(p))`
    - `str.match(Regex(p))` → `:RegexMatch` BuchiPack with
      `hasValue: Bool`, `full: Str`, `groups: @[Str]`, `start: Int`
    - `str.search(Regex(p))` → `Int` (char index of first match or
      `-1` when no match; no null leak — philosophy I)
- Backend implementations:
    - **Interpreter**: new `src/interpreter/regex_eval.rs` module
      wrapping the Rust `regex` crate. 16 unit tests.
    - **JS**: `src/js/runtime.rs` helpers backed by native `RegExp`.
    - **Native**: `src/codegen/native_runtime.c` POSIX `<regex.h>`
      with `taida_regex_rewrite_pattern` translating Perl-style
      meta escapes (`\d` / `\w` / `\s` etc.) to POSIX classes.
- Flag support: `i` (case-insensitive), `m` (multiline anchors),
  `s` (dotall — Interpreter / JS only; POSIX ERE has no dotall).
  Unknown flags throw `ValueError` at `Regex(...)` construction.
- wasm profiles do **not** link regex support (C12B-023); dispatcher
  stubs forward Regex-shaped calls back to fixed-string helpers.
- 9 parity tests (`test_c12_6_*_parity`) cover fixed-string
  regression, character classes, first-vs-all semantics, split,
  match with groups, search, literal `$` handling, and the `i` flag.

#### HTTP/1.1 Body Encoding Internal Representation (FB-2 / Phase 12)

- Internal-only refactor: introduces `BodyEncoding` enum in
  `src/interpreter/net_eval.rs` (`Empty` / `ContentLength(u64)` /
  `Chunked`) as the single source of truth for how an HTTP/1.1
  request body is read. `RequestBodyState` now carries a
  `body_encoding` field; `read_body_chunk` dispatches off it
  instead of juggling `is_chunked` / `content_length` /
  `fully_read` flags independently.
- Closes FB-2 (body span drift): ensures that `Content-Length: 0`,
  header-absent, and `Transfer-Encoding: chunked` paths can no
  longer drift out of sync with one another.
- Handler API unchanged: the `HttpRequest` buchi-pack still exposes
  `contentLength: Int` + `chunked: Bool` at the Taida surface — v1
  is preserved. The `BodyEncoding` refinement is purely internal.
- 9 unit tests added covering the classifier, constructor from
  parsed headers, accessors, and `RequestBodyState` integration.

#### JS Runtime File Split (FB-21 / Phase 9, partial)

- Internal-only refactor: split `src/js/runtime.rs` (6,496 lines) into
  `src/js/runtime/{core,os,net}.rs` + `mod.rs` so each chunk stays
  under 3,500 lines and owns a single coherent concern.
  - `core.rs` (2,015 lines) — helpers / types / arithmetic / Lax /
    Result / BuchiPack / throw / Async / Regex / stream / stdout /
    stderr / stdin / format / toString / HashMap / Set / equals /
    typeof / spread.
  - `os.rs` (1,142 lines) — `taida-lang/os` 13 API + `sha256` crypto.
  - `net.rs` (3,254 lines) — `taida-lang/net` HTTP v1 (parser /
    encoder / chunked / streaming writer / SSE / body reader /
    WebSocket).
- The embedded JS runtime bytes are **byte-identical** to the
  pre-split version; a new
  `test_runtime_js_chunk_concat_invariants` guards chunk boundaries.
- `RUNTIME_JS` surface changed from `pub const &str` to
  `pub static LazyLock<&'static str>` because `concat!()` only
  accepts literals. The single consumer in `src/js/codegen.rs`
  was updated (`push_str(RUNTIME_JS)` → `push_str(&RUNTIME_JS)`).
  `tests/parity.rs` file-path reads also migrate to
  `src/js/runtime/net.rs`.
- Placement tables for the remaining three targets
  (`src/codegen/lower.rs`, `src/interpreter/net_eval.rs`,
  `src/codegen/native_runtime.c`) are locked in
  `.dev/taida-logs/docs/design/file_boundaries.md`; the mechanical
  moves are tracked as C12B-024 / C12B-025 / C12B-026 and will land
  as independent follow-up PRs (per C12-9 policy: "split must not
  share a PR with semantic changes").

#### WASM Core Runtime Split + wasm-edge Size Budget Restoration (FB-26 / Phase 7)

- **wasm-edge `stdout("Hello from edge!")` is now 351 bytes** (previously
  ~10.5KB when the tagged runtime chain linked `taida_polymorphic_to_string`
  and its entire display helper fan-out). The B11-2f fix that isolated
  `taida_io_stdout_with_tag`'s non-Bool branch to a plain `char*` path —
  combined with `wasm-ld --gc-sections` — now yields a hello-world wasm
  binary that links only `_start` + `taida_io_stdout` + `write_stdout` +
  `wasm_strlen` + the WASI `fd_write` import. Closes FB-26.
- `tests/wasm_edge.rs::wasm_edge_size_check` threshold restored from 16KB
  back to **4KB** (the original WE-3c gate, raised transiently to 16KB in
  commit `7af9684` / FB-25 during B11). The new ~351B budget leaves ~11×
  headroom.
- New regression test `wasm_edge_hello_no_polymorphic_regression` (1KB
  upper bound) specifically guards against a future regression that would
  pull `taida_polymorphic_to_string` back onto the static-string stdout
  reference chain.
- Internal-only refactor: split `src/codegen/runtime_core_wasm.c`
  (6,463 lines) into `src/codegen/runtime_core_wasm/{01_core,
  02_containers, 03_typeof_list, 04_json_async}.inc.c` + `mod.rs` so each
  fragment owns a single functional concern and stays well under 3,000
  lines.
  - `01_core.inc.c` (2,698 lines) — libc stubs, bump allocator, strlen
    helpers, stdout/stderr/debug I/O, integer/bool arithmetic, float
    arithmetic + Rust-Display-compatible formatter, polymorphic
    display, BuchiPack / List / HashMap / Set runtimes, WC-6
    extensions.
  - `02_containers.inc.c` (1,555 lines) — Closure runtime, error
    ceiling (error-flag based, no setjmp/longjmp), Lax[T], Result[T,P]
    + Gorillax, Cage, Molten/Stub/Todo stubs, type conversion molds
    (returning Lax), float div/mod molds, string template helpers,
    error object helpers, digit/char helpers.
  - `03_typeof_list.inc.c` (887 lines) — RC no-ops (wasm has no heap
    refcount), typeof (compile-time tag + runtime heuristic), List
    HOF / operations / queries, element retain/release no-ops.
  - `04_json_async.inc.c` (1,323 lines) — JSON runtime (manual
    strtol/strtod/itoa/ftoa/FNV-1a), type detection for JSON
    serializer, public field lookup wrappers, schema helpers, schema
    descriptor application, Async runtime (synchronous blocking for
    wasm-min), `_taida_main` extern declaration, `_start` WASI entry.
- The embedded wasm runtime bytes are **byte-identical** to the
  pre-split version; a new `test_runtime_core_wasm_fragment_concat_preserves_bytes`
  pins the total C source length (235,855 bytes) and anchors the
  assembled source's first / last bytes.
- Same assembly pattern as the JS runtime split (C12-9d):
  `LazyLock<&'static str>` + `Box::leak` produces a `&'static str`
  without adding a crate dependency (`concat!()` would require literal
  arguments). All five `include_str!("runtime_core_wasm.c")` call sites
  in `src/codegen/driver.rs` now point to
  `&crate::codegen::runtime_core_wasm::RUNTIME_CORE_WASM`.
- The lightweight `stdout_with_tag` proposal (C12-7b) and `wasm-opt -Oz`
  post-link step (C12-7c) were **deliberately not adopted**: the current
  codegen already hits 351B for the static-string path, and swapping the
  tagged runtime in would risk breaking the wasm-min 512B size gate
  (`wasm_min_size_gate`) via the same `taida_polymorphic_to_string`
  reference chain the B11-2f fix removed. `wasm-opt` is also not
  installed in the standard toolchain. C12B-016 remains OPEN as a
  follow-up for when the wasm-min runtime grows further and makes the
  tagged unification cost-effective again.

## @b.11.rc3

Released: 2026-04-14

### New Features

#### Publish Package Identity (FB-22)

- `taida publish` now resolves the package name from the `<<<` line in `packages.tdm`
- Canonical format: `<<<@gen.num.label owner/name` (e.g. `<<<@b.11.rc3 taida-lang/terminal`)
- Existing `<<<@version` format remains valid (backward compatible)
- `proposals_url()`, release title, and dry-run output consistently use the manifest package identity
- Org package publishing (e.g. `taida-lang/*`) is now supported

#### Native Bool Display (FB-3)

- Native backend now displays `true`/`false` instead of `1`/`0` for Bool values
- Added `taida_io_stdout_with_tag()` to native and WASM runtimes for type-aware output
- 3-way parity restored for Bool stdout/stderr

#### Str Methods: replace / replaceAll / split (FB-5)

- `Str.replace(target, replacement)` -- replaces the first match
- `Str.replaceAll(target, replacement)` -- replaces all matches
- `Str.split(separator)` -- splits into a list of strings
- Empty target in replace/replaceAll is a no-op (returns original string)
- `split("")` splits into individual characters (equivalent to `Chars[]`)
- Full 3-way parity (Interpreter / JS / Native)

#### If Mold (FB-6)

- `If[condition, then_value, else_value]()` -- 2-branch conditional as a mold
- Non-selected branch is not evaluated (short-circuit)
- Pipeline placeholder `_` supported: `150 => If[_ > 100, 100, _]()`
- Nestable: `If[cond, If[cond2, a, b](), c]()`
- Branch type mismatch is rejected with `[E1603]` (same as `| |>`)
- Full 3-way parity

#### TypeIs / TypeExtends Molds (FB-12)

- `TypeIs[value, :TypeName]()` -- runtime type check returning Bool
- `TypeIs[value, EnumName:Variant]()` -- enum variant check
- `TypeExtends[:TypeA, :TypeB]()` -- compile-time type relationship check
- Restricted type-literal surface (`:Int`, `:Str`, `:NamedType`, etc.) accepted only inside `TypeIs`/`TypeExtends` brackets
- Named type and error subtype support via `__type` field and inheritance chain
- `TypeExtends` rejects `EnumName:Variant` literals with `[E1613]`
- Full 3-way parity

#### Int[str]() Surface Lock (FB-9)

- `Int[str]()` / `Int[str, base]()` officially documented as the canonical Str-to-Int conversion path
- `+` sign prefix accepted in base-specified conversions across all backends
- No `StrToInt` alias introduced (existing surface is the standard)

#### packages.tdm Export Surface Simplification (FB-23 + Phase 10)

- **Breaking**: Canonical surface simplified to `<<<@version owner/name @(symbols)` (no arrow)
- `>>> ./main.td` declares entry point only (no export symbols)
- `Manifest.exports` field -- extracted from `<<<@version owner/name @(symbols)` only
- Package root import uses `manifest.exports` as the authoritative facade filter across all backends
- **Breaking**: The following surfaces are no longer accepted:
  - `<<<@version owner/name => @(symbols)` (arrow form)
  - `>>> ./main.td => @(symbols)` as facade declaration (split surface)
  - `<<<@version @(symbols)` without package identity (symbols-only)
- `taida init` templates updated with canonical surface guidance

### Diagnostic Codes

| Code | Description |
|------|-------------|
| `[E1613]` | TypeExtends does not accept enum variant type literals |

### Internal Changes

- `taida_io_stdout_with_tag()` / `taida_io_stderr_with_tag()` in native runtime with type tag constants
- `taida_typeis_named()` runtime function for named type / error subtype checking
- `Expr::TypeLiteral` AST node for restricted type-literal surface in mold arguments
- `check_mold_errors_in_expr()` / `check_mold_errors_in_stmt()` for dedicated mold validation pass
- `CondBranch` IR for If mold in native backend
- JS `replace()` uses callback pattern to prevent `$&`/`$$` meta-character expansion
- `Manifest.exports: Vec<String>` for package public API facade extraction
- Parser accepts `<<<@version owner/name @(symbols)` as canonical export surface (arrow form removed)
- `eval_import` filters package root imports by `manifest.exports` when present
- Checker / JS / Native import validation unified to use `manifest.exports` as facade authority

### Documentation

- Guide updated: `01_types.md` (replace/split methods, Int[str]() docs), `05_molding.md` (If, TypeIs, TypeExtends), `07_control_flow.md` (If mold, TypeIs/TypeExtends sections)
- Reference updated: `mold_types.md` (If, TypeIs, TypeExtends, Int[str,base] sections), `standard_methods.md` (replace, replaceAll, split)

---

## @b.10.rc2

Released: 2026-04-10

### Breaking Changes

- **`taida build` default target is now `native`** -- `taida build file.td` now produces a native binary instead of `.mjs` output. If your CI or scripts relied on the default being JS, add `--target js` explicitly or use `taida transpile`.
- **taida-lang/net: Remove legacy OS re-exports** — 16 socket/DNS symbols (`dnsResolve`, `tcpConnect`, `tcpListen`, `tcpAccept`, `socketSend`, `socketSendAll`, `socketRecv`, `socketSendBytes`, `socketRecvBytes`, `socketRecvExact`, `udpBind`, `udpSendTo`, `udpRecvFrom`, `socketClose`, `listenerClose`, `udpClose`) are no longer exported from `taida-lang/net`. Use `taida-lang/os` instead.
- **httpServe protocol field** — Numeric literals for the `protocol` field (e.g. `@(protocol <= 42)`) are now rejected at compile time. Use `HttpProtocol` enum or `Str`.

### New Features

#### Enum Types (RC3)

- New `Enum` keyword for defining enumeration types
- Syntax: `Enum => Status = :Ok :Fail :Retry`
- Enum values evaluate to ordinal integers (0-indexed)
- Constructor syntax: `Status:Ok()`
- Full 3-way parity (Interpreter / JS / Native)

#### HttpProtocol Enum (RC3)

- `taida-lang/net` exports `HttpProtocol` enum with variants `:H1`, `:H2`, `:H3`
- Compile-time backend capability gates: JS rejects H2/H3, WASM rejects all httpServe usage
- Wire format mapping: `H1` = `"h1.1"`, `H2` = `"h2"`, `H3` = `"h3"`

#### Escape Sequences (RC3)

- `\0` — null character
- `\xHH` — hex escape (2-digit)
- `\u{HHHH}` — Unicode escape (1-6 digits)
- Unified escape handling across string literals and template strings

#### Chars Mold (RC3)

- `Chars["text"]()` splits a string into Unicode grapheme clusters
- `CodePoint[char]()` returns the Unicode code point

#### Doc Comments on Assignments (RC3-adjacent)

- `///@` documentation comments can now be attached to assignment statements

#### Rust Addon System (RC1 / RC1.5 / RC2 / RC2.5 / RC2.6 / RC2.7)

- **RC1**: Native addon foundation — `cdylib` loading, ABI v1, `addon.toml` manifest, function dispatch
- **RC1.5**: Prebuild distribution — `[library.prebuild]` in `addon.toml`, SHA-256 integrity verification, `~/.taida/addon-cache/`, host target detection (5 baseline + 5 extension targets), progress indicator, `file://` testing URLs
  - **RC15B hardening**: reserved `[library.prebuild.signatures]` schema with `gpg:<opaque>` value validation and canonical-target-triple keying; `PrebuildUnknownTarget` parse-error guard against cache-directory traversal via attacker-keyed targets; HTTPS download policies (120 s request timeout, max 10 redirects via `HTTPS_MAX_REDIRECTS`, 100 MB payload cap, `https → http` downgrade rejected, scheme whitelist `https://` + `file://`); strict unknown-key forward-compatibility (any unknown section / top-level key / inner key / duplicate key is a parse error); `_meta.toml` provenance sidecar (`schema_version`, `commit_sha`, `tarball_sha256`, `tarball_etag`, `fetched_at`, `source`, `version`) written next to every store entry. The literal `D26` token in the unsupported-backend error string is pinned for the gen-C generation; the rename to a wasm dispatcher is deferred to gen-D.
- **RC2**: Package scaffold — `taida init --target rust-addon`, Taida-side facade module, `src/addon/` module tree
- **RC2.5**: Cranelift native backend addon dispatch
- **RC2.6**: Publish workflow — `taida publish --target rust-addon`, 2-stage `--dry-run=plan|build`, `addon.lock.toml`, GitHub Release API integration, CI workflow template
- **RC2.7**: Distribution hardening — 9 blocker fixes, CI template robustness

#### CLI Surface Normalization (RC5)

- **`taida build` default target changed to `native`** -- Previously defaulted to `--target js`. Now `taida build file.td` produces a native binary. Use `--target js` or `taida transpile` for JS output.
- **`taida transpile`** remains as an alias for `build --target js` (unchanged behavior).
- **`taida upgrade`** -- New self-update command. Downloads and installs the latest taida binary from GitHub Releases. Supports `--check`, `--gen`, `--label`, and `--version` flags.

### CLI Changes

| Command | Change |
|---------|--------|
| `taida build` | **Breaking**: Default target changed from `js` to `native` |
| `taida upgrade` | New: Self-update taida binary |
| `taida upgrade --check` | New: Check for updates without installing |
| `taida init --target rust-addon` | New: Scaffold Rust addon project |
| `taida publish --target rust-addon` | New: Build and release addon |
| `taida publish --dry-run=build` | New: Build-only dry run |
| `taida install --force-refresh` | New: Ignore addon cache |
| `taida install --allow-local-addon-build` | New: Fallback to local cargo build |
| `taida update --allow-local-addon-build` | New: Fallback to local cargo build |
| `taida cache clean --addons` | New: Prune addon cache |

### Internal Changes

- `CompileTarget` enum for backend-specific type checking
- `net_surface.rs` module centralizes `taida-lang/net` symbol definitions
- `Expr::span()` method on AST for unified span access
- `TypeRegistry::enum_defs` for enum type registration
- `src/crypto.rs` hand-written SHA-256 (no external crate)
- `src/pkg/resolver.rs` dependency resolution engine
- `src/pkg/github_release.rs` GitHub Release API client
- `src/upgrade.rs` self-update module with version resolution

### Documentation

- Guide updated: enum types, escape sequences in `01_types.md`
- Guide index completed: all 14 chapters listed in `00_overview.md`
- CLI reference updated for all new commands and options
- README.md rewritten with current features and complete doc index

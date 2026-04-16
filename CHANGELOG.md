# Changelog

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

# Changelog

## @c.12.rc3 (in progress)

In-flight release tracking the @c.12.rc3 milestone (`FUTURE_BLOCKERS.md`
全 12 本消化）. See `.dev/C12_PROGRESS.md` for the live progress tracker.

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
  Use `Str[Int[n]().getOrDefault(0)]()` or define a dedicated radix
  formatter — see `docs/guide/01_types.md`.

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

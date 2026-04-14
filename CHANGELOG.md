# Changelog

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

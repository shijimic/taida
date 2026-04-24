//! Taida native runtime — single translation unit assembled from five
//! responsibility-aligned C source files plus a declarative header.
//!
//! **History**:
//!
//! - C12B-026 (C12-9 Phase 9 Step 2) で `src/codegen/native_runtime.c`
//!   (20,866 行 / 886,457 bytes) を 7 フラグメント (`01_core.inc.c` ..
//!   `07_net_h3_main.inc.c`) に機械的に分割した。
//! - C13-4 (C13B-004) で、`.dev/C13_DESIGN.md §C13-4` の責務境界に合わせて
//!   フラグメントを 5 責務ファイル + 共有ヘッダ (`runtime.h`) に再配置した。
//!   この再配置は **物理的なリネーム + 連結のみ** で、連結ストリームの
//!   バイト列は C12B-026 時点と完全一致する (886,457 bytes)。
//!
//! **採用方式**: Rust-level 連結 (`LazyLock<&'static str>` + `Box::leak`) —
//! `runtime_core_wasm/` (C12-7a / C12B-027) と同一。clang 視点では完全に
//! 1 translation unit として振る舞う (driver.rs が `fs::write` で連結済み
//! の C ソースを書き出してから clang に渡す) ため、DCE / static helper
//! の cross-reference / forward declaration は分割前とバイト単位で同一。
//!
//! **責務境界** (C13-4a, 詳細は `runtime.h`):
//!
//! - [`CORE_SECTION`] (7,838 行, `core.c`): libc stubs / safe-malloc /
//!   allocator / type conversion molds / ref-counting / heap strings /
//!   BuchiPack / globals / Closure / List / Bytes / String / Regex /
//!   polymorphic dispatchers / template strings / Int/Float/Bool/Num
//!   methods / HashMap / Set / polymorphic length / collection methods /
//!   Error ceiling (setjmp/longjmp) / Result / Lax methods / polymorphic
//!   monadic dispatch / Async pthread support / Async aggregation /
//!   Debug for list / JSON Molten Iron / stdlib math / Field registry /
//!   jsonEncode/jsonPretty / stdlib I/O / SHA-256
//! - [`OS_SECTION`] (668 行, `os.c`): taida-lang/os package
//!   (Read / readBytes / ListDir / Stat / Exists / EnvVar / writeFile /
//!   writeBytes / appendFile / remove / createDir / rename / run /
//!   execShell / allEnv / ReadAsync)
//! - [`TLS_SECTION`] (1,720 行, `tls.c`): NET5-4a OpenSSL dlopen /
//!   TLS-aware I/O wrappers / HTTP/1.1 over raw TCP / TCP socket APIs /
//!   pool package runtime
//! - [`NET_H1_H2_SECTION`] (6,182 行, `net_h1_h2.c`): taida-lang/net HTTP
//!   v1 runtime (httpParseRequestHead / httpEncodeResponse / readBody /
//!   keep-alive / chunked / streaming / WebSocket / thread pool) +
//!   Native HTTP/2 server (HPACK / H2 frames / taida_net_h2_serve)
//! - [`NET_H3_QUIC_SECTION`] (4,458 行, `net_h3_quic.c`): H3/QPACK
//!   constants / H3 frame I/O / libquiche dlopen FFI / QUIC connection
//!   pool / taida_net_h3_serve / httpServe entry / RC2.5 addon dispatch /
//!   main()
//!
//! 分割配置表: `.dev/taida-logs/docs/design/file_boundaries.md §4` +
//! `src/codegen/native_runtime/runtime.h`

use std::sync::LazyLock;

/// Fragment 1: C runtime core primitives + Error / Result / Async / JSON.
/// Merged from C12B-026 fragments 1 (`01_core.inc.c`) + 2
/// (`02_error_json.inc.c`). (7,838 lines)
pub const CORE_SECTION: &str = include_str!("core.c");

/// Fragment 2: taida-lang/os package (668 lines).
pub const OS_SECTION: &str = include_str!("os.c");

/// Fragment 3: OpenSSL TLS, TCP sockets, pool (1,720 lines).
pub const TLS_SECTION: &str = include_str!("tls.c");

/// Fragment 4: HTTP/1 + WebSocket + HTTP/2 runtime.
/// Merged from C12B-026 fragments 5 (`05_net_v1.inc.c`) + 6
/// (`06_net_h2.inc.c`). (6,182 lines)
pub const NET_H1_H2_SECTION: &str = include_str!("net_h1_h2.c");

/// Fragment 5: HTTP/3 + QPACK + QUIC + httpServe entry + addon dispatch +
/// main() (4,458 lines).
pub const NET_H3_QUIC_SECTION: &str = include_str!("net_h3_quic.c");

/// Full native runtime C source, assembled from the five responsibility
/// fragments on first access and cached for the process lifetime.
///
/// Byte-identical to the pre-split monolithic `native_runtime.c` as well
/// as to the C12B-026 seven-fragment layout (see
/// `test_native_runtime_fragment_concat_preserves_bytes` below for the
/// invariant assertion).
///
/// C13-4 note: the physical files were merged / renamed but the
/// concatenation order (core -> os -> tls -> net_h1_h2 -> net_h3_quic)
/// corresponds 1:1 to the C12B-026 fragment 1..7 concatenation order,
/// so DCE / static helper cross-reference / forward declarations see the
/// same byte stream as before.
///
/// Strategy note: `concat!()` cannot be used because that macro requires
/// literal arguments; `LazyLock<&'static str>` + `Box::leak` exposes a
/// `&'static str` without adding a crate dependency. Same strategy as
/// `src/codegen/runtime_core_wasm/mod.rs::RUNTIME_CORE_WASM` (C12-7a /
/// C12B-027) and `src/js/runtime/mod.rs::RUNTIME_JS` (C12-9d).
pub static NATIVE_RUNTIME_C: LazyLock<&'static str> = LazyLock::new(|| {
    let total = CORE_SECTION.len()
        + OS_SECTION.len()
        + TLS_SECTION.len()
        + NET_H1_H2_SECTION.len()
        + NET_H3_QUIC_SECTION.len();
    let mut s = String::with_capacity(total);
    s.push_str(CORE_SECTION);
    s.push_str(OS_SECTION);
    s.push_str(TLS_SECTION);
    s.push_str(NET_H1_H2_SECTION);
    s.push_str(NET_H3_QUIC_SECTION);
    debug_assert_eq!(s.len(), total);
    Box::leak(s.into_boxed_str())
});

#[cfg(test)]
mod tests {
    use super::*;

    /// C12B-026 / C13-4 invariant: the concatenation of the five
    /// responsibility fragments must be byte-identical to the pre-split
    /// monolithic `native_runtime.c` as well as to the C12B-026 seven-
    /// fragment concatenation. We anchor the total byte length + a check
    /// of the first / last meaningful lines of the assembled source to
    /// detect accidental edits that would break DCE or shift static
    /// helper references across fragment boundaries.
    ///
    /// Total bytes snapshot: 886,457 (at C12B-026 split time; preserved
    /// through C13-4 rename / merge). If a future change intentionally
    /// modifies the runtime C source, update both the relevant fragment
    /// file and the `EXPECTED_TOTAL_LEN` constant below in the same
    /// commit.
    ///
    /// C16 (2026-04-16): +2,074 bytes in core.c from `E{...}` enum schema
    /// descriptor support in `json_apply_schema` / `json_default_value_for_desc`.
    /// New total: 888,531.
    ///
    /// C16B-001 (2026-04-16): +4,359 bytes in core.c — rewrote
    /// `json_default_value_for_desc` to a dedicated `json_pure_default_apply`
    /// walker so TypeDef defaults embed `Int(0)` for Enum fields instead of
    /// routing through `json_apply_schema(NULL, ...)` which produced
    /// `Lax[Enum]` and broke 3-backend parity (Interpreter/JS correctly
    /// returned `Int(0)`). New total: 892,890.
    ///
    /// C18-2 (2026-04-17): +3,998 bytes in core.c — added
    /// `taida_register_field_enum`, `taida_lookup_field_enum_desc`, the
    /// `enum_desc` slot in the per-field registry, `json_append_enum_variant`,
    /// and the tag-5 (Enum) branch in `json_serialize_pack_fields`. These
    /// changes let `jsonEncode` emit the variant-name Str (e.g. `"Running"`)
    /// for Enum fields, symmetric with the C16 `JSON[raw, Schema]()`
    /// decoder. Legacy total after C18-2: 896,888.
    ///
    /// C18B-003/005 (2026-04-17): +5,086 bytes in core.c — added the
    /// per-pack enum registry (`taida_register_pack_field_enum` /
    /// `taida_lookup_pack_field_enum_desc` with `__pack_field_enum_registry`)
    /// so two packs sharing a field name with different Enum types no
    /// longer collide in `jsonEncode` (C18B-003); plus
    /// `taida_runtime_panic(msg)` for the strict `Ordinal[]` runtime
    /// contract (C18B-005). Intermediate total: 901,974.
    ///
    /// C18B-006 (2026-04-17): +2,272 bytes in core.c — added
    /// `json_parse_string_raw_len` (length-aware variant) + `str_len`
    /// field in `json_val` so the enum validation in `json_apply_schema`
    /// uses the decoded byte count instead of `strlen(js)`. This stops
    /// embedded-NUL JSON strings from silently truncating into a
    /// successful variant match (carry from C16B-005). New total:
    /// 904,246.
    ///
    /// C19 (2026-04-19): +195 bytes in core.c (two new forward
    /// declarations `taida_os_run_interactive` /
    /// `taida_os_exec_shell_interactive`) and +4,228 bytes in os.c
    /// (new `taida_os_process_inner_code_only` / `taida_os_extract_wait_code`
    /// helpers plus the two TTY-passthrough functions). Fragment 1 grew
    /// by +195 bytes (boundary now at 211,525); fragment 2 is unchanged.
    /// New total: 908,669.
    ///
    /// C19B-001 (2026-04-19): +19 bytes in core.c (added `#include <fcntl.h>`
    /// preamble required for `FD_CLOEXEC` / `F_SETFD` in the errno pipe used
    /// by the interactive exec helpers) and +4,429 bytes in os.c (rewrote
    /// `taida_os_run_interactive` / `taida_os_exec_shell_interactive` to
    /// propagate child `execvp` errno to the parent via a CLOEXEC self-pipe
    /// so ENOENT surfaces as `IoError` instead of `ProcessError(127)`,
    /// plus two shared `taida_os_write_all` / `taida_os_read_all` helpers).
    /// Fragment 1 grew by +19 bytes (boundary now at 211,544); fragment 2
    /// is unchanged. Intermediate total: 913,117.
    ///
    /// C19B-002 (2026-04-19): +396 bytes in core.c — introduced the
    /// `HASH___ERROR` field-hash constant (FNV-1a of `"__error"`) and
    /// threaded it through `taida_gorillax_new` / `taida_gorillax_err` /
    /// `taida_gorillax_relax` so Taida code can actually look up
    /// `gorillax.__error.<field>` at runtime. Before this fix the slot was
    /// stored under `HASH___DEFAULT` and field access silently missed.
    /// This unblocks the failure-path parity test which asserts that
    /// ENOENT surfaces as an observable `IoError` on all three backends.
    /// Fragment 1 grew by +396 bytes (boundary now at 211,940); fragment 2
    /// is unchanged. New total: 913,513.
    ///
    /// C20-3 (2026-04-20): +1,106 bytes in core.c — rewrote
    /// `taida_io_stdin` to use `getline` (POSIX) / realloc-loop
    /// (Windows) instead of a fixed `char[4096]` stack buffer so long
    /// stdin lines are no longer truncated (ROOT-8). Also keeps JS /
    /// Interpreter `""`-on-error behaviour (ROOT-9) — Interpreter
    /// side-change is Rust, not C. Fragment 1 is unchanged; fragment 2
    /// grew from 123,746 to 124,852. New total: 914,619.
    ///
    /// C20-4 (2026-04-20): +3,186 bytes in tls.c — added the
    /// list-of-record headers shape `@[@(name <= "...", value <= "...")]`
    /// for `HttpRequest` (C19B-007), complementing the legacy
    /// buchi-pack identifier shape. Two helpers
    /// (`taida_os_http_append_header_line`,
    /// `taida_os_http_pack_str_field`) were introduced and both
    /// `taida_os_http_headers_to_lines` and the curl header loop in
    /// `taida_os_http_do_curl` now accept both shapes. core.c and
    /// other fragments are unchanged. New total: 917,805.
    ///
    /// C20-2 (2026-04-20): +8,477 bytes in core.c — added the
    /// UTF-8-aware `taida_io_stdin_line` line editor (derived from
    /// linenoise BSD-2-Clause) so that `stdinLine(prompt) ]=> line`
    /// returns an `Async[Lax[Str]]` that survives multibyte input
    /// editing on a POSIX TTY (fixes ROOT-7). Non-TTY fallback uses
    /// getline to keep pipe / redirect parity with the other two
    /// backends. Include of `<termios.h>` / `<unistd.h>` is local to
    /// this block. Other fragments unchanged. New total: 926,282.
    ///
    /// C21-4 (2026-04-21): +3,228 bytes in core.c — FLOAT-tag dispatch
    /// in `taida_io_stdout_with_tag` / `taida_io_stderr_with_tag`
    /// (decode the boxed f64 bit-pattern via `memcpy` and render through
    /// `taida_float_to_str`) and the `taida_float_to_str` formatter
    /// rewritten to match the interpreter's Rust-f64::Display contract
    /// ("X.0" for integer-valued floats, shortest-round-trip via a
    /// `%.*g` + `strtod` loop for non-integers — matches Grisu/Ryu).
    /// Fixes the seed-03 / C21B-008 family: `stdout(triple(4.0))` now
    /// prints `12.0` on native (was a raw i64 bit pattern) and avoids
    /// spurious `3.14 → 3.1400000000000001` rendering. New total:
    /// 929,510.
    ///
    /// C21B-seed-07 (2026-04-22): +6,295 bytes in core.c — primitive-mold
    /// Lax constructors (`taida_{int,float,bool,str}_mold_*`) now stamp
    /// the output primitive tag on the Lax's `__value` / `__default`
    /// fields via a new `taida_lax_tag_value_default` helper; the pack
    /// display paths (`taida_pack_to_display_string` / `_full`) consult
    /// the per-field tag before falling back to the legacy global
    /// field-name/type registry; and `taida_io_stdout_with_tag` /
    /// `_stderr_with_tag` route any runtime-detected BuchiPack (Lax /
    /// Result / Gorillax / user-defined) through the full-form display.
    /// Fixes the C21B-seed-07 symptom where `stdout(Float[x]())` decoded
    /// the Lax pointer as an f64 bit pattern via the FLOAT fast path (→
    /// `3.958e-315`) and `stdout(Int[x]())` printed the short
    /// `.toString()` form `Lax(3)` instead of the interpreter's full
    /// `@(hasValue <= true, __value <= 3, __default <= 0, __type <=
    /// "Lax")`. F1 moves from 214,240 to 216,753; F2 moves from 134,257
    /// to 138,039. New total: 935,805.
    ///
    /// C23B-003 reopen (2026-04-22): +6,301 bytes in core.c. Added
    /// `taida_hashmap_to_display_string_full` and
    /// `taida_set_to_display_string_full` (synthetic full-form pack
    /// renderers mirroring the interpreter's
    /// `BuchiPack(__entries/__items, __type)` layout for HashMap/Set);
    /// `taida_stdout_display_string` now routes through those helpers so
    /// `Str[hm]()` / `Str[s]()` emit the interpreter's full form instead
    /// of the short-form `HashMap({…})` / `Set({…})`. Additionally,
    /// `taida_register_lax_field_names` also registers `__error`, and
    /// `taida_gorillax_new` calls the registration + tags its `__error`
    /// slot as `TAIDA_TAG_PACK` so `taida_pack_to_display_string_full`
    /// renders Unit as `@()` (matches interpreter `Value::Unit.to_debug_string`)
    /// — fixes `Str[Gorillax[v]()]()` rendering as `@()`. F1 moves from
    /// 216,753 to 223,054 (the other fragments are unchanged). New
    /// total: 943,160.
    ///
    /// C23B-003 reopen 2 (2026-04-22): F2 grew by +7,037 bytes from the
    /// new `taida_value_to_debug_string_full` helper + the three call
    /// sites in `taida_hashmap_to_display_string_full` /
    /// `taida_set_to_display_string_full` /
    /// `taida_pack_to_display_string_full` that route nested typed
    /// runtime objects (HashMap / Set / BuchiPack) through the full-form
    /// recursion, plus a List branch in `taida_stdout_display_string`
    /// that uses the same full-form helper so top-level
    /// `Str[@[hashMap()...]]()` produces `@[@(__entries <= …, __type <=
    /// "HashMap"), …]` instead of collapsing the items to the
    /// `HashMap({…})` short form. Without these changes, nested
    /// HashMap-in-HashMap / Set-in-HashMap / List-of-HashMap collapsed
    /// back to short-form `.toString()` output, breaking 4-backend
    /// parity with the interpreter's `Value::to_debug_string()` →
    /// recursive `to_display_string()` contract on BuchiPack. Total:
    /// 943,160 → 950,197.
    ///
    /// C23B-007 / C23B-008 (2026-04-22): 950,197 → 958,672 (+8,475).
    /// - C23B-007 native symmetry: introduced `TAIDA_TAG_HETEROGENEOUS
    ///   = -2`, taught `taida_list_set_elem_tag` /
    ///   `taida_hashmap_set_value_tag` to latch on it once two
    ///   disagreeing primitive tags collide. Retain/release keep the
    ///   UNKNOWN leak-rather-than-crash behaviour for the new sentinel.
    /// - C23B-008 native HashMap insertion-order side-index: added
    ///   macros (`TAIDA_HM_ORD_HEADER_SLOT`, `TAIDA_HM_ORD_SLOT`,
    ///   `TAIDA_HM_TOTAL_SLOTS`) and grew the allocation by `1 + cap`
    ///   slots. `taida_hashmap_set` / `_remove` / `_clone` /
    ///   `_resize` / `_entries` / `_keys` / `_values` / `_merge` /
    ///   `taida_hashmap_to_string` / `taida_hashmap_to_display_string_full` /
    ///   JSON serializer walk the new side-index.
    #[test]
    fn test_native_runtime_fragment_concat_preserves_bytes() {
        // C24-B (2026-04-23): +4,014 bytes in core.c total. F1 region
        // grew by +1,697 bytes (+1,114 zip/enumerate field-name
        // registration helper, +583 TAIDA_TAG_STR stamps on Lax /
        // Gorillax / RelaxedGorillax `__type` slots). F2 region grew by
        // +2,317 bytes (explicit `render_int` / `render_str` branches in
        // `taida_pack_to_display_string_full` so INT / STR fields
        // stamped via `taida_pack_set_tag` no longer fall into the
        // pointer-dereference path and segfault). See the F1_LEN test
        // body below for the detailed breakdown.
        //
        // C25B-028 (2026-04-23, commit 48d26da): +5,544 bytes in core.c
        // from the `jsonEncode(Gorillax/Lax/Result)` 4-backend parity fix.
        // Monadic-pack detection + `__error` / `__value` / `__default` /
        // `__predicate` / `throw` / `hasValue` emission paths were added
        // to `json_serialize_pack_fields` so native now matches the
        // interpreter's `{"__error":{},"__value":42,"hasValue":true}`
        // output instead of dropping fields / emitting booleans as 1/0.
        // Split across F1 and F2 — see the F1_LEN body below for the
        // per-region accounting.
        //
        // C25B-001 Phase 3 (2026-04-23, commit 4e17e89): +3,200 bytes in
        // core.c from the minimal Stream lowering (`taida_stream_new` /
        // `taida_stream_is_stream` / `taida_stream_to_display_string` +
        // routing from `taida_stdout_display_string`) that closes the
        // native Stream parity gap covered by `STREAM_ONLY_FIXTURES`.
        // Split across F1 and F2 — see the F1_LEN body below.
        //
        // Cumulative C25 delta: +8,744 bytes on core.c. Other fragments
        // (os / tls / net_h1_h2 / net_h3_quic) are unchanged. New total:
        // 965,529 → 974,273.
        //
        // C25B-025 Phase 5-I (2026-04-23): +1,895 bytes in core.c from
        // the math mold family (taida_float_sqrt / _pow / _exp / _ln /
        // _log2 / _log10 / _log / _sin / _cos / _tan / _asin / _acos /
        // _atan / _atan2 / _sinh / _cosh / _tanh). All 17 helpers are
        // thin wrappers over glibc libm (linked via -lm in driver.rs)
        // which on x86_64-linux / aarch64-linux is the same libm that
        // Rust's `f64::sqrt` / `f64::exp` / ... delegate to via the
        // LLVM `@llvm.*.f64` intrinsics — giving bit-for-bit parity
        // with the interpreter. Inserted ahead of the `// ── Error
        // ceiling` marker so F1_LEN absorbs the full delta; F2
        // (error/display/stdout-display) is unchanged. F1_LEN moves
        // from 233,853 to 235,748; F2_LEN stays at 159,407. Total
        // core.c size moves from 393,260 to 395,155. Other fragments
        // (os / tls / net_h1_h2 / net_h3_quic) are unchanged. New
        // total: 974,273 → 976,168.
        //
        // C26B-011 Phase 11 (2026-04-24): +3,834 bytes in core.c from
        // the Float-origin parity work for `Div` / `Mod` / math molds.
        //   - F1: +2,889 bytes (`taida_div_mold_f` + `taida_mod_mold_f`
        //     placed after `taida_lax_tag_value_default` definition,
        //     `taida_pack_get_tag_idx` positional getter near the
        //     pack helpers, `taida_float_to_str` forward declaration
        //     + doc comment, `taida_debug_float` rewrite to route
        //     through `taida_float_to_str` for NaN/Inf/"X.0" parity).
        //   - F2: +945 bytes (`taida_lax_to_string` now honours FLOAT
        //     tag on `__value` / `__default` slots so Float-origin
        //     `Lax(0.0)` doesn't render as `Lax(0)` via the untagged
        //     `taida_value_to_display_string` fallback).
        // C26B-020 柱 1 (@c.26): readBytesAt forward decl added to
        // core.c (+140 bytes in F1, before the Error ceiling) and
        // taida_os_read_bytes_at function body added to os.c
        // (additive +2,135 bytes total, including the F1 forward decl).
        // F1_LEN moves from 235,748 → +140 (P10) +2,889 (P11) → 238,777.
        // F2_LEN moves from 159,407 → +945 (P11) → 160,352.
        // C26B-021 (@c.26): +839 bytes in net_h3_quic.c for the
        // `setvbuf(stdout/stderr, _IOLBF, 0)` stdout line-buffering fix
        // at the top of main(). Does not affect core.c so F1_LEN /
        // F2_LEN unchanged. Only the grand total shifts.
        // C26B-026 (@c.26 Round 2, wC): +617 bytes in net_h1_h2.c to fix
        // `h2_extract_response_fields` silently dropping custom response
        // headers. Previously `taida_list_get` returned a Lax-wrapped
        // entry and the "name" / "value" lookup on the wrapper returned 0,
        // so every handler-returned header was filtered out before HPACK
        // encoding (wire response ended up with only `:status` +
        // `content-length`). Fix reads `hlist[4 + j]` directly to mirror
        // the h1 encode path. Also raises the in-function header cap from
        // 32 to H2_MAX_HEADERS (128) for parity with the HPACK block
        // encoder which already allows 128. Does not affect core.c so
        // F1_LEN / F2_LEN unchanged.
        // Combined delta on top of 976,168:
        //   +3,834 (C26B-011 core.c)
        //   +2,135 (C26B-020 os.c + F1 forward decl)
        //   +  839 (C26B-021 net_h3_quic.c)
        //   +  617 (C26B-026 net_h1_h2.c)
        // New total after Round 1: 976,168 + 7,425 = 983,593.
        //
        // C26B-016 (@c.26, Option B+, Round 2 wD): +5,339 bytes in core.c
        // (F1) from the span-aware comparison mold helpers
        // (`taida_net_span_extract`, `taida_net_raw_as_bytes`,
        // `taida_net_needle_as_bytes`, `taida_net_SpanEquals` /
        // `SpanStartsWith` / `SpanContains` / `SpanSlice`). All four public
        // helpers are byte-level comparisons over a `@(start, len)` span
        // pack view into a Bytes/Str raw buffer, matching interpreter +
        // JS parity. Placed immediately before the `// ── Error ceiling`
        // divider so F1 absorbs the full delta; F2 unchanged.
        // C26B-026 (@c.26, Round 2 wC): +617 bytes in net_h1_h2.c for
        // the HPACK custom header preservation fix (h2_extract_response_fields
        // switched from Lax-wrapping taida_list_get() to hlist[4+j] raw
        // inner pack access) + header_cap 32 → H2_MAX_HEADERS (128).
        // Combined Round 1 + Round 2 delta on top of 976,168:
        //   +3,834 (C26B-011 core.c)
        //   +2,135 (C26B-020 os.c + F1 forward decl)
        //   +  839 (C26B-021 net_h3_quic.c)
        //   +5,339 (C26B-016 core.c F1 span mold helpers)
        //   +  617 (C26B-026 net_h1_h2.c HPACK fix)
        //   +3,153 (C26B-018 (B)(C) core.c F1 byte-level primitives
        //            + StringRepeatJoin: forward decls + 5 fn impls)
        // Round 6 (wS, 2026-04-24) adds:
        //   +1,904 (C26B-022 Step 2 net_h1_h2.c: method 16 / path 2048 /
        //            Host-value 256 wire-byte reject in
        //            taida_net_http_parse_request_head; 400 Bad Request
        //            parity with Interpreter h1 parser)
        //   +  511 (C26B-011 core.c: signed-zero branch in
        //            taida_float_to_str — emits "-0.0" when
        //            signbit(a) != 0, matching Rust f64::Display +
        //            interpreter `format!("{:.1}", -0.0)` output)
        // New total: 992,085 + 1,904 + 511 = 994,500.
        const EXPECTED_TOTAL_LEN: usize = 994_500;
        let asm = *NATIVE_RUNTIME_C;
        assert_eq!(
            asm.len(),
            EXPECTED_TOTAL_LEN,
            "native_runtime fragments concatenate to unexpected size. \
             If you modified the C source deliberately, update EXPECTED_TOTAL_LEN."
        );
        // Anchor the first bytes of the assembled source to the historical
        // stdio.h include so accidental reordering of fragments is caught.
        assert!(
            asm.starts_with("#include <stdio.h>\n"),
            "first bytes of assembled source must start with <stdio.h> include"
        );
        // Anchor the tail of the assembled source to the closing brace of
        // main() — catches accidental truncation of the tail fragment.
        assert!(
            asm.trim_end()
                .ends_with("(void)_taida_main();\n    return 0;\n}"),
            "tail of assembled source must end with main() body + closing brace"
        );
    }

    /// Each fragment must be a proper C suffix / prefix — no fragment
    /// should begin mid-statement. Fragment `core.c` starts with the
    /// `#include` preamble; the other four each begin with a `// ──`
    /// section divider comment at column 0 (inherited from C12B-026).
    #[test]
    fn test_native_runtime_fragment_boundaries_are_top_level() {
        assert!(
            CORE_SECTION.starts_with("#include <stdio.h>"),
            "fragment 'core' must begin with the <stdio.h> include"
        );
        for (name, frag) in [
            ("os", OS_SECTION),
            ("tls", TLS_SECTION),
            ("net_h1_h2", NET_H1_H2_SECTION),
            ("net_h3_quic", NET_H3_QUIC_SECTION),
        ] {
            let first = frag.lines().next().unwrap_or("");
            assert!(
                first.starts_with("// ──") || first.starts_with("/*") || first.is_empty(),
                "fragment '{}' must begin at a top-level boundary (found: {:?})",
                name,
                first
            );
        }
    }

    /// Smoke test that none of the fragments are empty or suspiciously
    /// small (would indicate a boundary mis-calculation).
    /// Lower bounds chosen with a comfortable margin below the actual
    /// C13-4 sizes (see `mod.rs` docstring for the observed line counts).
    #[test]
    fn test_native_runtime_fragments_nonempty() {
        assert!(
            CORE_SECTION.len() > 150_000,
            "core fragment suspiciously small"
        );
        assert!(OS_SECTION.len() > 10_000, "os fragment suspiciously small");
        assert!(
            TLS_SECTION.len() > 30_000,
            "tls fragment suspiciously small"
        );
        assert!(
            NET_H1_H2_SECTION.len() > 150_000,
            "net_h1_h2 fragment suspiciously small"
        );
        assert!(
            NET_H3_QUIC_SECTION.len() > 100_000,
            "net_h3_quic fragment suspiciously small"
        );
    }

    /// C13-4 invariant: the five responsibility fragments must concatenate
    /// in the order `core -> os -> tls -> net_h1_h2 -> net_h3_quic`, which
    /// is byte-identical to the C12B-026 seven-fragment order
    /// `01_core -> 02_error_json -> 03_os -> 04_tls_tcp -> 05_net_v1 ->
    /// 06_net_h2 -> 07_net_h3_main`. The fragment 1+2 merge (core) and
    /// the fragment 5+6 merge (net_h1_h2) must preserve the historical
    /// byte boundaries — we verify this by sampling the exact byte
    /// offsets where the former fragment boundaries used to sit.
    #[test]
    fn test_native_runtime_c13_4_merge_preserves_historical_boundaries() {
        // Fragment 1 (01_core.inc.c) was exactly 209,911 bytes in
        // C12B-026. After C13-4 merge into core.c, the byte at offset
        // 209,911 inside CORE_SECTION must be the first byte of the
        // former fragment 2 (02_error_json.inc.c), which historically
        // begins with "// ── Error ceiling".
        // NOTE: we compare bytes (not &str slices) because the "─" box
        // drawing character is multi-byte UTF-8 and a naive &str slice
        // of fixed length would land mid-char.
        // C16 (2026-04-16): legacy fragment 2 grew by +2,074 bytes to host
        // the `E{...}` enum schema descriptor logic. Fragment 1 boundary at
        // offset 209,911 is unchanged — the F2_PREFIX anchor still lands
        // exactly on the former error_json section header.
        //
        // C16B-001 (2026-04-16): legacy fragment 2 grew by a further +4,359
        // bytes when `json_default_value_for_desc` was rewritten onto the new
        // `json_pure_default_apply` walker (TypeDef defaults now embed
        // `Int(0)` for Enum fields — fixes Interp/JS/Native parity regression
        // detected in Phase 7 post-review). Fragment 1 boundary is still at
        // offset 209,911.
        //
        // C18-2 (2026-04-17): fragment 1 grew by +305 bytes (new
        // `taida_register_field_enum` forward declaration) and fragment 2
        // grew by +3,693 bytes (new registration helper, enum variant
        // emitter, tag-5 branch in `json_serialize_pack_fields`). Total
        // core.c growth is +3,998 bytes, splitting across the historical
        // fragment boundary. The F2_PREFIX anchor lands at offset
        // 210,216 (was 209,911). These changes let jsonEncode emit the
        // variant-name Str (e.g. `"Running"`) in symmetry with the C16
        // `JSON[raw, Schema]()` decoder.
        //
        // C18B-003/005 (2026-04-17): fragment 1 grew by +1,114 bytes
        // (forward declaration for `taida_register_pack_field_enum` +
        // the `taida_runtime_panic` helper for C18B-005) and fragment 2
        // grew by +3,972 bytes (per-pack enum registry storage + two
        // register/lookup helpers + the preference-for-per-pack branch
        // in `json_serialize_pack_fields`). F2_PREFIX now lands at
        // offset 211,330.
        //
        // C18B-006 (2026-04-17): fragment 1 unchanged. Fragment 2 grew
        // by +2,272 bytes (length-aware JSON string parser variant
        // `json_parse_string_raw_len` + `str_len` field plumbing +
        // `memcmp` length check in the `E{...}` enum branch of
        // `json_apply_schema`).
        // C19 (2026-04-19): fragment 1 grew by +195 bytes (two new
        // `taida_os_run_interactive` / `taida_os_exec_shell_interactive`
        // forward declarations added near the other os function prototypes).
        // Fragment 2 is unchanged. F1_LEN moves from 211,330 to 211,525.
        //
        // C19B-001 (2026-04-19): fragment 1 grew by another +19 bytes from
        // the `#include <fcntl.h>` preamble required by the errno-pipe
        // (FD_CLOEXEC) used inside the interactive exec helpers. Fragment
        // 2 is still untouched. F1_LEN moves from 211,525 to 211,544.
        //
        // C19B-002 (2026-04-19): fragment 1 grew by another +396 bytes from
        // the `HASH___ERROR` constant introduction and the Gorillax field-
        // hash corrections in `taida_gorillax_new` / `_err` / `_relax` so
        // `.__error.<field>` actually resolves at runtime. Fragment 2 is
        // unchanged. F1_LEN moves from 211,544 to 211,940.
        //
        // C20-3 (2026-04-20): fragment 2 grew by +1,106 bytes from the
        // dynamic-buffer rewrite of `taida_io_stdin` (ROOT-8 fix:
        // `getline` on POSIX / realloc-loop on Windows replacing the
        // fixed 4 KiB stack buffer). Fragment 1 is unchanged. Fragment 2
        // size moves from 123,746 to 124,852.
        //
        // C20-2 (2026-04-20): fragment 1 grew by +178 bytes from the
        // `taida_io_stdin_line` forward declaration (and its doc-comment
        // header). Fragment 2 grew by +8,299 bytes from the UTF-8-aware
        // line editor body (static helpers + termios raw-mode loop).
        // F1 moves from 211,940 to 212,118; F2 moves from 124,852 to
        // 133,151.
        //
        // C21-4 (2026-04-21): fragment 1 grew by +2,122 bytes from the
        // `taida_float_to_str` rewrite (Rust-display-compatible "X.0"
        // integer output + shortest-round-trip `%.*g`/`strtod` loop for
        // non-integers) and the doc comment for it. Fragment 2 grew by
        // +1,106 bytes from the FLOAT-tag fast paths added to
        // `taida_io_stdout_with_tag` and `taida_io_stderr_with_tag`
        // (seed-03 / C21B-008 fix — `stdout(triple(4.0))` prints `12.0`
        // instead of the boxed f64 bit pattern). F1 moves from 212,118
        // to 214,240; F2 moves from 133,151 to 134,257.
        //
        // C21B-seed-07 (2026-04-22): fragment 1 grew by +2,513 bytes —
        // the primitive-mold tag-stamp helper, reworked primitive
        // mold constructors (`taida_{int,float,bool,str}_mold_*`), and
        // per-field-tag dispatch branches in
        // `taida_pack_to_display_string` / `_full`. Fragment 2 grew by
        // +3,782 bytes — buchi-pack detour branches in
        // `taida_io_stdout_with_tag` / `taida_io_stderr_with_tag`
        // routing any runtime-detected pack through
        // `taida_stdout_display_string` so interpreter-parity
        // `@(hasValue <= …, __value <= …, __default <= …, __type <=
        // "Lax")` output is preserved for Lax returns from
        // `Int[]/Float[]/Bool[]/Str[]`. F1 moves from 214,240 to
        // 216,753; F2 moves from 134,257 to 138,039.
        //
        // C23-2 (2026-04-22): fragment 1 grew by +1,054 bytes — forward
        // declaration of `taida_stdout_display_string` plus the new
        // generic `taida_str_mold_any` helper (routes `Str[x]()` for
        // non-primitive values through the full-form stdout-display
        // helper, fixing C23B-002 raw-pointer stringification of
        // `Str[@[…]]` / `Str[@(…)]` / `Str[Int[3.0]()]` on Native).
        // Fragment 2 is unchanged. F1 moves from 216,753 to 217,807;
        // F2 stays at 138,039.
        //
        // C23B-003 reopen (2026-04-22): core.c grew by +6,301 bytes
        // across the F1 and F2 regions. F1 (core primitives, bytes
        // [0..F1_LEN)) grew by +965 bytes via the
        // `taida_register_lax_field_names` addition and the
        // Gorillax-constructor registration calls plus the new
        // `render_unit_pack` branch in `taida_pack_to_display_string_full`.
        // F2 (error/display/stdout-display helpers, bytes [F1_LEN..))
        // grew by +5,336 bytes for `taida_hashmap_to_display_string_full`
        // / `taida_set_to_display_string_full` / the
        // `taida_stdout_display_string` routing update. F1 moves from
        // 217,807 to 218,772; F2 moves from 138,039 to 143,375.
        //
        // C23B-003 reopen 2 (2026-04-22): F2 grew by another +7,037
        // bytes — `taida_value_to_debug_string_full` (recursive
        // debug-string variant for nested typed runtime objects), the
        // forward declarations, the three call-site swaps inside the
        // existing `*_to_display_string_full` helpers, and the new
        // List-branch inside `taida_stdout_display_string` that uses
        // the full-form helper on list items. F1 is unchanged. F2
        // moves from 143,375 to 150,412.
        //
        // C23B-007 / C23B-008 (2026-04-22): F1 grew by +7,710 bytes
        // and F2 grew by +765 bytes.
        // - F1 (core primitives) absorbed:
        //   (a) `TAIDA_TAG_HETEROGENEOUS = -2` define plus the three
        //       `TAIDA_HM_ORD_*` layout macros (C23B-007 / C23B-008).
        //   (b) `taida_list_set_elem_tag` / `taida_hashmap_set_value_tag`
        //       downgrade logic bodies (C23B-007).
        //   (c) HashMap insertion-order side-index scaffolding:
        //       `_new_with_cap` allocation bump, `_set` / `_resize` /
        //       `_remove` / `_clone` / `_keys` / `_values` / `_entries`
        //       / `_merge` / `_to_string` all walk the new side-index
        //       (C23B-008).
        // - F2 (error + display helpers) absorbed:
        //   the `taida_hashmap_to_display_string_full` walk switch and
        //   the `json_serialize_typed` HashMap branch rewrite.
        // F1 moves from 218,772 to 226,482; F2 moves from 150,412 to
        // 151,177.
        //
        // C23B-008 reopen (2026-04-22): F1 grew by +1,935 bytes. The
        // native `taida_hashmap_merge` was rewritten from
        // "clone self; for each other entry call taida_hashmap_set"
        // (which preserves self's ordinal for overlap keys because
        // `taida_hashmap_set` updates in place) to the interpreter's
        // retain-then-push algorithm (fresh result map; fill with
        // self-entries whose key ∉ other in self-order; then append
        // every other entry in other-order). The previous implementation
        // emitted `[a, b, c, d]` for `a=[a,b].merge([c,b,d])`; interpreter
        // emits `[a, c, b, d]`. Fix is body-only inside F1. F2 is
        // unchanged.
        // F1 moves from 226,482 to 228,417; F2 stays at 151,177.
        //
        // C23B-009 (2026-04-22): F1 grew by +908 bytes. The native
        // `taida_hashmap_entries` now idempotently registers the `"key"` /
        // `"value"` field names via `taida_register_field_name` so that
        // `taida_pack_to_display_string_full` resolves them (previously
        // lookup returned NULL and every pair pack rendered as `@()`,
        // diverging from interpreter / JS / documented
        // `docs/reference/standard_library.md:238` shape). F2 is unchanged.
        // F1 moves from 228,417 to 229,325; F2 stays at 151,177.
        //
        // C24-B (2026-04-23): F1 grew by +1,114 bytes; F2 grew by +2,317.
        // - F1: added the `taida_register_zip_enumerate_field_names`
        //   helper + idempotent registration calls at the head of
        //   `taida_list_zip` / `taida_list_enumerate` so the `first` /
        //   `second` / `index` / `value` field names resolve in
        //   `taida_pack_to_display_string_full` (previously unregistered
        //   → NULL → every pair pack rendered as `@()`, which segfaulted
        //   when the outer list's elem_type_tag = TAIDA_TAG_PACK forced
        //   the full-form recursion to deref into the pair's unresolved
        //   slots). Field hashes themselves were already defined
        //   (`HASH_FIRST` / `HASH_SECOND` / `HASH_INDEX` / `HASH_VALUE`)
        //   and already stamped on each pair; the missing piece was
        //   name registration.
        // - F2: added explicit `render_int` and `render_str` branches
        //   inside `taida_pack_to_display_string_full` (symmetric with
        //   the WASM version's C23B-005 guards). Before this change,
        //   an INT-tagged pack field fell into the generic
        //   `taida_value_to_debug_string_full(field_val)` which
        //   dereferenced a small int (e.g. `1`) as `(char*)1` and
        //   segfaulted in `taida_read_cstr_len_safe`. The guard uses
        //   `!render_bool && !render_unit_pack` so Lax's `hasValue`
        //   (INT tag + legacy ftype-4 registry hint) continues to
        //   render as `true` / `false`, and Gorillax's `__error`
        //   (PACK tag + field_val == 0) continues to render as `@()`.
        // F1 moves from 229,325 to 231,022 (+1,697 total); F2 moves
        // from 151,177 to 153,494 (+2,317).
        // F1 breakdown:
        //  - +1,114 bytes for zip/enumerate field-name registration
        //    (`taida_register_zip_enumerate_field_names` helper + calls)
        //  - +583 bytes for `TAIDA_TAG_STR` stamps on the `__type` slot
        //    of `taida_lax_new` / `taida_lax_empty` / `taida_gorillax_new`
        //    / `taida_gorillax_err` / `taida_gorillax_relax`. Without
        //    these, the new `render_int` branch in F2 intercepts the
        //    INT-defaulted `__type` slot (stored pointer, tag left at 0)
        //    and renders the string pointer as a decimal integer
        //    (repro: `Str[@[Gorillax[42]()]]()` emitted
        //    `__type <= 4522605`). The string is a static C literal so
        //    TAIDA_TAG_STR is rendering-only — release / free paths
        //    continue to skip via the existing `value > 4096` gate.
        //
        // C25B-028 (commit 48d26da): core.c grew by +5,544 bytes from
        // the jsonEncode(Gorillax/Lax/Result) 4-backend parity fix
        // (monadic-pack detection + `__error`/`__value`/`__default`/
        // `__predicate`/`throw`/`hasValue` emission in
        // `json_serialize_pack_fields`).
        //
        // C25B-001 Phase 3 (commit 4e17e89): core.c grew by +3,200 bytes
        // from the minimal Stream lowering (`taida_stream_new` /
        // `taida_stream_is_stream` / `taida_stream_to_display_string` +
        // routing hook in `taida_stdout_display_string`). Closes the
        // native Stream parity gap previously skipped via
        // `STREAM_ONLY_FIXTURES`.
        //
        // Aggregate C25 delta (core.c only, both commits combined):
        //  - F1 (bytes [0..F1_LEN), ending just before "// ── Error
        //    ceiling"): +2,831 bytes. F1_LEN moves from 231,022 to
        //    233,853.
        //  - F2 (bytes [F1_LEN..end)): +5,913 bytes. F2_LEN moves from
        //    153,494 to 159,407.
        //  - Per-commit F1/F2 split is absorbed into the aggregate: we
        //    re-anchor against the observed byte offset of the
        //    "Error ceiling" marker rather than trying to attribute
        //    each byte to a specific commit, since the two commits
        //    were landed consecutively and together form the current
        //    C25 native_runtime drift.
        // C25B-025 Phase 5-I: F1 absorbs +1,895 bytes (math mold family)
        // inserted ahead of the "Error ceiling" marker. F2 unchanged.
        // C26B-011 Phase 11: F1 absorbs +2,889 bytes (Float-hint Div/Mod
        // variants, `taida_pack_get_tag_idx` helper, `taida_float_to_str`
        // forward declaration, `taida_debug_float` rewrite). F2 absorbs
        // +945 bytes (`taida_lax_to_string` FLOAT-tag aware rendering).
        // C26B-020 柱 1 (@c.26): F1 absorbs +140 bytes for the
        // taida_os_read_bytes_at forward declaration. F2 unchanged.
        // C26B-016 (@c.26, Option B+): F1 absorbs +5,339 bytes for the
        // span-aware comparison mold helpers (`taida_net_span_extract`,
        // `taida_net_raw_as_bytes`, `taida_net_needle_as_bytes`,
        // `taida_net_SpanEquals` / `SpanStartsWith` / `SpanContains` /
        // `SpanSlice`). All added immediately before the "Error ceiling"
        // marker so F1 absorbs the full delta. F2 unchanged.
        // F1_LEN moves: 238,777 + 5,339 = 244,116.
        // C26B-018 (B)(C) (@c.26, wK Round 4): F1 absorbs +3,261 bytes
        // for the byte-level primitives (`taida_str_byte_at`,
        // `taida_str_byte_at_lax`, `taida_str_byte_slice`,
        // `taida_str_byte_length`) and `taida_str_repeat_join` plus
        // their 5 forward declarations. Functions were inserted
        // immediately after `taida_str_repeat` — well before the
        // "Error ceiling" marker — so F1 absorbs the full delta. F2
        // unchanged.
        // F1_LEN moves: 244,116 + 3,153 = 247,269.
        // C26B-011 (@c.26, wS Round 6, 2026-04-24): F1 absorbs +511 bytes
        // for the signed-zero branch in `taida_float_to_str` (emits
        // "-0.0" when signbit(a) != 0). `taida_float_to_str` sits at
        // line ~4007 — well before the "Error ceiling" marker — so F1
        // absorbs the full delta. F2 unchanged.
        // F1_LEN moves: 247,269 + 511 = 247,780.
        const F1_LEN: usize = 247_780;
        assert_eq!(
            CORE_SECTION.len(),
            247_780 + 160_352,
            "core.c total byte length must equal legacy fragment1 + fragment2 (C25B-001 / C25B-028 / C25B-025 / C26B-011 / C26B-020 / C26B-016 / C26B-018 / C26B-011-wS adjusted)"
        );
        const F2_PREFIX: &[u8] = b"// \xE2\x94\x80\xE2\x94\x80 Error ceiling";
        let tail = &CORE_SECTION.as_bytes()[F1_LEN..F1_LEN + F2_PREFIX.len()];
        assert_eq!(
            tail, F2_PREFIX,
            "byte offset {} inside core.c must begin the legacy error_json fragment",
            F1_LEN
        );

        // Fragment 5 (05_net_v1.inc.c) was exactly 184,963 bytes in
        // C12B-026. After C13-4 merge into net_h1_h2.c, the byte at
        // offset 184,963 inside NET_H1_H2_SECTION must be the first byte
        // of the former fragment 6 (06_net_h2.inc.c), which historically
        // begins with "// ── Native HTTP/2 server".
        //
        // C26B-026 (@c.26 Round 2, wC): fragment 6 (net_h2 server) gained
        // +617 bytes for the HPACK custom header fix inside
        // `h2_extract_response_fields` (Lax unwrap via raw `hlist[4+j]`
        // + header_cap raised to H2_MAX_HEADERS). F5_LEN unchanged;
        // fragment 6 baseline moves from 91,152 to 91,769.
        // C26B-022 Step 2 (@c.26, wS Round 6, 2026-04-24): fragment 5
        // (HTTP/1 parser + worker) absorbs +1,904 bytes for the
        // parser-level wire-byte reject in
        // `taida_net_http_parse_request_head` (method 16 / path 2048 /
        // Host-value 256). Inserted entirely before the "// ── Native
        // HTTP/2 server" divider, so F5 grows and F6 is unchanged.
        // F5_LEN moves: 184,963 + 1,904 = 186,867.
        const F5_LEN: usize = 186_867;
        assert_eq!(
            NET_H1_H2_SECTION.len(),
            186_867 + 91_769,
            "net_h1_h2.c total byte length must equal legacy fragment5 + fragment6 (C26B-026 / C26B-022-wS adjusted)"
        );
        const F6_PREFIX: &[u8] = b"// \xE2\x94\x80\xE2\x94\x80 Native HTTP/2 server";
        let tail = &NET_H1_H2_SECTION.as_bytes()[F5_LEN..F5_LEN + F6_PREFIX.len()];
        assert_eq!(
            tail, F6_PREFIX,
            "byte offset {} inside net_h1_h2.c must begin the legacy net_h2 fragment",
            F5_LEN
        );
    }
}

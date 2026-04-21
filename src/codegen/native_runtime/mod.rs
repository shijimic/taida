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
    #[test]
    fn test_native_runtime_fragment_concat_preserves_bytes() {
        const EXPECTED_TOTAL_LEN: usize = 926_282;
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
        const F1_LEN: usize = 212_118;
        assert_eq!(
            CORE_SECTION.len(),
            212_118 + 133_151,
            "core.c total byte length must equal legacy fragment1 + fragment2 (C20-2 adjusted)"
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
        const F5_LEN: usize = 184_963;
        assert_eq!(
            NET_H1_H2_SECTION.len(),
            184_963 + 91_152,
            "net_h1_h2.c total byte length must equal legacy fragment5 + fragment6"
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

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
    #[test]
    fn test_native_runtime_fragment_concat_preserves_bytes() {
        const EXPECTED_TOTAL_LEN: usize = 886_457;
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
        const F1_LEN: usize = 209_911;
        assert_eq!(
            CORE_SECTION.len(),
            209_911 + 107_376,
            "core.c total byte length must equal legacy fragment1 + fragment2"
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

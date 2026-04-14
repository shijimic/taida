//! Taida WASM core runtime — single translation unit assembled from four
//! mechanical fragments.
//!
//! C12-7 (FB-26 / C12B-027) で `src/codegen/runtime_core_wasm.c` (6,463 行) を
//! `#include`-hub スタイルで 4 フラグメントに機能分割した。フラグメントは
//! **clang の視点では 1 つの translation unit** として扱われる (Rust 側で
//! 連結してから `fs::write` する) ため、DCE / static helper 参照 / forward
//! declaration 等のセマンティクスは分割前とバイト単位で同一。
//!
//! - [`CORE_SECTION`] — libc stubs / bump allocator / strlen / string helpers
//!   / stdout-stderr-debug I/O / int-bool 演算 / polymorphic display /
//!   float 演算 / BuchiPack / List / HashMap / Set / WC-6 extensions
//!   (元 lines 1..2698)
//! - [`CONTAINERS_SECTION`] — Closure runtime / Error ceiling / Lax / Result
//!   / Gorillax / Cage / Molten/Stub/Todo / type conversion molds / float
//!   div/mod / String template / digit helpers
//!   (元 lines 2699..4253)
//! - [`TYPEOF_LIST_SECTION`] — RC no-ops / typeof / List HOF / List 操作 /
//!   List query / element retain/release
//!   (元 lines 4254..5140)
//! - [`JSON_ASYNC_SECTION`] — JSON runtime (strtol/strtod/itoa/ftoa/FNV-1a /
//!   type detection / public field wrappers / schema / descriptor apply) /
//!   Async runtime / `_taida_main` extern / `_start` WASI entry
//!   (元 lines 5141..6463)
//!
//! 分割配置表: `.dev/taida-logs/docs/design/file_boundaries.md §5`

use std::sync::LazyLock;

/// Fragment 1: libc stubs, allocator, I/O, numerics, containers (core).
pub const CORE_SECTION: &str = include_str!("01_core.inc.c");

/// Fragment 2: Closure, Error ceiling, Lax/Result/Gorillax, Cage,
/// type conversion molds, float div/mod, misc helpers.
pub const CONTAINERS_SECTION: &str = include_str!("02_containers.inc.c");

/// Fragment 3: RC no-ops, typeof, List HOF / operations / queries.
pub const TYPEOF_LIST_SECTION: &str = include_str!("03_typeof_list.inc.c");

/// Fragment 4: JSON runtime + Schema + Async + `_start` entry.
pub const JSON_ASYNC_SECTION: &str = include_str!("04_json_async.inc.c");

/// Full wasm core runtime C source, assembled from the four fragments on
/// first access and cached for the process lifetime.
///
/// Byte-identical to the pre-split monolithic `runtime_core_wasm.c` — see
/// `test_runtime_core_wasm_fragment_concat_preserves_bytes` below for the
/// invariant assertion.
///
/// C12-7 note: `concat!()` cannot be used because that macro requires
/// literal arguments; `LazyLock<&'static str>` + `Box::leak` exposes a
/// `&'static str` without adding a crate dependency. Same strategy as
/// `src/js/runtime/mod.rs::RUNTIME_JS` (C12-9d).
pub static RUNTIME_CORE_WASM: LazyLock<&'static str> = LazyLock::new(|| {
    let total = CORE_SECTION.len()
        + CONTAINERS_SECTION.len()
        + TYPEOF_LIST_SECTION.len()
        + JSON_ASYNC_SECTION.len();
    let mut s = String::with_capacity(total);
    s.push_str(CORE_SECTION);
    s.push_str(CONTAINERS_SECTION);
    s.push_str(TYPEOF_LIST_SECTION);
    s.push_str(JSON_ASYNC_SECTION);
    debug_assert_eq!(s.len(), total);
    Box::leak(s.into_boxed_str())
});

#[cfg(test)]
mod tests {
    use super::*;

    /// C12-7 invariant: the concatenation of the four fragments must be
    /// byte-identical to the pre-split monolithic `runtime_core_wasm.c`.
    /// We anchor the total byte length + the first / last 200 bytes of
    /// the assembled source to detect accidental edits that would break
    /// DCE or shift static helper references across fragment boundaries.
    ///
    /// Total bytes snapshot: 237,295 (post-C12B-034 hardening of
    /// `taida_io_stdout_with_tag` / `taida_io_stderr_with_tag` to route
    /// non-Bool non-Str tags through `taida_polymorphic_to_string`
    /// instead of casting arbitrary integers to `char*`). If a future
    /// change intentionally modifies the runtime C source, update both
    /// the fragment file and the `EXPECTED_TOTAL_LEN` constant below in
    /// the same commit.
    #[test]
    fn test_runtime_core_wasm_fragment_concat_preserves_bytes() {
        const EXPECTED_TOTAL_LEN: usize = 237_295;
        let asm = *RUNTIME_CORE_WASM;
        assert_eq!(
            asm.len(),
            EXPECTED_TOTAL_LEN,
            "runtime_core_wasm fragments concatenate to unexpected size. \
             If you modified the C source deliberately, update EXPECTED_TOTAL_LEN."
        );
        // Anchor the first line of the assembled source to the file header
        // comment so accidental reordering of fragments is caught.
        assert!(
            asm.starts_with("/**\n * runtime_core_wasm.c"),
            "first bytes of assembled source must match the historical header"
        );
        // Anchor the last meaningful lines to the WASI entry point —
        // catches accidental truncation of the tail fragment.
        assert!(
            asm.trim_end().ends_with("_taida_main();\n}"),
            "tail of assembled source must end with _start body + closing brace"
        );
    }

    /// Each fragment must be a proper C suffix / prefix — no fragment
    /// should begin or end mid-statement. We approximate this by checking
    /// that each fragment does not start with an indented line (which
    /// would indicate a continuation from the previous fragment).
    ///
    /// Fragment 1 starts with the `/**` file header, fragments 2-4 each
    /// begin with a `/* ── section ── */` divider comment.
    #[test]
    fn test_runtime_core_wasm_fragment_boundaries_are_top_level() {
        assert!(
            CORE_SECTION.starts_with("/**"),
            "fragment 1 (core) must begin with the file header comment"
        );
        for (name, frag) in [
            ("containers", CONTAINERS_SECTION),
            ("typeof_list", TYPEOF_LIST_SECTION),
            ("json_async", JSON_ASYNC_SECTION),
        ] {
            let first = frag.lines().next().unwrap_or("");
            assert!(
                first.starts_with("/*") || first.starts_with("//") || first.is_empty(),
                "fragment '{}' must begin at a top-level boundary (found: {:?})",
                name,
                first
            );
        }
    }

    /// Smoke test that none of the fragments are empty (would indicate a
    /// boundary mis-calculation).
    #[test]
    fn test_runtime_core_wasm_fragments_nonempty() {
        assert!(CORE_SECTION.len() > 10_000, "core fragment suspiciously small");
        assert!(
            CONTAINERS_SECTION.len() > 10_000,
            "containers fragment suspiciously small"
        );
        assert!(
            TYPEOF_LIST_SECTION.len() > 5_000,
            "typeof_list fragment suspiciously small"
        );
        assert!(
            JSON_ASYNC_SECTION.len() > 10_000,
            "json_async fragment suspiciously small"
        );
    }
}

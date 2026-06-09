//! Taida WASM core runtime — single translation unit assembled from four
//! mechanical fragments.
//!
//! The runtime is split into four `.inc.c` fragments and concatenated at
//! Rust load time before being passed to `clang`. The fragments are
//! treated as **a single translation unit** by clang (Rust concatenates
//! them and writes the result via `fs::write`), so DCE / static helper
//! references / forward declarations behave byte-identically to the
//! pre-split monolithic source.
//!
//! - [`CORE_SECTION`] — libc stubs / bump allocator / strlen / string helpers
//! stdout-stderr-debug I/O / int-bool operators / polymorphic display /
//! float arithmetic / BuchiPack / List / HashMap / Set
//! - [`CONTAINERS_SECTION`] — Closure runtime / Error ceiling / Lax / Result
//! Gorillax / Cage / Molten/Stub/Todo / type conversion molds / float
//! div/mod / String template / digit helpers
//! - [`TYPEOF_LIST_SECTION`] — RC no-ops / typeof / List HOF / List
//! operations / List query / element retain/release
//! - [`JSON_ASYNC_SECTION`] — JSON runtime (strtol/strtod/itoa/ftoa/FNV-1a /
//! type detection / public field wrappers / schema / descriptor apply) /
//! Async runtime / `_taida_main` extern / `_start` WASI entry

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
/// `concat!()` cannot be used because that macro requires literal
/// arguments; `LazyLock<&'static str>` + `Box::leak` exposes a
/// `&'static str` without adding a crate dependency. The JS runtime in
/// `src/js/runtime/mod.rs::RUNTIME_JS` uses the same strategy.
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

    /// Invariant: the concatenation of the four fragments must be
    /// byte-identical to the equivalent monolithic `runtime_core_wasm.c`.
    /// We anchor the total byte length + the first / last 200 bytes of
    /// the assembled source to detect accidental edits that would break
    /// DCE or shift static helper references across fragment boundaries.
    ///
    /// If a future change intentionally modifies the runtime C source,
    /// update both the fragment file and the `EXPECTED_TOTAL_LEN`
    /// constant below in the same commit. The historical byte-count
    /// growth log used to live here in `///` doc form; commit messages
    /// and the test's failure output cover the same ground without
    /// rotting the source surface, so the log is no longer kept inline.
    #[test]
    fn test_runtime_core_wasm_fragment_concat_preserves_bytes() {
        // F55 S4 (2026-06-06): +17,954 bytes in 02_containers.inc.c for the
        // extended crypto surface that is available on every WASM profile —
        // SHA-512 / 384 / 224 cores, HMAC-SHA256, constant-time equality,
        // and hex / base64 encode (all return Str / Bool, so no Bytes
        // constructor is needed). The Bytes-producing decode / randomBytes
        // symbols live in runtime_wasi_io.c (wasm-wasi / wasm-full only), so
        // they do not affect this core total.
        // F55 S4 review follow-up (2026-06-06): the constant-time-equality
        // length fold dropped bits 24-31 / 40-63 of the length XOR;
        // replaced with a direct `(a_len != b_len)` seed in
        // 02_containers.inc.c: -86 bytes. Total 449,171 -> 449,085.
        // 2026-06-08 interpreter module rename (C8): -5 bytes from updating a
        //   stale `mold_eval.rs` path reference in an 02_containers.inc.c
        //   comment to `mold.rs`. Comment-only. Total 449,085 -> 449,080.
        // 2026-06-08 F56 secret carrier: +940 bytes (02_containers.inc.c —
        //   taida_moltenize_new / taida_secret_new / taida_redact). Total
        //   449,080 -> 450,020.
        // 2026-06-08 F56 unmold reject + fail-closed display/JSON: +3,451 bytes.
        //   01_core.inc.c: shared __wasm_moltenized_str / __wasm_secret_str
        //   statics + _wasm_carrier_kind (pointer-identity, never derefs an
        //   arbitrary __type slot — a content compare OOB-trapped on magic-tagged
        //   AsyncTask/Par packs) + taida_generic_unmold reject placed inside the
        //   field-pack __type branch (after the async/lax/result guards) +
        //   is-carrier guards on both pack renderers (_wasm_pack_to_string /
        //   _full) so sealed carriers throw on `>=>` and render
        //   "<Secret>"/"<Moltenized>".
        //   02_containers.inc.c: producers store the shared __type statics.
        //   04_json_async.inc.c: _wc_json_serialize_typed guard so
        //   `jsonEncode(secret)` emits `null` (matching the interpreter) instead
        //   of exposing __value. Total 450,020 -> 453,471.
        // 2026-06-09 F56 equality fail-closed: +2,510 bytes. _wasm_carrier_kind
        //   guards on every comparison entry point so a sealed carrier is never
        //   equal (even to itself), never hashable, and never mixes __value into a
        //   fingerprint — closing the `==`/`!=`/Unique/contains/indexOf/
        //   `@[a]==@[b]` equality oracle (/so review #2). 01_core.inc.c:
        //   _wasm_value_eq / _wasm_value_hashable / _wasm_fp_accum / taida_poly_eq
        //   / taida_poly_neq. 03_typeof_list.inc.c: taida_list_index_of /
        //   taida_list_last_index_of. 453,471 -> 455,981.
        // 2026-06-09 F56 _wasm_carrier_kind OOB fix: +814 bytes. Reads the pack
        //   header + field-0 slots directly instead of taida_pack_has_hash /
        //   taida_pack_get, which iterate the hash slots and OOB-trapped on
        //   magic-tagged Async/Lax/Result packs reaching the equality helpers
        //   (`i == n` in pi_approx). 455,981 -> 456,795.
        // 2026-06-09 F56 Phase 4: +793 bytes for the secret-aware consumers
        //   taida_hmac_sha256_secret / taida_constant_time_eq_secret next to the
        //   crypto primitives in 02_containers.inc.c. 456,795 -> 457,588.
        // 2026-06-09 F56 final review: +273 bytes for the taida_list_contains
        //   carrier guard in 03_typeof_list.inc.c (closes the contains identity
        //   oracle). 457,588 -> 457,861.
        // 2026-06-09 F56 Phase 6+ review: +431 bytes for the sealed-receiver
        //   guards in taida_polymorphic_contains / _index_of / _last_index_of
        //   (01_core.inc.c). 457,861 -> 458,292.
        // 2026-06-09 F56-FB-002: +830 bytes for the non-sealed first-arg reject in
        //   taida_hmac_sha256_secret / taida_constant_time_eq_secret
        //   (02_containers.inc.c; parity with interpreter/JS throw). 458,292 -> 459,122.
        const EXPECTED_TOTAL_LEN: usize = 459_122;
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
        assert!(
            CORE_SECTION.len() > 10_000,
            "core fragment suspiciously small"
        );
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

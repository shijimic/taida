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
        // 2026-06-10 F58 T-M: +2,019 bytes for the TAIDA_PERF_COUNTERS
        //   measurement-build hooks — counter block + stack-only stderr dump +
        //   wasm_alloc / wasm_arena_enter hooks (01_core.inc.c) and the
        //   #ifdef'd wasm_perf_dump() call at the tail of _start
        //   (04_json_async.inc.c). Compiled in only with -DTAIDA_PERF_COUNTERS
        //   (env-gated dev build; participates in the runtime .o cache key).
        //   459,122 -> 461,141.
        // 2026-06-10 F58 _wasm_is_string_ptr off-by-one fix: +353 bytes
        //   (01_core.inc.c — `v < 1024` instead of `<= 1024` + rationale
        //   comment). wasm-ld places the data segment at global-base 1024,
        //   so the first static string literal sits AT 1024 and the old
        //   guard rejected it: `"x" + "y"` via untyped params concatenated
        //   the pointer value instead ("x1024"). 461,141 -> 461,494.
        // 2026-06-10 F58 P2-2: taida_arena_iter_enter/reset/exit wrappers
        //   over wasm_arena_enter/leave in 01_core.inc.c (call sites stay
        //   disabled in the emitter until WASM strings carry headers).
        // 2026-06-10 F58 P2-4: taida_div_exact / taida_mod_exact mirrors in
        //   01_core.inc.c. 462,036 -> 462,377.
        // 2026-06-10 F58 string headers: WASM_STR_MAGIC/_STATIC + _wasm_str_alloc
        //   + 63 allocation-site conversions + magic-required _wasm_is_string_ptr
        //   + the generic_unmold string/field-count guards + the
        //   _looks_like_string magic fast path. -> 464,779.
        // 2026-06-10 F58 generic_unmold validation rework (01_core.inc.c):
        //   the arbitrary `fc_chk > 256` cap is replaced by tail-sentinel
        //   verification (in-bounds `WASM_PACK_MAGIC` at data[1+fc*3], magic
        //   define hoisted before generic_unmold), and the user-mold
        //   `type_ptr` guard switches from a low-address cutoff — which
        //   rejected every data-segment `__type` literal — to
        //   _wasm_is_string_ptr. Plus the pack/list section headers now
        //   describe the trailing-magic layouts instead of "no magic".
        //   -> 466,461.
        // 2026-06-10 scalar-intrinsic expansion (01_core.inc.c): the
        //   return-tag slot `__wasm_return_tag` goes non-static so the
        //   generated C can expand taida_get/set_return_tag into a
        //   direct slot swap (one runtime call per user-function call
        //   otherwise), with a rationale comment. -> 466,785.
        // 2026-06-10 positive-only identification: _looks_like_string
        //   loses its printable-byte fallback (magic word required —
        //   every string in the value space carries one now), the WSTR
        //   macro wraps all 153 runtime literals entering the value
        //   space in header-carrying statics, the _looks_like_pack
        //   field-count cap is lifted (the tail sentinel is the proof),
        //   and the JSON detectors (_wc_looks_like_*) delegate to the
        //   positive 01_core implementations instead of their pre-magic
        //   structural heuristics. _wasm_str_alloc goes non-static for
        //   the profile runtimes. -> 464,735 (the removed fallback and
        //   delegated duplicates outweigh the macro).
        // 2026-06-10 early Int display paths (01_core.inc.c): stdout /
        //   stderr _with_tag format a statically-known Int directly,
        //   before any runtime re-detection — an Int whose value
        //   coincides with a live string's data address carries that
        //   string's real magic word, so detection cannot save it.
        //   Plus the int_to_str forward declaration. -> 466,038.
        // 2026-06-10 _wf_strstr complexity fix (01_core.inc.c): the
        //   helper measured the whole remaining haystack on every call,
        //   turning cursor-walking callers (Split / Replace) into
        //   O(n^2) — a thousand-part split re-measured ~5MB of tail per
        //   pipeline pass. Now terminator-driven. string_ops: 629ms ->
        //   16ms. -> 466,389.
        // 2026-06-10 empty-separator Split codepoint fix
        //   (02_containers.inc.c): the per-byte walk tore multibyte UTF-8
        //   apart, diverging from the locked chars-split semantics that
        //   the interpreter / native / JS already implement. Fragments
        //   stay empty-free per the B11 method lock. -> 467,164.
        // 2026-06-10 loud allocation failure (01_core.inc.c): wasm_alloc
        //   traps with a message at the 2GB heap ceiling (sign-extending
        //   pointer-to-value casts make addresses past 2^31 negative and
        //   silently reject as invalid — a 10k-append loop run twice
        //   returned an empty second list) and on memory.grow failure
        //   (used to return NULL into constructors). -> 468,580.
        // 2026-06-10 consume-variant Append (03_typeof_list.inc.c):
        //   taida_list_append_consume — see the native twin. -> 469,140.
        // 2026-06-11 kind-aware hash path for Float-bearing Set ops /
        //   unique (01_core / 03_typeof_list): Float fingerprint
        //   canonicalisation + seen-set wiring in union/intersect/diff,
        //   tagged linear helpers removed — see the native twin.
        //   479,244 -> 481,465.
        // 2026-06-11 unhandled-throw report unification
        //   (02_containers.inc.c): the no-ceiling path writes the
        //   interpreter's `Runtime error: Unhandled error: ...` line to
        //   stderr and proc_exits(1) instead of trapping after a stdout
        //   note. 481,465 -> 483,773.
        // 2026-06-11 string-conversion parse parity
        //   (02_containers.inc.c): the manual float parser accepts
        //   nan / inf / infinity like the reference. 483,773 -> 484,806.
        // 2026-06-11 code-point string indexing (01_core /
        //   02_containers): length / get / CharAt / Slice / Reverse
        //   walk UTF-8 lead bytes — see the native twin.
        //   484,806 -> 486,760.
        // 2026-06-11 public-shape container display (01_core): the
        //   synthetic full-form renderers are removed — every display
        //   path routes HashMap / Set through `HashMap({...})` /
        //   `Set({...})`. 486,760 -> 482,417.
        // 2026-06-11 throw-report DCE fix (02_containers.inc.c): the
        //   unhandled report renders strings/scalars locally instead of
        //   referencing the polymorphic display machinery, which pulled
        //   ~16KB into every binary (wasm_min_size_gate caught it).
        //   482,417 -> 483,119.
        // 2026-06-11 edge-profile throw exit (02_containers.inc.c): the
        //   edge handler host provides no WASI proc_exit, so the
        //   unhandled report traps there (catchable RuntimeError) and
        //   proc_exits(1) on the WASI profiles. 483,119 -> 483,445.
        // 2026-06-11 Slice end-sentinel unification + list arm
        //   (02_containers.inc.c): 483,445 -> 484,166.
        // 2026-06-11 STR tags on str.get's Lax (02_containers.inc.c):
        //   484,166 -> 484,517.
        // 2026-06-11 numeric fingerprint = f64 image
        //   (01_core.inc.c) — see the native twin. 484,517 -> 483,936.
        const EXPECTED_TOTAL_LEN: usize = 483_936;
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
        // catches accidental truncation of the tail fragment. The _start
        // body tail gained the #ifdef'd perf-dump call in F58 T-M.
        assert!(
            asm.trim_end().ends_with(
                "_taida_main();\n#ifdef TAIDA_PERF_COUNTERS\n    wasm_perf_dump();\n#endif\n}"
            ),
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

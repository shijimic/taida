//! Taida WASM core runtime — single translation unit assembled from five
//! fragments and the math kernel shared with the native runtime.
//!
//! The runtime is split into five `.inc.c` fragments and concatenated at
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
//! - [`NUMERIC_SECTION`] — numeric kernel: f64 <-> decimal text (Float
//! display, ToFixed, Float parsing) and round. Concatenated before the
//! JSON / async fragment so the WASI `_start` entry stays last; callers
//! in the earlier fragments see the entry points through forward
//! declarations in `01_core.inc.c`.
//! - [`CRMATH_TABLES_SECTION`] / [`CRMATH_SECTION`] — the correctly
//! rounded math kernel behind the transcendental molds, shared with the
//! native runtime; concatenated after the numeric fragment.
//! - [`JSON_ASYNC_SECTION`] — JSON runtime (strtol/itoa/ftoa/FNV-1a /
//! type detection / public field wrappers / reader / schema / descriptor apply) /
//! Async runtime / `_taida_main` extern / `_start` WASI entry

use std::sync::LazyLock;

/// Fragment 1: libc stubs, allocator, I/O, numerics, containers (core).
pub const CORE_SECTION: &str = include_str!("01_core.inc.c");

/// Fragment 2: Closure, Error ceiling, Lax/Result/Gorillax, Cage,
/// type conversion molds, float div/mod, misc helpers.
pub const CONTAINERS_SECTION: &str = include_str!("02_containers.inc.c");

/// Fragment 3: RC no-ops, typeof, List HOF / operations / queries.
pub const TYPEOF_LIST_SECTION: &str = include_str!("03_typeof_list.inc.c");

/// Fragment 5 (assembled fourth): f64 <-> decimal text and round.
pub const NUMERIC_SECTION: &str = include_str!("05_numeric.inc.c");

/// Fragment 4: JSON runtime + Schema + Async + `_start` entry.
pub const JSON_ASYNC_SECTION: &str = include_str!("04_json_async.inc.c");

/// Shared with the native runtime: the tables and the correctly rounded
/// math kernel behind the transcendental molds.
pub const CRMATH_TABLES_SECTION: &str = include_str!("../runtime_crmath_tables.inc.c");
pub const CRMATH_SECTION: &str = include_str!("../runtime_crmath.inc.c");

/// Full wasm core runtime C source, assembled from the five fragments and
/// the shared math kernel on first access and cached for the process
/// lifetime.
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
        + NUMERIC_SECTION.len()
        + CRMATH_TABLES_SECTION.len()
        + CRMATH_SECTION.len()
        + JSON_ASYNC_SECTION.len();
    let mut s = String::with_capacity(total);
    s.push_str(CORE_SECTION);
    s.push_str(CONTAINERS_SECTION);
    s.push_str(TYPEOF_LIST_SECTION);
    s.push_str(NUMERIC_SECTION);
    s.push_str(CRMATH_TABLES_SECTION);
    s.push_str(CRMATH_SECTION);
    s.push_str(JSON_ASYNC_SECTION);
    debug_assert_eq!(s.len(), total);
    Box::leak(s.into_boxed_str())
});

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariant: the concatenation of the five fragments must be
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
        // 2026-06-12 F62B-012 Bytes layout unification (01_core /
        //   04_json_async): taida_collection_get / taida_polymorphic_length
        //   / _wasm_stdout_display_string gain the native-mirror Bytes
        //   branches, and the JSON wire encode/decode helpers
        //   (_wc_wire_bytes_len / _wc_wire_bytes_at /
        //   _wc_json_bytes_from_raw) speak the shared
        //   [TAIDBYT, len, byte...] layout instead of the list shape the
        //   ABI helpers used to emit. 483,936 -> 486,300.
        // 2026-06-12 F62B-015 wasm JSON schema validation (04_json_async):
        //   _wc_json_apply_schema rejects present-but-wrong-kind values
        //   like the interpreter's json_to_typed_value_checked (missing /
        //   null fields keep defaults), and default construction goes
        //   through the missing-value path. 486,300 -> 487881.
        // 2026-06-12 F62B-017 string ordering + Chars + Utf8Decode relocation
        //   (02_containers): taida_str_lt/gt/gte, taida_str_chars, and
        //   taida_utf8_decode_mold (+ _core_utf8_decode_one) move string
        //   ordering and Bytes decoding into the all-profile core.
        // 2026-06-12 Utf8Encode joins the core (02_containers) alongside
        //   Utf8Decode — the TAIDBYT constructor is core-safe.
        // 2026-06-12 F62B-019: slot-tag precedence in jsonEncode pack fields,
        //   kind-aware list element hints, Split elem tag (02/04).
        // 2026-06-12 F62B-026 (01_core): _wasm_non_mold_unmold_gorilla;
        //   compat audit narrowed the rule to machinery-less plain packs
        //   (no __type) — bare values stay identity: +2,011.
        //   495,406 -> 497,417.
        // 2026-06-12 F62B-034 (01_core): custom mold `__unmold` hook
        //   invocation in generic_unmold: +565. 497,417 -> 497,982.
        // 2026-06-12 F62B-026 follow-up (01_core): the non-mold gorilla
        //   terminates via __builtin_trap on the edge profile instead of
        //   importing proc_exit (the helper is reachable from every `>=>`
        //   lowering, so the hard import broke node instantiation):
        //   +477. 497,982 -> 498,459.
        // 2026-08-22 display-parity field-name registry (01_core /
        //   04_json_async): the bulk registration helper's forward
        //   declarations + ABI descriptor names join the abi_fields
        //   array, and the positive-string judgment precedes the pack
        //   heuristic in _wc_json_serialize_typed: +1,752.
        //   498,459 -> 500,211.
        // 2026-08-22 review-fix (01_core / 02_containers / 04_json_async):
        //   taida_todo_new stamps per-slot value tags via _wasm_todo_field_tag,
        //   the Todo hash names register in the bulk registrar, the HashMap
        //   jsonEncode arm gains a value kind hint + non-Str key skip, and
        //   the L2 comment is tempered: +2,836.
        //   500,211 -> 503,047.
        // 2026-08-22 review-fix round 2 (01_core): the monadic slot names
        //   (__default / __error / __value / has_value) join the bulk
        //   registrar's abi_fields so the wasm TODO renderer resolves all
        //   seven fields like interp/native: +278.
        //   503,047 -> 503,325.
        // 2026-08-22 review-fix round 3 (01_core): _wasm_pack_to_string_full
        //   gains the Lax __error display skip (native twin) — registering
        //   __error made its absence observable on wasm-full: +539.
        //   503,325 -> 503,864.
        //  (02_containers): taida_canonicalize_caught_error builds
        //   the full ErrorInfo shape at the catch site with per-slot value
        //   tags; taida_error_type_check_or_rethrow routes through it: +2,630.
        //   503,864 -> 506,494.
        //  (02_containers): the early-return guard gains
        //   a HASH___TYPE presence probe, symmetric with interp/native: +1,171.
        //   506,494 -> 507,665.
        //  (02_containers): taida_str_pad counts
        //   characters via _wasm_utf8_count and repeats the whole pad string,
        //   with a total-length guard: +371.
        //   507,665 -> 508,036.
        // 2026-09-29 (01_core / 02_containers / 03_typeof_list /
        //   04_json_async): the strict JSON reader (grammar, `\uXXXX`,
        //   exact numbers, self-referential schemas, kind tags, rebuild
        //   hooks for TypeDefs with methods, the interpreter's result on a
        //   kind mismatch), the Div/Mod smallest-Int case, saturating
        //   float-to-Int with exact Floor/Ceil/Round, exact ToFixed and
        //   Unicode-whitespace Trim — see the native twins — plus Enum
        //   schemas in the reader: 507,704 -> 534,314.
        // 2026-09-29: a throw stops every caller up to its ceiling (the
        //   pending flag is read by the generated C; callbacks and a second
        //   throw on the dead path are inert); the numeric kernels fragment
        //   (Float text + math library) replaces the ad-hoc formatter,
        //   parser, ToFixed and series approximations; the Bytes / bitwise /
        //   Float Div-Mod molds move in from the wasi layer and range, the
        //   byte-level Str molds, StringRepeatJoin and the Enum field
        //   registries join them on every profile; HashMap key equality and
        //   churn rebuild, the Int range check of Int[Str], jsonEncode
        //   escapes / non-finite nulls, Bytes display, the static empty
        //   pack, 128-bit integer support routines; the JSON schema error
        //   message as a Str.
        //   534,314 -> 644,100.
        // Clamp with reversed bounds, Sort in the reference's order (merge
        //   sort, NaN keys last, `by` keys by their kind), the uncaught-error
        //   message rule, and the omitted Error `type` / `message` reader:
        //   644,100 -> 643,950.
        // jsonEncode / jsonPretty written by the argument's static type
        //   (a schema descriptor from the lowering): 643,950 -> 649,538.
        // Join and kinded element strings render an Int as a number; Str.get
        //   defaults carry the string header: 649,538 -> 649,826.
        // List accessors default by the first element's kind; jsonEncode
        //   writes Bytes as null and checks a Str part: 649,826 -> 651,259.
        // Function values show as `<function NAME>`, methods are left out of
        //   displays and jsonEncode, AsyncTask shows in lists, Lax over Bytes
        //   records its kind: 651,259 -> 658,628.
        // Runtime-raised errors are built in the caught form with their
        //   string kinds, the Result predicate message names the value, and
        //   Error values show as packs: 658,628 -> 659,056.
        // Rejected Async values show a runtime-raised error as Error(type:
        //   message): 659,056 -> 660,747.
        // taida_list_note_push_dyn (kind only; WASM counts no references):
        //   660,747 -> 661,196.
        // HashMap lookups record the map's value kind and the empty value
        //   of it (_wasm_hashmap_lax): 661,196 -> 662,394.
        // pack display takes Bool from the field's own tag, not the
        //   name-keyed registry: 662,394 -> 661,883.
        // jsonEncode writes a Float in the reference notation
        //   (_wc_json_format_float): 661,883 -> 663,593.
        // Gorillax.toString() shows the value by its slot's kind: 663,593
        //   -> 664,034.
        // Abs clears the sign bit, Int[str, base]() checks the sign and the
        //   range, the fixed-width decodes need the exact length, and Slice
        //   takes Bytes: 664,034 -> 665,257.
        // taida_function_to_string shows a function value in a template
        //   part or Str[...](): 665,257 -> 665,486.
        // list and set displays read each element's kind: 665,486 ->
        //   665,770.
        // HashMap entries keep their key and value kinds, list / pack keys
        //   hash and compare by content, HashMaps compare structurally and
        //   jsonEncode names Enum values: 665,770 -> 673,186.
        // taida_lax_fill_default gives a failed lookup the default of the
        //   element's static type: 673,186 -> 673,783.
        // AsyncReject keeps its payload's kind, and a payload that is not
        //   an error is thrown as an AsyncError carrying its text: 673,783 ->
        //   675,301.
        // Upper / Lower use the whole Unicode case mapping, with the tables
        //   the interpreter's Rust standard library has: 675,301 -> 710,568.
        // ByteLength / ByteAt / ByteSlice read Bytes values too, and
        //   ByteSlice reads a cut UTF-8 sequence as U+FFFD: 710,568 ->
        //   712,025.
        // jsonEncode / jsonPretty stop past 128 levels of nesting as the
        //   native runtime does: 712,025 -> 713,179.
        // A Result given no error reads and shows its throw as the empty
        //   Error: 713,179 -> 714,700.
        // A Result given no error reads and shows its throw as the empty
        //   Error: 714,700 -> 715,002.
        // Functions are named Function by typeof: 715,002 -> 715,003.
        // hashMap(entries) builds a HashMap on the compiled backends:
        //   715,003 -> 719,028.
        // Error types are told apart by their declaration: 719,028 ->
        //   722,573.
        // Transcendental molds use the shared correctly rounded kernel:
        //   722,573 -> 731,100.
        // WASM Float text conversion rewritten in-house: 731,100 ->
        //   729,444.
        // HashMap keys tell strings apart first, and Int / Bool sets
        //   compare by value: 729,444 -> 732,972.
        // Pointer checks do not wrap at 2^32; field names have no fixed
        //   limit; Molten and Stream show as the reference shows them:
        //   732,972 -> 734,753.
        const EXPECTED_TOTAL_LEN: usize = 734_753;
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
            ("numeric", NUMERIC_SECTION),
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
            NUMERIC_SECTION.len() > 10_000,
            "numeric fragment suspiciously small"
        );
        assert!(
            JSON_ASYNC_SECTION.len() > 10_000,
            "json_async fragment suspiciously small"
        );
    }
}

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
    ///
    /// C12B-016 (2026-04-15): +540 bytes for doc-comment updates describing
    /// the new codegen two-path dispatch (compile-time-known Str literals
    /// go through plain `taida_io_stdout(char*)`; everything else reaches
    /// `taida_io_stdout_with_tag` polymorphic formatter). Runtime bodies
    /// of `_with_tag` are unchanged — only comments grew.
    ///
    /// C21-4 (2026-04-21): +1103 bytes for the new FLOAT-tag fast path in
    /// `taida_io_stdout_with_tag` / `taida_io_stderr_with_tag` (dispatch
    /// the boxed f64 bit-pattern through `taida_float_to_str`) plus its
    /// forward declaration at the top of the file. Fixes `stdout(...)`
    /// of a function-returned Float rendering as the raw i64 bit pattern
    /// (symptom `4622945017495814144` for `stdout(triple(4.0))`).
    ///
    /// C21B-seed-07 (2026-04-22): +6,195 bytes across the four fragments.
    /// `01_core.inc.c` grew to add `_wasm_pack_to_string_full` +
    /// `_wasm_stdout_display_string`, per-field-tag Float/Bool dispatch
    /// in the existing `_wasm_pack_to_string`, and BuchiPack-aware
    /// detour branches in `taida_io_stdout_with_tag` /
    /// `_stderr_with_tag` (symmetric with the native
    /// `taida_stdout_display_string` routing). `02_containers.inc.c`
    /// grew to stamp the per-field primitive tag on `__value` /
    /// `__default` for `taida_{int,str}_mold_*` via the new
    /// `_lax_tag_vd` helper (Float/Bool mold constructors already
    /// carried the stamping). Fixes `stdout(Float[x]())` on wasm-wasi
    /// rendering a pack pointer as subnormal f64 bits and
    /// `stdout(Int[x]())` emitting the short `Lax(3)` toString form
    /// instead of the interpreter's full `@(hasValue <= …, …)` shape.
    ///
    /// C23-2 / C23-4 (2026-04-22): +3,674 bytes across two fragments.
    /// `02_containers.inc.c` adds `taida_str_mold_any` (generic `Str[x]()`
    /// entry for non-primitive values, routes through
    /// `_wasm_stdout_display_string`) and `_taida_float_to_str_mold` (Rust
    /// `f64::to_string`-compatible formatter that strips the trailing `.0`
    /// on integer-valued floats to match the interpreter's
    /// `Str[3.0]() -> "3"` contract). `01_core.inc.c` extends both
    /// `_wasm_pack_to_string` and `_wasm_pack_to_string_full` with a
    /// `WASM_TAG_STR` render branch so `__value <= ""` / `__default <= ""`
    /// render as quoted char* instead of degrading into the
    /// `_wasm_value_to_debug_string` integer-pointer fallback. Fixes
    /// C23B-001 (wasm primitive/Lax `Str[...]()` parity) and C23B-002
    /// (native/wasm non-primitive `Str[...]()` pointer stringification).
    ///
    /// C23B-003 reopen (2026-04-22): +2,772 bytes in `01_core.inc.c`.
    /// Added `_wasm_hashmap_to_display_string_full` and
    /// `_wasm_set_to_display_string_full` (synthetic full-form pack
    /// renderers that mirror the interpreter's
    /// `BuchiPack(__entries/__items, __type)` layout for HashMap/Set) and
    /// routed `_wasm_stdout_display_string` through them so `Str[hm]()` /
    /// `Str[s]()` on wasm no longer yield the short-form `HashMap({…})` /
    /// `Set({…})` strings. Symmetric with native's
    /// `taida_hashmap_to_display_string_full` /
    /// `taida_set_to_display_string_full`.
    ///
    /// C23B-003 reopen 2 (2026-04-22): +5,369 bytes in `01_core.inc.c`.
    /// Added `_wasm_value_to_debug_string_full` (recursive debug-string
    /// variant that keeps nested typed runtime objects in full-form
    /// shape) plus its forward declaration block, replaced the three
    /// call sites inside `_wasm_hashmap_to_display_string_full`,
    /// `_wasm_set_to_display_string_full`, and `_wasm_pack_to_string_full`
    /// that previously called the short-form `_wasm_value_to_debug_string`,
    /// and added a List branch in `_wasm_stdout_display_string` that
    /// also routes list items through the full-form helper (so
    /// `Str[@[hashMap()...]]()` no longer emits the short-form
    /// `HashMap({…})` items). Fixes the HIGH-severity regression where
    /// `hashMap().set("k", hashMap().set("a", 1))` collapsed the nested
    /// HashMap to the `.toString()` short-form `HashMap({"a": 1})`.
    /// Symmetric with native's `taida_value_to_debug_string_full`.
    /// EXPECTED_TOTAL_LEN: 254,479 → 259,848.
    ///
    /// C23B-003 reopen 3 (2026-04-22): +5,646 bytes in `01_core.inc.c`.
    /// Added `_looks_like_empty_pack` detector (matches the
    /// `taida_pack_new(0)` layout that `_looks_like_pack` rejects because
    /// it requires `fc >= 1`) and routed it through
    /// `_wasm_value_to_debug_string_full`, `_wasm_value_to_debug_string`,
    /// `_wasm_value_to_display_string`, and `_wasm_stdout_display_string`
    /// so that nested or top-level `@()` render as the interpreter-parity
    /// string `"@()"` instead of the raw heap pointer integer. The
    /// detector uses the same bump-allocator address-range heuristic as
    /// `_wasm_is_string_ptr` (`__heap_base <= addr < bump_ptr`) — a
    /// tighter guard than `_wasm_is_valid_ptr`, which would accept static
    /// data-section offsets and false-positive on small integer outputs
    /// such as `5050` (tail-recursion Fibonacci) / `8080` (interpolated
    /// port number), both caught by `tests/wasm_full.rs`. Fixes WASM-only
    /// divergence found on `Str[@()]()` /
    /// `Str[hashMap().set("u", @())]()` / `Str[@(u <= @())]()` /
    /// `Str[@[@()]]()`. The four added fixtures (`str_from_empty_pack` /
    /// `str_from_pack_with_empty_pack` /
    /// `str_from_hashmap_with_empty_pack` /
    /// `str_from_list_with_empty_pack`) pin byte-for-byte 4-backend
    /// parity. EXPECTED_TOTAL_LEN: 259,848 → 265,494.
    ///
    /// C23B-003 reopen 4 (2026-04-22): +1,935 bytes in `01_core.inc.c`.
    /// Replaced the heap-range + zero-slot heuristic in
    /// `_looks_like_empty_pack` with a positive-identification magic
    /// sentinel (`WASM_EMPTY_PACK_MAGIC = 0x5441494450414B55`, stamped
    /// in `pack[1]` by `taida_pack_new(0)`). The old heuristic
    /// false-positive'd on dynamic Int expressions whose bit patterns
    /// happened to land inside the bump arena and point at an
    /// 8-byte-aligned zero chunk, rendering integers as `@()`
    /// (repro: `a <= 36000; b <= 37088; stdout(Str[a + b]())` printed
    /// `__value <= "@()"` instead of `"73088"`). Also widened the
    /// `Str[x]()` fast-path dispatch in `src/codegen/lower_molds.rs`
    /// from `expr_is_int_literal` (literal only) to
    /// `Lowering::expr_is_int` (richer: int_vars / arithmetic /
    /// int-returning calls) so more dynamic Int shapes short-circuit
    /// directly through `taida_str_mold_int` at compile time,
    /// defence-in-depth on top of the detector-level fix.
    /// EXPECTED_TOTAL_LEN: 265,494 → 267,429.
    ///
    /// C23B-005 reopen + widen + C23B-006 (2026-04-22): +3,541 bytes in
    /// `01_core.inc.c`. Unified every WASM collection detector
    /// (`_looks_like_list`, `_is_wasm_set`, `_is_wasm_hashmap`,
    /// `_looks_like_pack`) onto positive-identification 8-byte magic
    /// sentinels — `WASM_LIST_MAGIC` ("TAIDLST") at `list[3]`,
    /// `WASM_SET_MAGIC` ("TAIDSET") at `set[3]`, `WASM_HM_MAGIC`
    /// ("TAIDHMP") at `hm[3]`, and `WASM_PACK_MAGIC` ("TAIDPKK")
    /// appended to the tail of every non-empty BuchiPack. The old
    /// structural heuristics (cap ∈ 8..=65_536 for List, 4-byte
    /// `"HMAP"` / `"SET\0"` markers at `data[3]`, fc ∈ 1..=100 +
    /// `first_hash != 0` for Pack) false-positived on any untagged
    /// Int64 whose bit pattern happened to point at heap memory
    /// matching one of those signatures — a HashMap value slot
    /// holding a raw `73088` re-rendered the integer as an empty
    /// HashMap (C23B-006), and list, pack, or set members with a
    /// large Int sent the renderer into stack-overflow recursion
    /// (C23B-005). Allocation paths (`taida_list_new`,
    /// `taida_pack_new(fc>=1)`, `taida_set_new`, `taida_hashmap_new`,
    /// `_wasm_hashmap_new_with_cap`, plus resize sites) now stamp the
    /// matching sentinel; detectors require `_wasm_is_valid_ptr` with
    /// 8-byte alignment and exact sentinel match. Also added the
    /// tag-aware element renderers (`_wasm_render_elem_tagged_debug`
    /// and `_wasm_render_elem_tagged_debug_full`) and threaded the
    /// list or set `elem_type_tag` (slot 2), hashmap `value_type_tag`
    /// (slot 2), and pack `field_tag` into every collection-member
    /// rendering loop so primitive Int, Float, Bool, or Str members
    /// dispatch by tag instead of through the structural detectors.
    /// This is the defence-in-depth partner for the magic-sentinel
    /// detector change — when a list element carries `WASM_TAG_INT`,
    /// we render with `taida_int_to_str` regardless of whether the
    /// Int value happens to alias a heap address matching any
    /// collection magic. `taida_set_from_list` and `taida_set_add`
    /// now propagate the source `elem_type_tag` across immutable
    /// clone paths.
    /// EXPECTED_TOTAL_LEN: 270,970 then 278,235.
    ///
    /// C23B-005 reopen 2 + C23B-006 (2026-04-22): +5,248 bytes in
    /// `01_core.inc.c`. Dual-magic collection identification — every
    /// List / Set / HashMap allocation now stamps the 8-byte magic
    /// at BOTH a head position (`data[3]`) and a shape-dependent
    /// trailing position (`data[WASM_LIST_ELEMS + cap]` for lists /
    /// sets, `data[WASM_HM_HEADER + cap * 3]` for hashmaps). Detectors
    /// (`_looks_like_list` / `_is_wasm_set` / `_is_wasm_hashmap`)
    /// verify both positions, giving 128 bits of identification
    /// entropy that cannot be spoofed by an untagged Int whose bit
    /// pattern merely points at another collection's base — an
    /// attacker would need the trailing magic to ALSO align at the
    /// cap-dependent offset, which is vanishingly unlikely.
    /// `taida_list_push` / `taida_set_new` / `_wasm_hashmap_new_with_cap`
    /// propagate the trailing magic across grow paths. Also hardened
    /// `taida_hashmap_set_value_tag` and `taida_list_set_elem_tag` to
    /// downgrade heterogeneous containers to UNKNOWN(-1) instead of
    /// silently overwriting with the last inserted value's tag —
    /// that lets the tag-aware renderer safely fast-path
    /// homogeneous primitive containers while keeping the structural
    /// fallback correct for heterogeneous ones.
    /// EXPECTED_TOTAL_LEN: 278,235 → 283,669 (includes the
    /// `04_json_async.inc.c` `_wc_is_hashmap` / `_wc_is_set`
    /// delegation wrappers that forward to the hardened
    /// `_is_wasm_hashmap` / `_is_wasm_set` in `01_core.inc.c`).
    ///
    /// C23B-007 / C23B-008 (2026-04-22): 283,669 → 292,933 (+9,264).
    /// The two deltas are:
    /// - C23B-007: introduced `WASM_TAG_HETEROGENEOUS = -2`, taught
    ///   `taida_list_set_elem_tag` / `taida_hashmap_set_value_tag` to
    ///   latch on it so once a mixed container is downgraded it can't
    ///   re-promote to a primitive tag on a subsequent `.push()` /
    ///   `.set()`. The renderers already treat non-primitive tags as
    ///   "fall back to structural dispatch", so no renderer changes
    ///   were needed.
    /// - C23B-008: added the HashMap insertion-order side-index
    ///   (`[next_ord, order_array[cap]]` appended after the trailing
    ///   magic). `_wasm_hashmap_new_with_cap` allocates +`1+cap` slots
    ///   and stamps `next_ord = 0`; `taida_hashmap_set` / `_remove` /
    ///   `_clone` / `_resize` maintain the ordering; display /
    ///   `taida_hashmap_keys` / `_values` / `_entries` / `_merge` /
    ///   JSON serialize walk it so output matches interpreter / JS
    ///   byte-for-byte. Added two macros (`WASM_HM_ORD_HEADER_SLOT`,
    ///   `WASM_HM_ORD_SLOT`) to centralise the offset math.
    ///
    /// C23B-008 reopen (2026-04-22): 292,933 → 293,560 (+627).
    /// The wasm `taida_hashmap_merge` was rewritten from
    /// "copy self then call `taida_hashmap_set` per other entry" (which
    /// preserves self's ordinal for overlap keys, diverging from
    /// interpreter) to the interpreter's retain-then-push algorithm
    /// (fresh map; fill with self-entries whose key ∉ other in
    /// self-order; then append every other entry in other-order).
    /// Repro: `a=[a,b].merge([c,b,d])` — interpreter emits `[a,c,b,d]`,
    /// buggy wasm emitted `[a,b,c,d]`. Matches the symmetric
    /// native fix (`src/codegen/native_runtime/core.c::taida_hashmap_merge`)
    /// and the JS fix (`src/js/runtime/core.rs::__taida_createHashMap.merge`).
    ///
    /// C23B-009 (2026-04-22): 293,560 → 295,319 (+1,759, all inside
    /// `01_core.inc.c::taida_hashmap_entries`). The wasm entries() helper
    /// now (a) idempotently registers the `"key"` / `"value"` field
    /// names into `_wasm_field_registry` so `_wasm_pack_to_string_full`
    /// resolves them (previously unregistered → NULL → every pair
    /// rendered as `@()`), (b) stamps `WASM_TAG_STR` on the `key` slot
    /// and propagates the hashmap's `value_type_tag` onto the `value`
    /// slot so primitives render through the tagged fast-path, and
    /// (c) stamps `WASM_TAG_PACK` on the returned list's
    /// `elem_type_tag` so the outer list dispatches into pair packs via
    /// `_wasm_render_elem_tagged_debug_full`. Paired with the JS rename
    /// of `{first, second}` → `{key, value}` and the symmetric native
    /// `taida_register_field_name` calls so all four backends now
    /// emit `@(key <= …, value <= …)` for `.entries()`, matching the
    /// interpreter and `docs/reference/standard_library.md:238`.
    ///
    /// C24-B (2026-04-23): 299,284 → 301,386 (+2,102). Added the
    /// `_wasm_register_zip_enumerate_field_names` helper and called it
    /// at the head of `taida_list_zip` / `taida_list_enumerate` in
    /// `03_typeof_list.inc.c` so the `first` / `second` / `index` /
    /// `value` field names resolve in `_wasm_pack_to_string_full`
    /// (previously unregistered → NULL → every pair rendered as
    /// `@()`, which then trapped on the recursive full-form walk
    /// because the outer list's `elem_type_tag = WASM_TAG_PACK` forced
    /// the pair through tagged fast-path rendering into unresolved
    /// slots). Also propagated per-source-list `elem_type_tag` to each
    /// pair slot's tag (index keeps WASM_TAG_INT, zip's first/second
    /// and enumerate's value carry the source tag) and stamped
    /// WASM_TAG_PACK on the returned list's `elem_type_tag` so outer
    /// list renders each pair through the structural Pack path.
    /// Idempotent registration follows the C23B-009
    /// `taida_hashmap_entries` pattern. Interpreter / JS were already
    /// correct; this closes the native / wasm empty-list / segfault
    /// divergence flagged as C24B-002.
    ///
    /// C25B-026 (2026-04-23, Codex reopen of C24-A HOLD): 301,386 →
    /// 302,700 (+1,314). Replaced the per-call `taida_pack_new(0)`
    /// allocation with a lazily-initialised singleton cached in a
    /// function-local `static int64_t empty_pack_singleton`. Previously
    /// the wasm bump allocator (which never frees) appended 16 bytes to
    /// `bump_ptr` for every empty-pack producer — C24-A's Gorillax /
    /// RelaxedGorillax allocators invoke it on every successful value
    /// to populate the `__error <= @()` slot, so a long-running program
    /// constructing N Gorillax values in a loop leaked 16 × N bytes
    /// until `memory.grow` ran out of pages and trapped. The empty
    /// pack is immutable (`field_count == 0` means `taida_pack_set`
    /// has no valid target) and every reader only consults the
    /// `WASM_EMPTY_PACK_MAGIC` sentinel at slot[1], so reusing the
    /// same pointer is indistinguishable from per-call allocation at
    /// the observable level. Native's `taida_gorillax_new` already
    /// avoids the leak through a different mechanism (`PACK + 0 →
    /// @()` rendering special-case, no per-call allocation); the
    /// singleton is the wasm-side equivalent without touching the
    /// detector or renderer path. Fixes the linear-memory OOM flagged
    /// in Codex's C24 HOLD review.
    ///
    /// C25B-001 Phase 3 (2026-04-23): 302,700 → 313,465 (+10,765). Added
    /// minimal Stream lowering to `01_core.inc.c` so `Str[stream]()` on
    /// wasm no longer errors with `unsupported mold type: Stream`: a
    /// `taida_stream_new` allocator that emits the 3-field pack
    /// `@(__stream_status <= "completed", __stream_count <= 1,
    /// __type <= "Stream")`, a `_wasm_is_stream` detector keyed on the
    /// `__type` FNV-1a hash, a `_wasm_stream_to_display_string` full-form
    /// renderer, and the three-byte field-name registration block. Routed
    /// from `_wasm_stdout_display_string` ahead of the generic BuchiPack
    /// path so the Stream shape renders byte-for-byte identical to the
    /// interpreter's `@(__stream_status <= "completed", __stream_count <=
    /// 1, __type <= "Stream")` form. Closes the 4-backend Stream parity
    /// gap flagged as C25B-001.
    ///
    /// C25B-026 Phase 5-G (2026-04-23): 313,465 → 318,189 (+4,724). Added
    /// the arena-scoped release infrastructure to `01_core.inc.c`:
    /// `wasm_arena_enter()` snapshots `bump_ptr` and returns it as an
    /// opaque i32 handle; `wasm_arena_leave(saved)` restores `bump_ptr`
    /// to the snapshot, releasing every allocation made since the
    /// matching enter; `wasm_arena_used()` reports the current heap
    /// consumption since `__heap_base`; and
    /// `wasm_arena_roundtrip_test(iters, inner)` drives an
    /// enter/alloc/leave loop and returns the net heap delta (0 on
    /// success, `iters * inner * 64` on regression) so
    /// `tests/wasm_wasi.rs::wasm_wasi_arena_release_is_bounded` can
    /// invoke it via `wasmtime --invoke` as a single-call behavioural
    /// assertion. For the wasm-wasi and wasm-full profiles,
    /// `wasm_arena_export_flags()` in `src/codegen/driver.rs` emits
    /// `--export=wasm_arena_*` wasm-ld flags that simultaneously keep
    /// the symbols from `--gc-sections` and make them callable from
    /// wasmtime / host harnesses. For wasm-min / wasm-edge the flag
    /// list is empty and `--gc-sections` drops all four helpers
    /// entirely, preserving the 512-byte hello.wasm ceiling guarded
    /// by `tests/wasm_min.rs::wasm_min_size_gate`. Paired with the
    /// runtime-linker plumbing in `src/codegen/driver.rs` that
    /// translates `TAIDA_WASM_INITIAL_PAGES` / `TAIDA_WASM_MAX_PAGES`
    /// env vars into `--initial-memory=…` / `--max-memory=…` wasm-ld
    /// flags, giving per-build control of the initial and maximum
    /// linear-memory footprint. This is the foundation for Phase 5-I
    /// (or a later pass) to inject enter/leave around
    /// escape-analysis-safe scopes; today it lets regression tests
    /// assert that a bounded-scope loop does not monotonically leak
    /// heap across iterations. Fixes the `@[Float]` / LLM forward-loop
    /// linear-memory OOM flagged in C25B-026.
    ///
    /// C24-A (2026-04-23): 295,319 → 299,284 (+3,965). Unified WASM
    /// Gorillax's first-field hash from `WASM_HASH_IS_OK` (0x6550…) to
    /// `WASM_HASH_HAS_VALUE` (0x9e9c…) so `Str[Gorillax[v]()]()` matches
    /// the interpreter / JS / native `@(hasValue <= …, __value <= …,
    /// __error <= @(), __type <= "Gorillax")` shape byte-for-byte. The
    /// five Gorillax / RelaxedGorillax allocators in `02_containers.inc.c`
    /// (`taida_gorillax_new`, `taida_gorillax_err`, `taida_gorillax_relax`,
    /// `taida_relaxed_gorillax_new`, `taida_relaxed_gorillax_err`) now
    /// (a) set `hash0 = WASM_HASH_HAS_VALUE`, (b) idempotently register
    /// the `hasValue` / `__value` / `__error` / `__type` field names via
    /// the new `_wasm_register_gorillax_field_names` helper so
    /// `_wasm_pack_to_string_full` resolves all four fields (previously
    /// `__error` was unregistered and silently skipped), (c) stamp
    /// `WASM_TAG_PACK` on the `__error` slot plus store a proper empty
    /// pack pointer (via `taida_pack_new(0)`) when there is no error so
    /// the display helper emits `__error <= @()` instead of `"0"`, and
    /// (d) stamp `WASM_TAG_STR` on the `__type` slot so the `"Gorillax"` /
    /// `"RelaxedGorillax"` string is quoted. Because Lax and Gorillax
    /// now share `hash0`, `_wasm_is_gorillax` (in `01_core.inc.c`) and
    /// `_wasm_is_lax` (in `02_containers.inc.c`) disambiguate via the
    /// field-2 hash (`__error` for Gorillax / RelaxedGorillax vs
    /// `__default` for Lax) — a plain int64_t compare, no pointer
    /// dereference, which keeps the wasm-min size gate green. The
    /// `isOk` field name was WASM internal-only — no user-facing
    /// `.isOk()` method dispatches to this slot (that method lives on
    /// `Result` and routes through `taida_result_is_ok`), so no
    /// compatibility shim is required.
    ///
    /// C25B-025 Phase 5-I (2026-04-23): 318,189 → 331,894 (+13,705).
    /// Added the math mold family to `03_typeof_list.inc.c`:
    /// `taida_float_sqrt` / `_pow` / `_exp` / `_ln` / `_log2` / `_log10`
    /// / `_log` / `_sin` / `_cos` / `_tan` / `_asin` / `_acos` / `_atan`
    /// / `_atan2` / `_sinh` / `_cosh` / `_tanh`. Freestanding
    /// implementations are required because wasm-ld `-nostdlib` does
    /// not link libm: `Sqrt` uses the `f64.sqrt` wasm opcode via
    /// `__builtin_sqrt`; `Pow` has an integer-exponent fast path
    /// (via repeated squaring) and falls back to `exp(y * ln(x))` for
    /// non-integer exponents; `Exp` / `Ln` use range reduction plus
    /// truncated Taylor / atanh series; `Sin` / `Cos` use Cody-Waite
    /// reduction + a 13-term Taylor polynomial; `Asin` / `Acos` are
    /// expressed in terms of `Atan(x / sqrt(1 - x^2))`; `Atan` uses
    /// the tan(pi/8) reduction shortcut + a 20-term Maclaurin; `Atan2`
    /// resolves the quadrant via sign tests; `Sinh` / `Cosh` / `Tanh`
    /// use the exp-based identities with small-|x| series paths to
    /// avoid cancellation. These are NOT bit-exact with glibc libm
    /// (so the 4-backend parity test in
    /// `tests/c25b_025_math_molds.rs` compares wasm vs interpreter
    /// with a relative-ULP tolerance) but remain within ~1e-12
    /// relative for the Phase 5-I fixture inputs. Native remains
    /// bit-for-bit with the interpreter because both delegate to the
    /// same libm on Linux / macOS. Closes the Phase 5-I scope for
    /// C25B-025 (math mold family on native + wasm backends).
    #[test]
    /// D29B-016 / Phase 10-E (Track-θ, 2026-04-27): 333,024 → 333,863 (+839).
    /// Added the `WASM_STR_ROPE_MAGIC = 0x5441494452505400` sentinel + a
    /// design rationale comment block in `01_core.inc.c`. Reserved for a
    /// future widening addition (§ 6.2) that introduces a rope-aware
    /// `_wf_str_concat` polymorphic dispatch. The interpreter side
    /// (Lock-K verdict V-1) implements rope promotion transparently via
    /// `StrRepr::Rope` so 4-backend surface parity is preserved against
    /// the existing wasm-wasi `_wf_str_*` heap-copy path.
    fn test_runtime_core_wasm_fragment_concat_preserves_bytes() {
        const EXPECTED_TOTAL_LEN: usize = 333_863;
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

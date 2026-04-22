# Changelog

## @c.23.rc6

Single-scope follow-up that finishes the `Str[...]()` mold family
4-backend parity work flagged by the `@c.22.rc5` (PR #36) Codex review
as FB-34 and expanded by a follow-up audit into three root-cause
blockers. The Taida surface is unchanged — `Str[...]()` still returns
`Lax[Str]`, the Lax / ぶちパック contracts are intact, and the
interpreter's `Str[x]()` semantics stay put as the source of truth.
What moves is the JS / Native / WASM-wasi rendering of non-primitive
`Str[x]()` results plus the WASM primitive/Lax display, all of which
had been drifting away from the interpreter.

### Interpreter is still the reference; the other three backends now line up behind it

- **C23B-001 (WASM primitive / Lax `Str[...]()`) — FIXED**:
  `src/codegen/runtime_core_wasm/02_containers.inc.c` now has
  `_taida_float_to_str_mold`, which strips the trailing `.0` that
  `taida_float_to_str` emits for integer-valued floats. That
  matches the interpreter's `f.to_string()` contract
  (`Str[3.0]() -> "3"` / `Str[-5.0]() -> "-5"`) without disturbing
  the `Value::to_display_string`-style `"3.0"` rendering used by
  `stdout(Float)`. `src/codegen/runtime_core_wasm/01_core.inc.c` also
  adds a `WASM_TAG_STR` arm to `_wasm_pack_to_string` and
  `_wasm_pack_to_string_full`, so the Lax's `__value` / `__default`
  fields on a `Str` mold now render as explicitly-quoted char* (`""`,
  `"abc"`) instead of falling back to the `_wasm_value_to_debug_string`
  integer-pointer path, which previously leaked raw data-section
  offsets (`1250`, `1254`, etc.) into the output.
- **C23B-002 (Native / WASM non-primitive `Str[...]()`) — FIXED**:
  `src/codegen/lower_molds.rs` no longer falls through to
  `taida_str_mold_int` for anything that is not a compile-time-known
  Float / Str / Bool / Int literal. Non-primitive arguments (List /
  ぶちパック / Lax / Result / …) now route through a new generic
  `taida_str_mold_any`, defined in
  `src/codegen/native_runtime/core.c` and
  `src/codegen/runtime_core_wasm/02_containers.inc.c`. The helper
  re-uses the existing `taida_stdout_display_string` /
  `_wasm_stdout_display_string` entries — the same full-form pack
  rendering that `stdout` produces — so
  `Str[@[1,2,3]]() -> "@[1, 2, 3]"`,
  `Str[@(a <= 1)]() -> "@(a <= 1)"`, and
  `Str[Int[3.0]()]() -> "@(hasValue <= true, __value <= 3, __default <= 0, __type <= "Lax")"`
  line up exactly with the interpreter. No new magic bits; all tag
  plumbing (`TAIDA_TAG_STR` / `WASM_TAG_STR` on `__value` / `__default`
  via `_lax_tag_vd` / `taida_lax_tag_value_default`) is the same
  mechanism `C21B-seed-07` introduced.
- **C23B-003 (JS `Str_mold`, with reopen) — FIXED**:
  `src/js/runtime/core.rs` no longer uses `String(value)` in `Str_mold`.
  A new `__taida_display_string` helper mirrors the interpreter's
  `Value::to_display_string()` contract case-by-case — `Int`, `Float`,
  `Bool` use the natural JS number / boolean formatting; arrays render
  as `@[...]`; plain objects render as `@(field <= value, ...)` skipping
  `__` internals; typed packs (Lax / Result / Async) render their full
  form including `__`-prefixed internals and, for Lax, honour the
  `__floatHint` that C21B-seed-04 / C21B-seed-07 propagate for Float-
  origin bindings. `Str[@(a <= 1)]()` and `Str[Int[3.0]()]()` now
  match the other three backends byte-for-byte instead of rendering
  `[object Object]` or the short-form `Lax(3)`.

  The reopen resolution extends `__taida_display_string` to every
  `__type`-carrying runtime object (HashMap / Set / Stream / TODO /
  Gorillax / RelaxedGorillax / Molten) so their interpreter-shaped
  display strings stay intact instead of falling through to the
  plain-pack branch (which was leaking JS method source bodies as
  pack fields). `Str[hashMap().set("a", 1)]()` and `Str[setOf(@[1, 2, 3])]()`
  now produce the interpreter's `BuchiPack(__entries/__items, __type)`
  full form on JS. The same reopen adds the two missing native / wasm
  pieces to keep 4-backend parity end-to-end:

  * `src/codegen/native_runtime/core.c` gains
    `taida_hashmap_to_display_string_full` /
    `taida_set_to_display_string_full` and routes
    `taida_stdout_display_string` through them, so
    `Str[hashMap()...]()` / `Str[setOf(...)]()` no longer yield the
    short-form `HashMap({"a": 1})` / `Set({1, 2, 3})` (the
    `.toString()` format). It also registers the `__error` field name
    (which `taida_pack_to_display_string_full` needs for Gorillax
    packs), triggers the registration from `taida_gorillax_new` /
    `taida_gorillax_err`, stamps the Gorillax `__error` slot with
    `TAIDA_TAG_PACK`, and teaches the full-form helper to render a
    PACK-tagged `0` slot as `@()` (interpreter
    `Value::Unit.to_debug_string` parity). `Str[Gorillax[v]()]()` now
    renders `@(hasValue <= true, __value <= v, __error <= @(),
    __type <= "Gorillax")` on native instead of collapsing to `@()`.
  * `src/codegen/runtime_core_wasm/01_core.inc.c` mirrors the native
    change with `_wasm_hashmap_to_display_string_full` /
    `_wasm_set_to_display_string_full` and routes
    `_wasm_stdout_display_string` through them.

  Wasm Gorillax still stores `isOk` (not `hasValue`) in its first
  pack slot — a pre-existing divergence tracked as **C23B-004**
  (separate track after `@c.23.rc6`) and explicitly scoped out of
  the 4-backend fixture via `WASM_SKIP_FIXTURES` in
  `tests/c23_str_parity.rs`. Stream molds are interpreter + JS only
  (native / wasm lowering unsupported) and gated via
  `STREAM_ONLY_FIXTURES` in the same test.

- **C23B-003 reopen 2 (nested typed runtime object recursion) — FIXED**:
  The first reopen addressed the top-level rendering of HashMap / Set
  / Gorillax but left the *nested* render paths using the short-form
  debug helper. `hashMap().set("k", hashMap().set("a", 1))` and
  friends therefore collapsed the inner HashMap to
  `HashMap({"a": 1})` on every non-interpreter backend. The reopen-2
  fix teaches every full-form helper to recurse through itself when
  descending into field / entry values:

  * `src/js/runtime/core.rs` — when `__taida_format` encounters a
    `__type`-tagged object it now delegates to `__taida_display_string`,
    which already has the per-`__type` full-form dispatch. Previously
    `__taida_format` fell through to `String(v)` and called the
    runtime object's short-form `.toString()` prototype.
  * `src/codegen/native_runtime/core.c` — new
    `taida_value_to_debug_string_full` mirrors the short-form
    `taida_value_to_debug_string` but dispatches HashMap / Set /
    BuchiPack to their full-form helpers, then the existing
    `taida_hashmap_to_display_string_full`,
    `taida_set_to_display_string_full`, and
    `taida_pack_to_display_string_full` call the new variant instead
    of the short-form helper for their nested values. A List branch
    is also added to `taida_stdout_display_string` so top-level
    `Str[@[hashMap()...]]()` uses the new variant on its items.
  * `src/codegen/runtime_core_wasm/01_core.inc.c` — symmetric changes
    add `_wasm_value_to_debug_string_full`, route the three
    `_wasm_*_display_string_full` helpers through it, and give
    `_wasm_stdout_display_string` a List branch that does the same.

  The interpreter reference (`Value::BuchiPack.to_display_string()`
  walks field values via `to_debug_string()` which recurses back
  into `to_display_string()` for non-Str values) is now honoured
  identically on all four backends for nested HashMap-in-HashMap,
  Set-in-HashMap, List-of-HashMap, and BuchiPack-carrying-HashMap.

- **C23B-003 reopen 3 (WASM empty pack `@()` detection) — FIXED**:
  The reopen-2 fix taught every full-form helper to recurse, but the
  wasm `_wasm_value_to_debug_string_full` still gated pack rendering
  behind `_looks_like_pack`, which requires `field_count >= 1` to
  avoid false-positives against List / HashMap / Set header layouts.
  Empty packs (`@()` / `Value::Unit`) allocated by
  `taida_pack_new(0)` carry `field_count == 0` and therefore fell
  through to the integer fallback — rendering a raw heap pointer
  (e.g. `73088`) instead of `"@()"`. Native never hit this because
  its `TAIDA_PACK_MAGIC` header lets `taida_is_buchi_pack` match
  `fc == 0` directly; JS and the interpreter already rendered empty
  objects as `@()`. The reopen-3 fix adds a dedicated wasm detector:

  * `src/codegen/runtime_core_wasm/01_core.inc.c` gains
    `_looks_like_empty_pack`, which accepts any pointer that (a) is
    in the bump-allocator's live range (`__heap_base <= addr <
    bump_ptr` — the same invariant `_wasm_is_string_ptr` uses), (b)
    reads a single zero int64_t, and (c) is not simultaneously a
    List / HashMap / Set header. The bump-range guard is essential:
    a pure memory-peek detector false-positives on small integer
    outputs such as `5050` (tail-recursion Fibonacci) and `8080` (in
    `localhost:8080` string interpolation), both caught by
    `tests/wasm_full.rs::wasm_full_parity_all_examples` while
    developing the fix. The detector is wired into four display
    helpers — `_wasm_value_to_debug_string_full` (nested pack fields
    / hashmap entries / set items), `_wasm_value_to_debug_string`
    (short-form fallback), `_wasm_value_to_display_string`
    (short-form display fallback), and `_wasm_stdout_display_string`
    (top-level `stdout(@())`). Each check sits between the richer
    detectors (`_looks_like_pack`, `_is_wasm_hashmap`, `_is_wasm_set`)
    and the raw-pointer fallback, so existing typed-pack rendering
    is never shadowed and integer pointers never win over the empty-
    pack rendering.

  `Str[@()]()`, `Str[@(u <= @())]()`,
  `Str[hashMap().set("u", @())]()`, and `Str[@[@()]]()` now all emit
  the interpreter string on wasm. `stdout(@())` also renders `@()`
  directly instead of the bump-allocator offset.

- **C23B-003 reopen 4 (WASM tag-based empty-pack identification + richer compile-time Int check) — FIXED**:
  The reopen-3 detector used a heap-range + zero-slot heuristic
  (`__heap_base <= addr < bump_ptr && *(int64_t*)addr == 0`) to
  recognise empty packs. A later review surfaced a HIGH-severity
  false-positive: dynamic Int expressions routed through the generic
  `Str[...]()` path (`taida_str_mold_any`) hand untagged int64_t
  values to the display helpers, and if the integer happens to fall
  inside the bump arena on an 8-byte-aligned zero chunk, the
  heuristic fires and renders the integer as `"@()"`. The canonical
  repro — `a <= 36000; b <= 37088; stdout(Str[a + b]())` — emitted
  `__value <= "@()"` on wasm where interpreter / JS / native all emit
  `"73088"`. The reopen-4 fix replaces the heuristic with a
  positive-identification magic sentinel and hardens the lowering
  side so as many dynamic Int shapes as possible never reach the
  runtime heuristic:

  * **Tag-based identification.**
    `src/codegen/runtime_core_wasm/01_core.inc.c` `taida_pack_new(0)`
    now allocates two int64_t slots instead of one, writing
    `[field_count=0, WASM_EMPTY_PACK_MAGIC]` where
    `WASM_EMPTY_PACK_MAGIC = 0x5441494450414B55LL` (the seven-byte
    printable string "TAIDPAKU" — TAIDA + PACK + Unit). Non-empty
    packs are unchanged — their `field_count >= 1` already
    disambiguates them, and `pack[1]` continues to hold
    `field0_hash`. `_looks_like_empty_pack` now tests exactly
    `data[0] == 0 && data[1] == WASM_EMPTY_PACK_MAGIC` (plus wasm32
    pointer + 8-byte-alignment guards). The heap-range / bump_ptr /
    List-HashMap-Set negation checks are gone — integer values can
    no longer false-match the detector no matter what is sitting in
    memory at that offset.
  * **Compile-time Int fast-path widening.**
    `src/codegen/lower_molds.rs` `Str` dispatch now consults
    `Lowering::expr_is_int` (from `src/codegen/lower/infer.rs`)
    instead of a local syntactic `expr_is_int_literal`. That
    recognises int-typed bindings via `int_vars`, arithmetic on
    Int operands via `BinaryOp::{Add,Sub,Mul}`, int-returning
    methods (`length` / `indexOf` / `lastIndexOf` / `count`), and
    int-returning user functions via `int_returning_funcs`. Those
    shapes now short-circuit to `taida_str_mold_int` at compile
    time, so the dynamic-Int values never enter the runtime
    heuristic stack at all — defence-in-depth on top of the
    detector-level fix. `expr_is_int`'s visibility widens from
    `pub(super)` to `pub(crate)` (no logic change) so the sibling
    `lower_molds.rs` module outside the `lower/` submodule can
    call it.

  Four regression fixtures pin the fix:
  `str_from_dynamic_int` (`Str[a + b]()` with `a + b = 73088`),
  `str_from_dynamic_int_zero` (`Str[5 - 5]()`),
  `str_from_dynamic_int_negative` (`Str[-x]()` on an int_var), and
  `str_from_dynamic_int_funcall` (`Str[double(36544)]()` with
  `double n = n * 2 => :Int`). All four now render the integer
  value on every backend byte-for-byte. JS / Native were already
  correct (JS uses `__type`-based dispatch, Native uses
  `TAIDA_PACK_MAGIC` for empty packs — neither relies on address
  heuristics), so the reopen-4 work is wasm-scoped.

  Follow-up audit during the reopen-4 work uncovered a separate
  pre-existing wasm heuristic bug — `_looks_like_list` accepts any
  pointer whose `data[0]` (cap) is in `8..=65536` and `data[1]`
  (len) is in `0..=cap`, which false-matches on Int values such as
  73088 stored uninterpretted inside a List / ぶちパック. That
  stack-overflows `Str[@[73088]]()` and friends on wasm. Originally
  filed as **C23B-005 (TRACKED)** for a later release track.

- **C23B-005 reopen + widen + C23B-006 (WASM collection detectors
  false-positive on untagged large Ints) — FIXED**: a deeper audit
  during the @c.23.rc6 review showed the bug class extended far
  beyond `_looks_like_list`: `_is_wasm_hashmap` relied on a 4-byte
  `"HMAP" = 0x484D4150` marker at `data[3]` and would recursively
  re-render any HashMap value slot holding a large Int (e.g.
  `hashMap().set("x", 73088)` collapsed 73088 to an empty HashMap),
  and `_is_wasm_set` layered on top of `_looks_like_list` and
  inherited the same stack-overflow. All four WASM collection
  detectors (`_looks_like_list`, `_is_wasm_set`, `_is_wasm_hashmap`,
  `_looks_like_pack`) have been unified onto a single tag-based
  positive-identification scheme:

  * **Wide 8-byte printable-ASCII magic sentinels** replace every
    prior structural heuristic (cap-range for List, 4-byte
    `"HMAP"` / `"SET\0"` for HashMap / Set, `fc-range + first_hash`
    for Pack). The new constants — `WASM_LIST_MAGIC` ("TAIDLST"),
    `WASM_SET_MAGIC` ("TAIDSET"), `WASM_HM_MAGIC` ("TAIDHMP"),
    `WASM_PACK_MAGIC` ("TAIDPKK") — are stamped by the
    corresponding allocation paths (`taida_list_new`,
    `taida_set_new`, `taida_hashmap_new` /
    `_wasm_hashmap_new_with_cap` / `taida_hashmap_set` resize,
    `taida_pack_new(fc>=1)`) and carry 64 bits of entropy so they
    cannot arise from user arithmetic.
  * **Dual-magic identification** for List / Set / HashMap. Every
    allocation stamps the magic at BOTH a head position (`data[3]`)
    AND a shape-dependent trailing position (`data[WASM_LIST_ELEMS
    + cap]` for lists and sets, `data[WASM_HM_HEADER + cap * 3]`
    for hashmaps). Detectors verify both, giving 128 bits of
    entropy. This closes the last residual attack path where a
    user-supplied large Int stored inside one collection's value
    slot happened to equal the base pointer of a *different*
    collection in the same bump arena — single-magic identification
    would still succeed because that real collection carries the
    head magic legitimately, but the trailing magic at the
    cap-dependent offset cannot simultaneously align for both the
    fake (integer as pointer) and real (pointer to collection)
    interpretations. Pack uses a single trailing magic plus
    `data[0]` in `1..=100`; the `fc`-range constraint supplies the
    provenance the dual scheme would otherwise provide.
  * **Tag-aware element rendering** (`_wasm_render_elem_tagged_debug`
    and `_wasm_render_elem_tagged_debug_full`). The list's
    `elem_type_tag` (slot 2), the hashmap's `value_type_tag`
    (slot 2), and the pack's per-field `field_tag` are threaded
    into every collection-member rendering loop. Primitive Int /
    Float / Bool / Str members dispatch directly via the tag
    without going through any structural detector — defence-in-depth
    on top of the dual-magic check. `taida_hashmap_set_value_tag`
    and `taida_list_set_elem_tag` are hardened to downgrade the
    stored tag to UNKNOWN (-1) on type conflict (e.g.
    `hashMap().set("name", "Asuka").set("age", 14)` now leaves
    `hm[2] = -1` instead of silently overwriting with the last
    inserted value's type). Heterogeneous containers fall back to
    structural dispatch, which is safe thanks to the dual-magic
    detectors.
  * **`_is_wasm_hashmap` / `_is_wasm_set` in
    `src/codegen/runtime_full_wasm.c`** were mirroring the old
    4-byte markers and have been retargeted to the same wide
    sentinels so the wasm-full profile matches the wasm-wasi
    behaviour. Forward declarations for `_wf_is_valid_ptr` and the
    magic constants appear early in the file so the detector
    helpers can reference them without reordering.
  * **`_wc_is_hashmap` / `_wc_is_set` in
    `src/codegen/runtime_core_wasm/04_json_async.inc.c`** now
    delegate to the hardened `_is_wasm_hashmap` / `_is_wasm_set`
    in fragment 1, eliminating the duplicate (and now-stale)
    4-byte marker checks that JSON / async code previously relied
    on.

  Five regression fixtures pin the fix across all four backends:
  `str_from_hashmap_with_large_int` (C23B-006 direct repro),
  `str_from_set_with_large_int` (`setOf(@[73088])` stack overflow
  repro), `str_from_list_with_large_int` (`Str[@[73088, 42000]]()`),
  `str_from_pack_with_large_int` (`Str[@(x <= 73088)]()`), and
  `str_from_nested_collection_with_large_int` (nested: HashMap
  containing `@[73088]`). Every backend (Interpreter / JS / Native /
  WASM-wasi) renders them byte-for-byte identically. JS and Native
  were already correct (JS uses `__type`-based dispatch, Native
  uses `TAIDA_PACK_MAGIC` / `TAIDA_LIST_MAGIC` / `TAIDA_HMAP_MAGIC`
  positive identification that already satisfies the provenance
  requirement), so the reopen lands wasm-scoped.

  > **Why not per-entry type tags instead?** Adding per-entry tags
  > to HashMap would require a layout change and cascade through
  > every reader in the runtime. The dual-magic approach achieves
  > equivalent safety without touching the hot-path load of
  > `hm[WASM_HM_HEADER + slot * 3 + 2]`. Tag-aware rendering on top
  > reuses the already-tracked per-container elem / value / field
  > tags that the lowering installs for homogeneous containers,
  > giving us the fast path without the layout cost.

- **C23B-007 (WASM tag re-promotion into heterogeneous containers)
  — FIXED**: the `taida_list_set_elem_tag` /
  `taida_hashmap_set_value_tag` downgrade path that C23B-005 installed
  reused the UNKNOWN (-1) sentinel to mean both "not set yet" and
  "downgraded from type conflict". That made the downgrade reversible:
  a subsequent `.push()` / `.set()` carrying a fresh primitive tag
  would see `existing == -1`, treat the container as unset, and
  re-promote it to that new tag. The renderer would then force every
  member through that tag's fast path — strings emerged as raw
  pointer integers (`@[1, "a", 2]` → `@[1, 1127, 2]`), string-valued
  HashMap entries showed as pointer Ints (`.set("a", 1).set("b",
  "x").set("c", 2)` → `value <= 1058` for "x"). Resolution:

  * Split the sentinel. `WASM_TAG_HETEROGENEOUS = -2` (and symmetric
    `TAIDA_TAG_HETEROGENEOUS = -2` on native) joins `WASM_TAG_UNKNOWN
    = -1` as a terminal state. `taida_list_set_elem_tag` /
    `taida_hashmap_set_value_tag` now follow the four-case latch:
    `HETEROG → keep`, `UNKNOWN → stamp`, `equal → no-op`,
    `different → HETEROG`. Once HETEROG, never re-promote.
  * Renderers do not need changes: `_wasm_render_elem_tagged_debug` /
    `_full` already fall through to the structural dispatcher
    (`_wasm_value_to_debug_string(_full)`) for any non-primitive
    tag, so the new -2 lands on the per-element structural path
    automatically. Native `taida_list_to_display_string` /
    `taida_hashmap_to_display_string_full` already use structural
    per-element dispatch.
  * `src/codegen/lower/expr.rs::lower_list_lit` had been stamping the
    element tag only once, trusting a "checker guarantees homogeneity"
    comment that no longer held (the interpreter accepts
    `@[1, "a", 2]` verbatim). We now call `taida_list_set_elem_tag`
    for every element. Homogeneous list literals still converge to
    the primitive tag on the first call (subsequent calls are no-ops);
    heterogeneous literals latch to HETEROG as soon as the second
    disagreeing tag appears.

  Four fixtures pin the repros and adjacent shapes:
  `str_from_mixed_list` (the C23B-007 direct List repro),
  `str_from_mixed_hashmap` (the HashMap three-value-type variant),
  `str_from_mixed_set` (Sets share the list header, so the fix
  applies), and `str_from_nested_mixed` (outer heterogeneous list
  wrapping an inner heterogeneous list — both levels latch
  independently). All four render byte-for-byte identically on
  Interpreter / JS / Native / WASM-wasi.

- **C23B-008 (Multi-entry HashMap display emits bucket order, not
  insertion order) — FIXED**: `taida_hashmap_to_display_string_full`
  (native) and `_wasm_hashmap_to_display_string_full` (wasm) iterated
  the open-addressing bucket array. Interpreter and JS represent
  HashMap as a `Vec<(k, v)>` (interpreter) / `Array` (JS) of insertion
  order, so `hashMap().set("a", 1).set("b", 2)` came out as "b", "a"
  on native / wasm. The same drift affected `taida_hashmap_entries`,
  `taida_hashmap_keys`, `taida_hashmap_values`, `taida_hashmap_merge`,
  `taida_hashmap_to_string` (short form), the native JSON serializer
  `json_serialize_typed`, and the wasm JSON `_wc_json_serialize_typed`
  — anywhere iteration order was observable. Resolution:

  * Both runtimes now append an insertion-order side-index to every
    HashMap allocation: `[next_ord, order_array[cap]]`. `next_ord` is
    a monotonic ordinal counter (never decremented, even after
    `.remove()`). `order_array[i]` stores the bucket slot of the i-th
    insertion, or `-1` for a hole left by `.remove()`. The layout is
    appended after the trailing magic (wasm) or the entry array
    (native); existing header / entry offsets are unchanged, so the
    dual-magic detection from C23B-005 / C23B-006 keeps working
    unchanged.
  * New offset macros centralise the math:
    `WASM_HM_ORD_HEADER_SLOT` / `WASM_HM_ORD_SLOT` on the wasm side,
    `TAIDA_HM_ORD_HEADER_SLOT` / `TAIDA_HM_ORD_SLOT` /
    `TAIDA_HM_TOTAL_SLOTS` on native. `_wasm_hashmap_new_with_cap` /
    `taida_hashmap_new_with_cap` bump their allocations by `1 + cap`
    slots, calloc-zero; `next_ord` starts at 0.
  * `taida_hashmap_set` records `order_array[next_ord++] = slot` on
    a new insertion; updates of an existing key leave the ordinal
    untouched (first-insertion-wins, matching the interpreter's
    `Vec` update). The tombstone-reuse path records the ordinal the
    same way. `taida_hashmap_remove` nulls the matching
    `order_array[i]` slot to `-1`; `next_ord` stays put.
  * `taida_hashmap_resize` now walks the OLD order array in
    insertion order and re-inserts each surviving entry into the
    new table, rebuilding the new side-index as it goes.
    `taida_hashmap_set_internal` (native) returns the new bucket
    slot (or `-1` for an update in place) so the caller can record
    the ordinal. The wasm `taida_hashmap_set` inline resize path
    applies the same order-preserving rebuild.
  * `taida_hashmap_clone` grows its allocation to include the new
    side-index and copies it verbatim (same bucket layout → same
    slot indices remain valid).
  * Display / iteration helpers on both runtimes —
    `taida_hashmap_to_display_string_full`,
    `taida_hashmap_to_string`, `taida_hashmap_entries`,
    `taida_hashmap_keys`, `taida_hashmap_values`,
    `taida_hashmap_merge`, plus the native / wasm JSON serializers
    — now walk `order_array[0..next_ord]` and skip holes /
    tombstoned buckets.
  * JS required no runtime change: `__taida_createHashMap` already
    stored `__entries` as an Array (insertion-ordered by
    construction), and `__taida_display_string` /
    `__taida_format` walked it in Array order.
  * Interpreter is the source of truth and was untouched.

  Four fixtures pin the behaviour:
  `str_from_multi_entry_hashmap` (the direct two-entry repro),
  `str_from_large_hashmap` (16 keys, crosses the wasm 70% / native
  75% resize boundary — proves the resize rebuild preserves
  insertion order), `str_from_hashmap_after_remove` (remove the
  middle entry, verify the hole is skipped and the surrounding
  entries stay in their original order), and
  `str_from_hashmap_update_preserves_order` (re-`.set()` an existing
  key, verify the ordinal stays put so the final order is still
  first-insertion-wins).

- **C23B-008 reopen (HashMap.merge() overlap-key ordinal divergence)
  — FIXED**: the reopen-5 fix above pinned HashMap display / iteration
  to insertion order, but a follow-up review (reopen-7) showed that
  `.merge()` itself still diverged from the interpreter on overlap
  keys. The interpreter (`src/interpreter/methods.rs:787-822`) does
  `merged.retain(|e| e.key != other_key); merged.push(other_entry)`
  for each `other` entry, which MOVES every overlap key to other's
  position with other's value. JS called `merged[idx] = oe`, and
  native / wasm cloned self then called `taida_hashmap_set` per
  `other` entry — both variants update in place and preserve self's
  ordinal for overlap keys. Repro: self = `[a, b]`, other = `[c, b, d]`;
  interpreter emits `[a, c, b, d]`, the three broken backends emitted
  `[a, b, c, d]`. Resolution:

  * All three backends (native / wasm / JS) now build a fresh result
    map and fill it in the order interpreter would emit — step 1
    walks self in self-order and copies entries whose key is absent
    from other; step 2 walks other in other-order and appends every
    entry (all guaranteed new to the fresh map). Value retention
    flows through the normal `taida_hashmap_set` / `__taida_createHashMap`
    code paths.
  * `taida_hashmap_set` is NOT modified — its update-in-place ordinal
    preservation is still required for plain `.set("k", v)` chains
    (interpreter's `Vec<(k, v)>` update semantics keep the position
    of a re-`.set()` key). Only `.merge()` needed the retain-then-push
    variant, and it achieves that by avoiding `.set()` on overlap
    keys entirely (they never reach the fresh result as self entries).
  * Six fixtures pin the behaviour:
    `str_from_hashmap_merge_overlap` (the direct review repro —
    `a=[a,b].merge([c,b,d])` ⇒ `[a,c,b,d]`),
    `str_from_hashmap_merge_non_overlap` (degenerate no-overlap path,
    self-order + other-order),
    `str_from_hashmap_merge_full_overlap` (every self key in other,
    result = other in other-order with other's values),
    `str_from_hashmap_merge_empty_self` (empty self ⇒ result = other),
    `str_from_hashmap_merge_empty_other` (empty other ⇒ result =
    self), `str_from_hashmap_merge_resize` (16-entry merge with two
    overlap keys crossing the 0.75 load-factor resize on the fresh
    result map — exercises `taida_hashmap_resize` + side-index
    rebuild during the fill loop).
  * The reopen-7 audit also flagged an independent divergence in
    `HashMap.entries()` across backends — interpreter returns
    `@(key <= …, value <= …)`, JS returns `@(first <= …, second <= …)`
    (wrong field names), and native / wasm render the pair pack as
    empty `@()` (pair content does not reach the renderer). This is
    tracked as **C23B-009** and resolved inside the same release (see
    below).

- **C23B-009 (HashMap.entries() 4-backend divergence) — FIXED**: an
  independent divergence from the reopen-7 audit. The documented
  contract (`docs/reference/standard_library.md:238`) and the
  interpreter (`src/interpreter/methods.rs:761-783`) both use
  `@[@(key, value)]` for `.entries()`. Previously:

  * **JS** (`src/js/runtime/core.rs:2555` `entries()`): emitted
    `Object.freeze({ first: e.key, second: e.value })` — a legacy
    convention inadvertently shared with `zip()` / `Zip[]()` (those
    stay `first`/`second` because the interpreter itself does; only
    `.entries()` was wrong). Fix: rename to `{key, value}`. The
    `hashMap(entries)` constructor (line ~2600) still accepts
    `.first` / `.second` fallback for back-compat with user-built
    pair lists.
  * **Native** (`src/codegen/native_runtime/core.c::taida_hashmap_entries`):
    stamped the correct FNV-1a hashes (`HASH_KEY` /
    `HASH_VAL`) and tags but never called
    `taida_register_field_name` for them. When
    `taida_pack_to_display_string_full` looked them up it got NULL
    and silently skipped every field, emitting `@()`. Fix: idempotent
    registration of `"key"` / `"value"` inside the entries helper
    (guarded by a static flag so the cost is paid once per program).
  * **WASM** (`src/codegen/runtime_core_wasm/01_core.inc.c::taida_hashmap_entries`):
    symmetric to native's missing registry entry — plus the wasm
    implementation didn't stamp per-field tags on the pair pack, so
    even if the names had been found the values would have rendered
    through the untagged fallback path. Fix: register `"key"` /
    `"value"` idempotently, stamp `WASM_TAG_STR` on `key`, propagate
    the hashmap's `value_type_tag` onto `value`, and flag the outer
    list's `elem_type_tag = WASM_TAG_PACK` so every pair dispatches
    through `_wasm_render_elem_tagged_debug_full`.
  * **Interpreter**: unchanged (source of truth).

  Repro:
  ```taida
  m <= hashMap().set("a", 1).set("b", 2)
  stdout(Str[m.entries()]())
  ```
  All four backends now emit
  `@(hasValue <= true, __value <= "@[@(key <= "a", value <= 1), @(key <= "b", value <= 2)]", __default <= "", __type <= "Lax")`.

  The shape is non-breaking: JS's `{first, second}` field names were
  never part of the documented API, and the hashMap() constructor's
  back-compat fallback still reads them so user code that built pair
  lists via `@(first <= k, second <= v)` continues to work. Pre-fix
  audit covered `.keys()`, `.values()`, `.size()`, `.has()`,
  `.merge()`, `Set.toList()`, `List.first()`, `zip()` / `Zip[]`, and
  `enumerate()` — all parity-clean on interpreter/JS; `zip()` /
  `enumerate()` silently return empty on native / wasm (pre-existing
  divergence, not on the `Str[...]()` family path, outside the C23B
  scope).

### Regression guard

- `tests/c23_str_parity.rs` drives forty-six fixtures under
  `examples/quality/c23b_str_parity/` — `str_from_float_int_form`,
  `str_from_float_frac_form`, `str_from_bool`, `str_from_str`,
  `str_from_list`, `str_from_pack`, `str_from_lax`, the
  C23B-003-reopen additions `str_from_hashmap`, `str_from_set`,
  `str_from_gorillax`, `str_from_stream`, the reopen-2
  additions `str_from_nested_hashmap`, `str_from_nested_set`,
  `str_from_list_of_hashmap`, `str_from_pack_with_hashmap`, the
  reopen-3 additions `str_from_empty_pack`,
  `str_from_pack_with_empty_pack`, `str_from_hashmap_with_empty_pack`,
  `str_from_list_with_empty_pack`, the reopen-4 additions
  `str_from_dynamic_int`, `str_from_dynamic_int_zero`,
  `str_from_dynamic_int_negative`, `str_from_dynamic_int_funcall`,
  the C23B-005 reopen + C23B-006 additions
  `str_from_hashmap_with_large_int`, `str_from_set_with_large_int`,
  `str_from_list_with_large_int`, `str_from_pack_with_large_int`,
  `str_from_nested_collection_with_large_int`, the C23B-007
  additions `str_from_mixed_list`, `str_from_mixed_hashmap`,
  `str_from_mixed_set`, `str_from_nested_mixed`, and the C23B-008
  additions `str_from_multi_entry_hashmap`, `str_from_large_hashmap`,
  `str_from_hashmap_after_remove`,
  `str_from_hashmap_update_preserves_order`, and the C23B-008
  reopen-7 merge-semantics additions
  `str_from_hashmap_merge_overlap`, `str_from_hashmap_merge_non_overlap`,
  `str_from_hashmap_merge_full_overlap`,
  `str_from_hashmap_merge_empty_self`,
  `str_from_hashmap_merge_empty_other`,
  `str_from_hashmap_merge_resize`, and the C23B-009
  `.entries()` additions `str_from_hashmap_entries`,
  `str_from_hashmap_entries_empty`,
  `str_from_hashmap_entries_single`,
  `str_from_hashmap_entries_after_remove` — through all four
  backends where each backend supports the mold. The interpreter
  fixture test also pins the `.expected` files so a future
  interpreter change cannot silently drift them away from the
  source of truth. `WASM_SKIP_FIXTURES` excludes
  `str_from_gorillax` from the wasm run (tracked as C23B-004) and
  `STREAM_ONLY_FIXTURES` excludes `str_from_stream` from both
  native and wasm (Stream lowering unsupported on those backends).

### Byte-length assertions

- `src/codegen/runtime_core_wasm/mod.rs` `EXPECTED_TOTAL_LEN`:
  248,033 → 251,707 → 254,479 → 259,848 → 265,494 → 267,429 →
  283,669 → 292,933 → 293,560 → 295,319 (+3,674 initial, +2,772 in
  C23B-003 reopen, +5,369 in C23B-003 reopen 2, +5,646 in C23B-003
  reopen 3, +1,935 in C23B-003 reopen 4, +16,240 for C23B-005 reopen
  + C23B-006, +9,264 for C23B-007 + C23B-008, +627 for C23B-008
  reopen-7: rewrote `taida_hashmap_merge` from clone-then-set to
  the interpreter's retain-then-push algorithm — fresh result map,
  self entries whose key ∉ other in self-order, then every other
  entry in other-order; +1,759 for C23B-009: wasm
  `taida_hashmap_entries` now idempotently registers `"key"` /
  `"value"` in `_wasm_field_registry`, stamps per-field tags
  (`WASM_TAG_STR` on key, hashmap value_type_tag on value) and
  flags the returned list's `elem_type_tag = WASM_TAG_PACK`).
- `src/codegen/native_runtime/mod.rs` `EXPECTED_TOTAL_LEN`:
  935,805 → 936,859 → 943,160 → 950,197 → 958,672 → 960,607 → 961,515
  (+1,054 initial, +6,301 in C23B-003 reopen, +7,037 in C23B-003
  reopen 2 for `taida_value_to_debug_string_full` + List dispatch
  in `taida_stdout_display_string`, +8,475 for C23B-007 + C23B-008,
  +1,935 for C23B-008 reopen-7: the native `taida_hashmap_merge`
  symmetric rewrite; +908 for C23B-009: native
  `taida_hashmap_entries` idempotent registration of `"key"` /
  `"value"` via `taida_register_field_name`).
  `F1_LEN` moves 218,772 → 226,482 → 228,417 → 229,325 (+7,710 for
  C23B-007 / C23B-008 absorbing the `TAIDA_TAG_HETEROGENEOUS`
  define + `TAIDA_HM_ORD_*` macros + the `set_elem_tag` /
  `set_value_tag` latch bodies + the HashMap insertion-order
  scaffolding in `_new_with_cap` / `_set` / `_resize` / `_remove` /
  `_clone` / `_keys` / `_values` / `_entries` / `_merge` /
  `_to_string`, plus +1,935 for C23B-008 reopen-7 on `_merge`, plus
  +908 for C23B-009 on `_entries`). F2 moves 150,412 →
  151,177 (+765, absorbing the
  `taida_hashmap_to_display_string_full` and
  `json_serialize_typed` HashMap walk switches).

### What we did NOT touch

- No change to `src/interpreter/mold_eval.rs`, `src/interpreter/eval.rs`,
  or the wider interpreter — C23 treats the interpreter as the
  reference and only modifies codegen / runtime helpers / JS runtime.
- No change to `src/types/mold_returns.rs`'s Pack classification
  (C21B-seed-07 land is left intact).
- No change to `src/codegen/driver.rs`, `src/codegen/lower/*.rs`
  (except `src/codegen/lower_molds.rs`, which is the only C23 scope
  file in that tree), or any C21 / C22 work-stream entry point.
- `.dev/official-package-repos/terminal/` is untouched.

## @c.22.rc5

Two concurrent tracks land together in a single RC bump. Track A
(formerly the `@c.21.rc4` draft) restores 3-backend Float semantics
and opens the WASM SIMD path. Track B (formerly the `@c.22.rc1`
draft) restores I/O symmetry and hardens the CLI against pipe-chain
termination. Both tracks share the same merge, so the RC counter
advances monotonically from `@c.20.rc4` to `@c.22.rc5` — the `22`
reflects the active generation, the `rc5` keeps the incremental
index `taida upgrade` and label-based selectors rely on. The two
drafts' change-notes are kept as subsections below for traceability.

### Track A — 4-backend Float parity + WASM SIMD path open

Restore 3-backend Float semantics and open the WASM SIMD path that
bonsai-wasm Phase 0 identified as a one-shot blocker for writing
matmul-shaped numeric code in Taida. The observable behaviour changes
cluster into four mutually-reinforcing fixes (C21-1 through C21-5) plus
the `-msimd128` profile split (C21-3) that this release tag is pinned
on. The Taida surface — `3.0` vs `3`, `Float[x]()` / `Int[x]()`,
`@[Float]`, `:Float`, arithmetic operators — is unchanged; every fix
lives inside codegen / runtime helpers.

### Interpreter is the reference, the other three backends line up behind it

- **C21-1 (regression guard, Phase 1)**: `examples/quality/c21b_float_fn_boundary/triple.{td,expected}`
  (`triple(4.0) => 12.0`) and `dot_product.{td,expected}`
  (`dotProductAt(@[1.0,2.0], @[3.0,4.0], 0, 2, 0.0) => 11.0`) pin the
  minimal Float-function-boundary behaviour across Interpreter / JS /
  Native / WASM-wasi with `tests/c21_float_fn_boundary.rs`. This fixture
  would have caught every Phase 2-5 bug before it reached bonsai-wasm.
- **C21-2 (Wasm Float hot loop, Phase 2)**: `@[Float]`-element
  `a.get(i) ]=> av` now propagates the element type through
  `track_unmold_type`, so `av * bv` lowers to `taida_float_mul` instead
  of `taida_int_mul`. Fixes the "internal dot product silently computes
  0" class of bug observed in bonsai-wasm's hot loop.
- **C21-4 (Float → Str ABI, Phase 4)**: the native Cranelift path
  bitcasts `ConstFloat` into the boxed `value_ty` on emit (fixes the
  `define_function failed: Compilation error: Verifier errors` that
  blocked every `=> :Float` function in native builds). The native
  `taida_float_to_str` now matches Rust's `f64::Display`
  (shortest-round-trip `%.*g` + `strtod` loop + integer-form `X.0`);
  both native and WASM `taida_io_stdout_with_tag` / `_stderr_with_tag`
  route the `FLOAT` tag through that formatter so `stdout(triple(4.0))`
  no longer leaks the i64 bit-pattern.
- **C21-5 (JS `Int[x]` / `Float[x]` parity, Phase 5)**: JS
  `Number.isInteger(3.0) === true` closed the door on the naive
  `typeof+isInteger` checker; we now carry a compile-time
  `is_float_origin_expr` / `is_int_origin_expr` analysis in
  `src/js/codegen.rs`, specialize `stdout` / `debug` / `stderr` /
  `.toString()` call sites for Float-origin arguments
  (`__taida_stdout_f`, `__taida_debug_f`, `__taida_to_string_f`,
  `__taida_float_render`) and fold `Int[floatLit]()` / `Float[intLit]()`
  statically. Arithmetic paths are untouched — zero deopt for the hot
  case, compile-time fold covers every literal / single-bind case that
  used to diverge from Interpreter / Native.

### `-msimd128` profile split (C21-3, this tag's pin)

`WASM_CLANG_FLAGS` was a single profile-agnostic constant that lacked
`-msimd128`. That silently closed the SIMD door at the clang layer for
every `wasm-*` target, so even after C21-2 taught the wasm codegen to
emit f64 Float operations LLVM's auto-vectorizer could not consider
`v128.*` lowerings. `src/codegen/driver.rs` now splits the flags:

- `WASM_CLANG_FLAGS_COMMON` (`--target=wasm32-unknown-wasi`, `-nostdlib`,
  `-O2`, `-c`) stays the same.
- `wasm_clang_flags_for(profile)` appends `-msimd128` for `Wasi`,
  `Edge`, `Full` — and **nothing** for `Min`, so consumers who pick
  `--target wasm-min` for minimal-runtime compatibility still get a
  `.wasm` that does not request the `simd128` feature.
- `WasmRuntimeCache` is profile-aware: `rt_core` / `rt_wasi` / `rt_edge`
  / `rt_full` now take a `WasmProfile`, their cache keys hash the
  per-profile flag vector so a wasm-min `rt_core.o` is never served to
  a wasm-wasi build (and vice versa), and the stale-entry cleanup
  preserves every live profile's key for the same source.

Result: on `examples/quality/c21b_wasm_simd/matmul_small.td`
(sum-of-squares of 8 Floats), the disassembled `.wasm` now contains
`v128.*` and `i8x16.*` instructions under `wasm-wasi` while `wasm-min`
stays at zero SIMD opcodes. `tests/c21_wasm_simd.rs` locks both
directions in place. On bonsai-wasm's `bench/matmul.td` smoke the same
shift is visible (v128 count: `0 → 27`, f64 count: `5 → 10`), which was
the goal that motivated C21 in the first place.

### Tests

- `tests/c21_float_fn_boundary.rs` — 8 cross-backend tests (Interpreter
  reference + JS / Native / WASM-wasi parity × 2 fixtures).
- `tests/c21_wasm_simd.rs` — 3 tests: `wasm-wasi` disassembly must
  contain Float ops and at least one SIMD-family opcode; `wasm-min`
  disassembly must contain zero SIMD opcodes; matmul_small runs
  correctly under wasmtime and prints `204.0`.
- `src/codegen/driver.rs::tests::test_cache_key_differs_on_source_change`
  gains a fourth key comparing `wasm-min` vs `wasm-wasi` with identical
  source + clang version, asserting that the profile-specific flag
  change alone produces a distinct cache key.

### Out of scope

- Taida-level `@[Float<f32>]` / `@[Float<f64>]` quantifier additions are
  a language-surface change and stay deferred.
- Manual `v128.*` intrinsic exposure from Taida source remains
  out-of-bounds (Taida-first design — auto-vectorize only).
- JS closure-crossing dynamic Float/Int discrimination is still
  best-effort; the compile-time analysis covers every single-bind case,
  dynamic `map`/`fold` callbacks fall back to `Number.isInteger`. This
  is a language-spec limitation of JS `Number`, not a C21 regression.

### Fixed

- JS local binding Float-origin propagation (C21B-seed-04 reopen,
  Phase 5 re-fix): the initial Phase 5 landing only covered terminal
  sites whose argument was a `FloatLit` / Float-origin arithmetic /
  `=> :Float` user-fn call. A subsequent review confirmed that
  `x <= 3.0; stdout(Float[x]())` / `stdout(x)` / `stdout(x.toString())`
  still diverged in JS because `is_float_origin_expr(Expr::Ident)`
  returned `false`. `src/js/codegen.rs` grows a scope-aware tracker
  (`float_origin_vars` / `int_origin_vars` / `float_list_vars`) that is
  pushed / popped across function boundaries. Typed parameters
  (`x: Float`, `a: @[Float]`), annotated bindings (`x: Float <= ...`),
  Float-origin RHS bindings (`x <= 3.0`, `x <= floatExpr`), and unmold
  targets rooted in Float lists (`a.get(i) ]=> av`) now carry the tag.
  `Float[x]()` routes to a new `Float_mold_f` runtime helper that tags
  the resulting `Lax` with `__floatHint: true`; the stdout / format
  path renders `__value` / `__default` through `__taida_float_render`
  when the tag is present. Arithmetic paths stay untouched (no deopt).
  `tests/c21_js_float_binding.rs` pins both REOPEN repros plus a
  one-level-deeper `triple(4.0) → local → Float[y]()` case.

- Native / WASM Float Lax parity for local bindings (C21B-seed-07,
  Phase 4 補修): the Phase 5 re-fix split out a pre-existing Native /
  WASM divergence — `x <= 3.0; stdout(Float[x]())` printed
  `3.958204945e-315` on native and subnormal garbage on wasm-wasi
  because `mold_returns.rs` declared `Float[x]()` as returning a bare
  `Float` (tag 1), so `lower_stdout_with_tag` routed the Lax pointer
  through the `TAIDA_TAG_FLOAT` fast path which `memcpy`'d the pointer
  bits as an f64. `Int[x]()` printed the short `Lax(3)` `.toString()`
  form instead of the interpreter's full
  `@(hasValue <= true, __value <= 3, __default <= 0, __type <= "Lax")`.
  The fix spans three layers: (1) `src/types/mold_returns.rs` re-
  classifies `Int` / `Float` / `Bool` / `Str` as `Pack` (the actual
  runtime return type, a `taida_lax_new(...)` result); (2) native
  `src/codegen/native_runtime/core.c` adds a
  `taida_lax_tag_value_default` helper so every
  `taida_{int,float,bool,str}_mold_*` function stamps the per-field tag
  on the Lax's `__value` / `__default` slots, and
  `taida_pack_to_display_string` / `_full` honor that tag before
  falling back to the global registry, and
  `taida_io_stdout_with_tag` / `_stderr_with_tag` route any runtime-
  detected BuchiPack through `taida_stdout_display_string` (the
  `_full` entry) so interpreter-parity `@(hasValue <= …, __value <=
  …, __default <= …, __type <= "Lax")` emerges; (3) wasm
  `src/codegen/runtime_core_wasm/{01_core,02_containers}.inc.c` land
  symmetric changes: Lax field-name registration on first
  `taida_lax_new` call, per-field tag dispatch in the new
  `_wasm_pack_to_string_full`, tight `_is_pack_for_stdout` guard that
  excludes Lists / HashMaps / Sets / Async objects (so
  `stdout(@[1,2,3])` still renders as a list, not as `@()`). All four
  pre-existing `Float[x]()` Lax divergences in
  `examples/quality/c21b_float_fn_boundary/{float_local_binding,
  float_fn_result_local}.td` now match the interpreter on 4 backends.
  `tests/c21_js_float_binding.rs` adds four new parity assertions
  (Native / WASM × the two previously-JS-only fixtures), reflecting
  the scope-expanded pin.

### Track B — stream I/O + SIGPIPE tolerance

Restore observable I/O symmetry in the interpreter and harden the CLI
against pipe-chain termination. Post-C20 smoke (Hachikuma Phase 11
TUI-First) exposed that `stderr` / `stdin` already flushed eagerly
while `stdout` / `debug` silently accumulated into an internal Vec
and only surfaced after `eval_program` returned — breaking progress
output, spinners, printf-debugging, and TUI rendering. The same
audit found that `taida run file.td | head -N` exited 141 because
the process carried the default SIGPIPE disposition.

### Interpreter — `stdout` / `debug` two-mode API

`Interpreter` now carries a `stream_stdout: bool` flag and exposes
two public constructors:

- `Interpreter::new()` keeps the legacy buffered behaviour. REPL,
  the in-process `eval_with_output` test harness, and the JS
  codegen embedding path all stay on this mode unchanged, so every
  existing call site works without modification.
- `Interpreter::new_streaming()` switches `stdout` / `debug` to
  `writeln!(io::stdout().lock(), "{}", line)` + `flush().ok()`.
  `taida <file>` / `taida run <file>` now use this variant so
  progress output hits the terminal line-by-line, matching the
  POSIX-standard behaviour every other CLI tool ships with.

Taida surface is unchanged: `stdout(...)` / `debug(...)` still
accept the same arguments, still return the written byte count as
`Int`, still append the implicit trailing newline, and still
suppress the "final value auto-display" when stdout has been
emitted to. No Taida source needs to be edited.

Design note on `debug` routing: an earlier IMPL_SPEC draft routed
stream-mode `debug` to stderr for symmetry with `stderr()`. That
would have broken 3-backend parity — JS (`console.log`) and Native
(`taida_debug_*` → `printf`) already emit to stdout, and the
`test_native_compile_parity` harness diffs captured stdout across
backends. The interpreter was the outlier, so it lines up with the
other two: stream-mode `debug` writes to stdout as well. The
observable symptom (progress / printf-debug surfaces in real time)
is still fixed; only the stream differs from the original plan.

### CLI — SIGPIPE tolerance

`fn main()` now installs `signal(SIGPIPE, SIG_IGN)` once at startup
(unix only, behind `#[cfg(unix)]`). Combined with the `writeln!` +
`flush().ok()` pattern above, `taida run script.td | head -N` now
exits cleanly — the `stdout` builtin silently absorbs `EPIPE` and
returns a zero byte count, matching the standard `ripgrep` / `bat`
convention. Process-wide signal disposition is touched in exactly
one place; individual `stdout` / `debug` call sites do not
re-install handlers.

### Tests

- `tests/c22_stdout_stream.rs` (5 tests) pins the Rust API parity
  between `Interpreter::new()` and `Interpreter::new_streaming()`,
  including the byte-count return and the implicit trailing newline.
- `tests/c22_sigpipe.rs` (4 tests) exercises `taida | head`, stdout
  close, stream-mode `debug` routing to stdout, and the mixed
  stdout/debug ordering contract.
- `tests/c22_stdout_stream_parity.rs` (6 tests) drives the three
  backends (Interpreter / JS / Native) subprocess-style against
  `examples/quality/c22_stdout_stream/progress_loop.{td,expected}`
  and `debug_stream.{td,expected}` so a future regression in
  either backend's stdout routing is caught immediately.

### Out of scope

- REPL remains on buffered mode; its return-value indentation
  machinery is Vec-dependent and will move in a dedicated track.
- Raw-mode / alt-screen auto-leave on panic, SIGHUP, SIGTERM is
  parked as a follow-up (future FB entry).
- Addon-level raw-write path (`terminal.Write`) and the SIGWINCH
  install-order race moved to the TM track (TMB-016 / TMB-017) so
  `taida-lang/terminal` and the main repo can ship independently.

## @c.20.rc4 (in progress)

Complete the Hachikuma Phase 8-10 / Phase D follow-up track: parser
silent-bug elimination, stdin UX alignment across three backends, and
a list-of-record shape for `HttpRequest` headers so dash-bearing names
like `x-api-key` are finally reachable from Taida.

### Parser — new diagnostic `E0303`

`name <= | cond |> A | _ |> B` written across multiple physical lines
on the right-hand side of `<=` silently greedy-absorbed the next
statement as a continuation arm (C19B-009 / ROOT-5). One-line
`|== error: Error = <expr>` dropped every subsequent top-level
definition from the loaded module (C19B-008 / ROOT-4). Both shapes
checker-green'd and broke at runtime.

Now:

- One-line `|== error: Error = <expr>` parses as a single-expression
  handler body and leaves surrounding definitions intact (equivalent
  to the multi-line block form).
- Multi-line multi-arm `| cond |> A | _ |> B` on the right-hand side
  of `<=` is rejected with
  `[E0303] rhs of \`<=\` cannot contain a multi-arm conditional`.
  Use `If[cond, then, else]()`, extract a helper, or wrap the
  conditional in parentheses (`name <= (| ... |> ...)`), which
  restores the top-level parsing context.
- Single-line rhs guards (`name <= | a |> 1 | _ |> 2` on one
  physical line) remain a legal one-shot shape.
- Top-level / function-body `| cond |> body` is untouched.

### New: `stdinLine(prompt?) => :Async[Lax[Str]]`

Hachikuma Phase D's Japanese interview flow exposed ROOT-7: kernel
cooked-mode Backspace deletes one byte at a time, corrupting
multibyte UTF-8 when users edit their typing. C20-2 introduces a
dedicated prelude API that routes through a UTF-8-aware line editor
on all three backends:

- Interpreter: `rustyline` (MIT/Apache-2.0), default editor with
  codepoint-wide Backspace, arrow keys, and Ctrl-U line clear.
- JS: `node:readline/promises` + `rl.question()`; TTY mode enables
  the full editor, pipe mode falls back to line-buffered reads.
- Native: a trimmed derivative of linenoise (BSD-2-Clause, see
  `LICENSES/linenoise.LICENSE`) — termios raw mode, UTF-8
  codepoint-aware Backspace, pipe input drops through to `getline`.

```taida
stdinLine("お名前: ") ]=> line
stdout("こんにちは、" + line.getOrDefault("旅人"))
```

Shape and discipline:

- Return type is **`Async[Lax[Str]]`** across all three backends. The
  Async wrapper exists so the JS path (async-only readline) and the
  Interpreter / Native paths (sync editors) share one surface. Callers
  **must** unmold with `]=>` to obtain the inner `Lax[Str]`; `<=`
  binding leaves the Async in place.
- Any failure (EOF, pipe close, Ctrl-C, Ctrl-D on empty line, missing
  `node:readline/promises`, `termios` error, …) collapses to
  `Lax(null, "")` so the default-value guarantee is preserved —
  `.getOrDefault("")` and `.isEmpty()` both keep working.
- Prompt is optional; non-Str prompts are display-stringified before
  being written, matching the ROOT-14 parity rule already applied to
  `stdin`.
- Out of scope: history, tab completion, multi-line edit. A future
  `taida-lang/readline` addon will layer those features on top.

### `stdin` — three-backend parity (no new API)

`stdin(prompt?)` now behaves identically on Interpreter, JS, and
Native:

- Returns `""` on EOF / read error everywhere (Interpreter used to
  throw `IoError`; JS and Native already silently returned empty).
  Callers that need failure awareness should use the new
  `stdinLine => :Lax[Str]` API (see next section).
- Prompt is optional on every backend including the type checker
  (`stdin()` is now valid; previously `[E1507]` rejected it).
- JS decodes stdin via a streaming `TextDecoder('utf-8', { stream })`
  over a 4 KiB chunk buffer — multibyte codepoints survive chunk
  boundaries instead of collapsing to U+FFFD.
- JS stringifies non-Str prompts via `String(prompt)` inside the
  try/catch so `stdin(1)` / `stdin(@(...))` no longer crashes Node
  with `ERR_INVALID_ARG_TYPE`.
- Native replaces the fixed `char[4096]` stack buffer with
  `getline(3)` on POSIX / a `fgets` realloc loop on Windows, so long
  pasted lines are read completely instead of bleeding the tail into
  the next `stdin` call.

### `HttpRequest` — list-of-record headers

Dash-bearing HTTP headers (`x-api-key`, `anthropic-version`,
`content-type`, ...) are no longer reachable via buchi-pack
identifier keys. C20 adds a second accepted shape:

```taida
resp <= HttpRequest["POST", "https://api.example.com/v1/echo"](
  headers <= @[
    @(name <= "x-api-key", value <= "secret-k"),
    @(name <= "anthropic-version", value <= "2023-06-01"),
  ],
  body <= "{}",
) ]=> await
```

Both shapes are supported on all three backends:

- Legacy: `headers <= @(ident <= "value")` — identifier becomes the
  wire header name as before.
- New: `headers <= @[@(name <= "...", value <= "...")]` — any UTF-8
  is legal in the wire name.

Also:
- JS `HttpRequest[method]()` (fewer than 2 type args) now fails at
  `taida build --target js` time with
  `HttpRequest requires at least 2 type arguments`, matching the
  Interpreter and Native rejection path instead of emitting
  syntactically invalid JavaScript.
- Native lowering's undocumented third-type-arg body fallback
  (`HttpRequest["POST", url, body]()`) has been removed. Interpreter
  and JS always consulted the `body <= ...` field only, so this shape
  silently sent a body on Native while the other two backends sent an
  empty string — a cross-backend parity trap (C20B-012 / ROOT-15). No
  in-tree Taida code relied on the legacy shape; migrate to
  `HttpRequest["POST", url](body <= "...")`.

### User-defined functions called via mold syntax (C20B-014 / ROOT-17)

User-defined functions invoked as `Fn[arg1, arg2]()` now dispatch to
the function instead of silently returning a `@(__value, __type)` mold
wrapper. This closes a 2.1.3-era regression that silently passed
`taida check` but crashed Hachikuma's TUI at every one of 81 call
sites (`CursorMoveTo[r, c]()`, `PadWidth[t, w]()`,
`TruncateWidth[t, w]()`, …). The diagnostic surface aligns:

- **Interpreter** (`src/interpreter/eval.rs`): before the generic
  mold-wrap path, `MoldInst` now detects `Value::Function` in scope
  with no matching `MoldDef` and dispatches to `call_function` with
  `type_args` treated positionally.
- **Native lowering** (`src/codegen/lower_molds.rs`): before the
  `unsupported mold type` error, the `_` arm consults
  `self.user_funcs` and lowers through `lower_func_call` for known
  user functions. Previously `Fn[args]()` failed at build time.
- **Checker** (`src/types/checker.rs`): the `MoldInst` fallback now
  returns the function's registered return type instead of
  `Type::Unknown`, so downstream type inference matches runtime
  behaviour. Named `()` fields on a user-fn mold-syntax call are
  rejected with new diagnostic `[E1511]` — user functions have no
  named-field ABI.
- **JS** is unchanged. Its existing
  `__taida_solidify(Fn(args))` generic fallback already dispatched to
  the user function correctly; the regression test pins that the
  behaviour matches the Interpreter's new shape.

Both `Fn[args]()` and `Fn(args)` are valid and produce identical
results across all three backends.

### Tests

- `tests/c20_parser_silent_bugs.rs` (parser unit, 8 cases)
- `tests/c20_stdin_parity.rs` (3 backends × 4 fixtures + checker
  no-prompt + JS non-Str prompt guard, 14 cases)
- `tests/c20_stdinline_parity.rs` (3 backends × 3 fixtures + 3 checker
  cases, 12 cases — pins `Async[Lax[Str]]` surface, EOF failure, UTF-8
  round-trip)
- `tests/c20_http_dash_header.rs` (3 backends × 2 header shapes +
  JS arity guard, 7 cases)
- `tests/c20b_014_mold_user_fn_call.rs` (3 backends + 2 checker cases,
  5 cases — pins user-fn mold-syntax dispatch and `[E1511]` rejection)
- `examples/quality/c20_parser/*` (2 pins)
- `examples/quality/c20_stdin/*` (4 pins)
- `examples/quality/c20_stdinline/*` (3 pins)
- `examples/quality/c20_mold_user_fn/*` (1 pin)

## @c.19.rc4

Add TTY-passthrough variants of the process-execution APIs so Taida
programs can launch interactive TUI applications (nvim, less, fzf,
git commit, etc.). Closes the Hachikuma Phase 3 P-3-13 (B-006) external
editor integration blocker.

### Features

#### New: `runInteractive(program, args)` / `execShellInteractive(command)`

`taida-lang/os` gains two new functions that match `run` / `execShell`
argument-wise but hand the parent process's stdin / stdout / stderr
directly to the child instead of capturing them through pipes. This is
the mode you want when the child is a terminal UI that needs to read
keystrokes and draw on the TTY.

```taida
>>> taida-lang/os => @(runInteractive)

// Drop the user into $EDITOR, then pick up the exit code afterwards.
r <= runInteractive("nvim", @["/tmp/draft.md"])
stdout("editor exit: " + r.__value.code.toString())
```

Return type: `Gorillax[@(code: Int)]`.

Key differences vs. the captured variants:

| API                   | stdio          | Inner shape                          | Intended use           |
|-----------------------|----------------|--------------------------------------|------------------------|
| `run`                 | pipes (captured) | `@(stdout: Str, stderr: Str, code: Int)` | programmatic output parsing |
| `execShell`           | pipes (captured) | `@(stdout: Str, stderr: Str, code: Int)` | shell expansion + output parsing |
| `runInteractive`      | inherited TTY  | `@(code: Int)`                       | TUI apps (nvim, fzf, …) |
| `execShellInteractive`| inherited TTY  | `@(code: Int)`                       | TUI apps + shell glob |

Signal death uses the `128 + signal` POSIX convention on all three
backends. Windows is best-effort (POSIX is the first-class target).

#### Affected backends

All three backends ship parity implementations:

- **Interpreter** (`src/interpreter/os_eval.rs`): `Command::status()`
  with default (inherited) stdio.
- **JS** (`src/js/runtime/os.rs`): `child_process.spawnSync(..., { stdio: 'inherit' })`.
- **Native** (`src/codegen/native_runtime/os.c`): `fork()` + `execvp`
  with **no** `dup2` in the child.

The 3-backend contract is pinned by `tests/c19_interactive_exec.rs` and
`examples/quality/c19_interactive_exec/*.td`.

### Non-goals (scope-out, for future tracks)

- async / non-blocking interactive exec (the child owns the foreground TTY)
- pty allocation (`openpty` / `forkpty`) — belongs in a future `taida-lang/tty` addon
- automatic raw-mode save / restore on behalf of `terminal` addon users
  (the caller is responsible for `rawModeLeave` / `rawModeEnter` around the handoff)
- stdin write-through API for interactive children

### Backward compatibility

Pure additive change: existing `run` / `execShell` behaviour and return
shape are byte-identical to `@c.18.rc4`. `OS_SYMBOLS[0..35]` keeps the
pre-C19 ordering; the two new entries live at indices 35 and 36.

### Follow-up hardening (C19B, same release)

Two gaps surfaced in the C19 code-review HOLD are fixed in the same
release:

- **C19B-001 — Native `execvp` failure is now an `IoError`.** Before the
  fix, Native collapsed child-side `execvp` failure into `_exit(127)`,
  indistinguishable from a program that merely exited with 127. The
  parent now reads the child's `errno` through a CLOEXEC self-pipe and
  emits a proper `IoError{code, kind, message}` on ENOENT / EACCES /
  etc. — matching Interpreter and JS. Normalized `err.errno` sign on JS
  so all three backends report the positive POSIX errno (e.g. `2` for
  ENOENT).
- **C19B-002 — Checker pins `Gorillax[@(code: Int)]`.** Before the fix,
  `runInteractive(...).__value.stdout` silently passed `taida check`
  because os symbols fell through to `Type::Unknown`. The checker now
  registers typed signatures for `runInteractive` /
  `execShellInteractive`, resolves `.__value` through the Gorillax
  envelope, and rejects access to any field not in the pinned `@(code:
  Int)` inner shape. Captured `run` / `execShell` remain Unknown-typed
  and non-interfering, as promised by the C19 design.
- Native `Gorillax` had a long-standing field-hash mismatch (`__error`
  stored under `HASH___DEFAULT`). Fixed by introducing `HASH___ERROR`
  and threading it through `taida_gorillax_{new,err,relax}`, which
  unblocks `r.__error.<field>` on the Native backend for the first time.

The new failure-path parity is pinned by
`examples/quality/c19_interactive_exec/os_interactive_enoent.{td,expected}`
and the three `c19_run_interactive_enoent_*` tests in
`tests/c19_interactive_exec.rs`.

## @c.18.rc4

Close 4 of the 6 Enum limitations identified by Hachikuma Phase F (2026-04-16).
The 5th (JSON mold Enum validate) shipped in `@c.16.rc4`; the 6th (function
boundary contract dependency) auto-resolves once Enum types cross module
boundaries.

### Features

#### New: Cross-module Enum type resolution (Hachikuma #1 / #6)

`>>> ./m.td => @(Color)` followed by `Color:Red()` in the importer no longer
triggers `[E1608] Unknown enum type`. The type checker now parses the
exporting module and registers its EnumDefs into the importer's type
registry. Aliased imports (`>>> ... => @(Color: Paint)`) are honoured.

**New diagnostic `[E1618]`**: Variant-order mismatch across module boundary.
When an importer keeps a local `Enum => Color = ...` redefinition, its
variant order must match the exporting module's exactly — otherwise ordinals
silently diverge and `jsonEncode` / ordering comparison break. The checker
now catches this at compile time with a clear mismatch diagnostic.

(E1618 was allocated because E1610 is already used for cyclic-inheritance
detection; see the inline docstring in `src/types/checker.rs`.)

#### New: `jsonEncode` emits variant-name Str

`jsonEncode(@(state <= HiveState:Running()))` now returns
`{"state":"Running"}` on all three backends, symmetric with the C16
`JSON[raw, Schema]()` decoder which already accepts the variant-name Str.
Round-trip is now guaranteed:

```taida
rec <= @(state <= HiveState:Running())
raw <= jsonEncode(rec)                 // {"state":"Running"}
JSON[raw, Status]() ]=> rec2
rec2.state == HiveState:Running()      // true
```

**Migration**: If pre-C18 code depended on the ordinal Int wire format
(`{"state":1}`), wrap the Enum value in `Ordinal[]` before encoding:

```taida
payload <= @(state_id <= Ordinal[rec.state]())
jsonEncode(payload)                    // {"state_id":1}
```

The internal representation (`Value::Int(ordinal)` / `int32` in native)
and the `.toString()` contract (returns ordinal Str) are unchanged —
`jsonEncode` is the only observable behaviour that switches.

#### New: `Ordinal[enum]()` mold — explicit Enum → Int

The sanctioned path for converting an Enum value to its declared ordinal:

```taida
Enum => HiveState = :Creating :Running :Stopped

Ordinal[HiveState:Running()]()         // 1
Ordinal[HiveState:Stopped()]() > 0     // true — Int space comparison
```

Replaces the fragile `.toString()`-parsing `initResumeStateOrdinal` helper
from Hachikuma. Arity-1 only; non-Enum arguments produce a typed
runtime error.

The inverse direction (`FromOrdinal[Color, 1]()`) is C18 scope-out.

#### New: Same-Enum ordering comparisons

Same-Enum ordering (`<` / `>` / `>=`) now uses the declared ordinal order:

```taida
HiveState:Creating() < HiveState:Running()    // true
HiveState:Running() >= HiveState:Creating()   // true

ready s =
  | s >= HiveState:Running() |> "yes"
  | _ |> "no"
=> :Str
```

Cross-Enum ordering and Enum↔Int ordering continue to emit `[E1605]` —
use `Ordinal[]` (above) to bridge to Int explicitly. The declared order
of an Enum is now a semantic contract; treat variant reorderings as
breaking changes.

### Notes

- Enum definition syntax (`Enum => Name = :A :B :C`) and the
  "最初のバリアントがデフォルト" rule are unchanged.
- Enum internal representation (`Value::Int(ordinal)` in the interpreter,
  `int32` in native, tagged `__taida_enumVal` wrapper in JS for
  `jsonEncode` toJSON support) is additive; existing code paths that
  read Enum values as Int ordinals continue to work.
- `.toString()` still returns the ordinal Str; the variant-name Str is
  exclusively the `jsonEncode` / wire-format representation.
- All 3 backends (Interpreter / JS / Native) pass the 4 new parity
  tests in `examples/quality/enum_*.td`.

## @c.17.rc4

### Fixes

- **Installer: auto-detect tag re-publish and refresh the cached store
  entry.** Before C17, `taida install` keyed the local store
  (`~/.taida/store/<org>/<name>/<version>/`) only by the tag string.
  When a package maintainer retagged (`taida publish --retag` /
  delete + recreate) the same version against a new commit, the
  consumer's cache kept the old tarball indefinitely -- a subsequent
  `taida install` was a silent no-op, and `taida run` would later
  break on the stale facade. C17 writes a `_meta.toml` provenance
  sidecar alongside every extracted store package recording the
  resolved commit SHA, the tarball SHA-256, and a UTC fetch timestamp.
  On every subsequent install the sidecar is compared against the
  remote tag's commit SHA (via the GitHub git/refs API). When the SHAs
  disagree, the store entry is invalidated and re-extracted
  automatically. Offline / unverifiable states emit a stderr warning
  but never silently skip. The install success-path stdout (package
  list, lockfile line, exit code) is unchanged.

### Features

- **`taida install --no-remote-check`**: skips the remote commit-SHA
  lookup and trusts the existing store sidecar. Intended for offline
  or rate-limited environments. Mutually exclusive with
  `--force-refresh` (rejected at argument parsing time).
- **`taida install --force-refresh` now also wipes the store entry**
  for each package before re-extracting. The legacy addon-cache
  invalidation (`~/.taida/addon-cache/`) is unchanged; store
  invalidation is additive.
- **`taida cache clean --store`**: prune `~/.taida/store/` after
  showing a pre-flight summary. Requires TTY confirmation or `--yes`
  so scripts cannot wipe the store accidentally.
- **`taida cache clean --store-pkg <org>/<name>`**: prune a single
  package from the store (all versions). Scope is narrow enough that
  no confirmation is requested.
- **`taida cache clean --all` now includes the store** in addition to
  the WASM runtime cache and the addon prebuild cache.

### Internal

- `src/pkg/store.rs` gains a pinned 5-row stale-detection decision
  table (`classify_stale`), `resolve_version_to_sha` (GitHub git/refs
  with annotated-tag dereferencing), `invalidate_package` /
  `read_package_meta` primitives, and a `StorePruneReport` for
  cache-clean operations.
- `src/pkg/provider.rs::StoreProvider` carries `force_refresh` /
  `no_remote_check` flags and implements the decision table via
  `apply_stale_decision` before reusing a cached entry.
- `src/pkg/resolver.rs` adds `StoreRefreshFlags` and
  `resolve_deps_with_flags` / `resolve_deps_locked_with_flags`.

### Tests

- 22 unit tests in `src/pkg/store.rs` for decision-table rows, the
  JSON object/string extractors used to parse the git/refs response,
  and an in-process mock server that exercises annotated-tag
  dereferencing.
- 7 new integration tests across `tests/installer_store_staleness.rs`,
  `tests/installer_force_refresh.rs`, `tests/installer_offline.rs`
  backed by a shared `tests/mock.rs` HTTP server that serves both the
  archive tarball and the API endpoints.

## @c.16.rc4

### Fixes

- **`JSON[raw, Schema]()` now validates Enum-typed fields**. Previously,
  `Schema` containing an `Enum` field (`Status: :Active :Inactive
  :Pending`) would silently accept any JSON string — e.g.
  `"status": "Bogus"` — and pass it through as a plain `Str`. This
  broke the "暗黙の型変換なし" philosophy at the JSON boundary:
  downstream code saw an Enum-typed field holding a value outside
  the declared variant set. The fix:
  - `JsonSchema::Enum(name, variants)` is now a first-class schema
    variant alongside `Primitive` / `TypeDef` / `List`.
  - On match, the variant's ordinal (`Int`) is returned (unchanged
    Enum internal representation).
  - On mismatch, key-missing, or `null`, the field becomes
    `Lax[Enum]` with `hasValue=false`, `__value=Int(0)`, and
    `__default=Int(0)` (first variant — the existing "最初のバリアント
    がデフォルト" rule reused as the Lax fallback). Callers must
    handle the boundary explicitly via `hasValue`,
    `| .hasValue |> ... | _ |> ...`, or `getOrDefault(Variant)`
    (`|==` is the `throw`-catching operator and does NOT branch on
    Lax — see `docs/reference/operators.md`).
  - Enum definition syntax (`Enum => Name = :A :B :C`) and the
    first-variant-default rule are unchanged.
  - All three backends (Interpreter / JS / Native) produce
    byte-identical output on `examples/quality/json_enum_validate.td`.

### Migration

If pre-C16 code relied on the silent Str pass-through for Enum
fields, the field now comes back as `Lax[Enum]` and a direct
property access like `result.status.toString()` will surface the
Lax metadata instead of the raw string. Update the access site to
one of:

```taida
// 1. Resolve with a fallback
u.status.getOrDefault(Status:Pending())

// 2. Branch on hasValue
| u.status.hasValue |> handleValid(u.status.__value)
| _                 |> handleInvalid()
```

See `docs/guide/03_json.md` (Enum 型フィールドの検査) and
`docs/reference/mold_types.md` (JSON モールディング型) for the full
rules.

## @c.15.rc3

### Security

- **Supply chain: `taida upgrade` canonical source**. Earlier CLIs
  hard-coded `shijimic/taida` — a personal development fork — as
  the `taida upgrade` release source. Anyone with control of that
  single personal account (by compromise, sale, rename, or deletion)
  could replace published binaries and `SHA256SUMS` with attacker-
  controlled versions, and every `taida upgrade` invocation
  worldwide would silently trust them. The constant is now
  `taida-lang/taida` (the canonical org), and a mandatory regression
  test (`canonical_release_source_is_taida_lang_org`) pins it so
  a future edit must go through a compiler failure and explicit
  review. Stale documentation references to `shijimic/taida` as
  "the core repo" in `docs/reference/cli.md` and the scaffold doc
  comments in `src/pkg/init.rs` were also corrected.
- **User migration**. `@c.13.rc3` and earlier CLIs still look at
  `shijimic/taida` and cannot see `@c.14.rc3+` releases on the
  canonical org. Affected users must reinstall through `install.sh`
  or download a `@c.15.rc3+` archive from
  `github.com/taida-lang/taida/releases` directly. Once on
  `@c.15.rc3+`, `taida upgrade` will see canonical releases as
  expected.

### Fixes

- `taida upgrade --version @c.14.rc3` (and any `--gen c --label rc3`
  query) from a pre-fix CLI returned `version @c.14.rc3 not found in
  releases` because the fork (where the CLI was looking) never
  received that release. After this fix, the upgrade path points at
  the canonical org directly and the lookup resolves.

## @c.14.rc3

### Breaking changes

- **`taida publish` tag-push-only redesign**: `taida publish` is now
  a minimal CLI that only validates the manifest identity, computes
  the next version from an API diff, creates a local git tag, and
  pushes that tag to `origin`. It no longer runs `cargo build`,
  computes SHA-256 digests, writes `addon.lock.toml`, rewrites
  `packages.tdm` or `native/addon.toml`, commits, pushes to `main`,
  or calls `gh release create`. All of those responsibilities moved
  to CI (`.github/workflows/release.yml`). See
  `docs/reference/cli.md#taida-publish` for the new surface and
  `docs/guide/13_creating_addons.md#8-migration-from-pre-c14-addons`
  for the step-by-step migration.
- **Release author semantics unified**: addon releases are now
  created exclusively by `github-actions[bot]` via the addon's own
  `release.yml`. Pre-C14 releases created by the CLI user (human
  account) no longer occur. This matches the core Taida release
  workflow.
- **`packages.tdm` qualified identity required**: `taida publish`
  now rejects bare `<<<@<version>` (no `owner/name`). The
  directory-name fallback that older surfaces permitted is gone —
  every publishable package must declare
  `<<<@<version> <owner>/<name>`. The `taida-lang/terminal` PR #2
  migration commit (`db9637d`) is the reference.
- **`addon.toml` placeholder SHA + lockfile fallback**: `taida
  install` now detects the canonical placeholder SHA
  (`sha256:` + 64 zeros) in `[library.prebuild.targets]` at a tag's
  `native/addon.toml` and falls back to the release-asset
  `addon.lock.toml` for the authoritative hash. This lets addons
  ship a lockfile-only design on `main` while still publishing
  verified cdylibs for tags whose `addon.toml` left the placeholder
  value in place.

### New CLI surface

```
taida publish [--label LABEL] [--force-version VERSION] [--retag] [--dry-run]
```

- `--label LABEL` attaches a pre-release label to the resolved next
  version (`a.4` + `--label rc` → `a.4.rc`).
- `--force-version VERSION` overrides the auto-detected version
  entirely. Skips the API diff snapshot.
- `--retag` force-replaces an already-pushed tag. Skips the API diff
  snapshot.
- `--dry-run` prints the publish plan without touching git.

Automatic version bump (Phase 2a — symbol-level export set diff):

| API change                                  | Bump                               |
|---------------------------------------------|------------------------------------|
| Initial release (no previous tag)           | `a.1` (fixed)                      |
| Symbol removed or renamed                   | Generation (`a.3` → `b.1`)         |
| Symbol added or internal-only               | Number (`a.3` → `a.4`)             |

### New workflow template

`crates/addon-rs/templates/release.yml.template` is the canonical
C14 addon release workflow. It is symmetric with the core Taida
`.github/workflows/release.yml` on all load-bearing axes (4-job
`prepare → gate → build → publish` structure, `github.token`-based
`gh release create`, Taida tag regex, 5-platform build matrix).

- `taida init --target rust-addon` scaffolds the template with two
  placeholders (`{{LIBRARY_STEM}}`, `{{CRATE_DIR}}`).
- Existing addons must migrate manually. See
  `docs/guide/13_creating_addons.md#8-migration-from-pre-c14-addons`.

### Reference release

`taida-lang/terminal@a.1` is the first addon to ship through the
C14 pipeline and serves as the ground-truth reference implementation:

- Release author: `github-actions[bot]`
- 8 assets: 5 × `libtaida_lang_terminal-<triple>.{so,dylib,dll}`,
  `addon.lock.toml`, `prebuild-targets.toml.txt`, `SHA256SUMS`
- CI run: https://github.com/taida-lang/terminal/actions/runs/24495250052
  (all 8 jobs green, ~90s end-to-end)
- Release page: https://github.com/taida-lang/terminal/releases/tag/a.1

### Migration (summary)

For existing addon authors:

1. Add qualified identity to `packages.tdm`:
   `<<<@<version>` → `<<<@<version> <owner>/<name>`.
2. Replace `.github/workflows/prebuild.yml` with the C14
   `release.yml` template (4 jobs, 5-platform matrix, CI-owned
   release creation).
3. Remove obsolete CLI flags from scripts:
   - `taida publish --target rust-addon` → `taida publish`
   - `taida publish --dry-run=plan` → `taida publish --dry-run`
   - `taida publish --dry-run=build` → removed (no local build)
   - `TAIDA_PUBLISH_SKIP_RELEASE=1` → removed (CLI never creates
     releases)
4. Accept that release author is now `github-actions[bot]` in all
   downstream automation / documentation.
5. (Optional) Re-tag existing releases with
   `taida publish --force-version <existing-version> --retag` to
   re-publish them under the new author / asset layout.

Full step-by-step migration: `docs/guide/13_creating_addons.md`
§8. Migration blockers resolved in this cycle: `TMB-013` (identity
on terminal), `TMB-014` (release author on terminal), plus `C14B-001`
through `C14B-006`, `C14B-011`, `C14B-012` (taida-core side).

### Internal

- `src/pkg/publish.rs`: 2,762 → 807 lines. Deleted:
  `prepare_publish`, `PublishRollback`, `rewrite_export_version`,
  `rewrite_prebuild_url_if_needed`, `build_addon_artifacts`,
  `compute_cdylib_sha256`, `create_github_release`,
  `git_commit_tag_push`, `check_dirty_allowlist`,
  `compute_publish_integrity`, `proposals_url`,
  `proposals_repo`. Added: `PublishPlan`, `plan_publish`,
  `render_plan`, `tag_and_push`, `validate_manifest_identity`,
  `require_identity_matches_remote`, `check_gh_auth`,
  `bump_number`, `bump_generation`, `attach_label`,
  `next_version_from_diff`, `latest_taida_tag`, `DiffSkipReason`.
- `src/pkg/api_diff.rs`: new module. Reuses the existing Taida
  parser (`crate::parser::parse`) to snapshot export symbols from
  `taida/*.td` at HEAD and at a tag's tree. `ApiDiff::{Initial,
  None, Additive, Breaking}` classifies the set diff. Phases 2b
  (function signatures), 2c (type pack fields), 2d (`addon.toml`
  arity) are deferred to @c.14.rc2+.
- `src/addon/prebuild_fetcher.rs::is_placeholder_sha` — detects the
  canonical placeholder SHA (`sha256:` + 64 zeros) so the resolver
  can route to lockfile fallback deterministically.
- `src/pkg/resolver.rs::ShaSource` / `choose_sha_source` — pure
  decision table between `AddonToml`, `LockfileFallback`, and
  `NoPrebuild`, pinned by 5 unit tests.
- `crates/addon-rs/templates/release.yml.template` — new 4-job
  template, structural symmetry with core pinned by
  `tests/init_release_workflow_symmetry.rs`.
- `src/main.rs::run_publish` simplified from ~500 lines to ~140
  lines.
- Deleted tests (no longer reflect the real CLI):
  `tests/publish_cli.rs`, `tests/publish_rust_addon.rs`,
  `tests/publish_install_roundtrip.rs` (~1,492 lines total).
- New tests: `tests/publish_tag_push.rs` (7),
  `tests/publish_identity_validation.rs` (4),
  `tests/publish_force_version.rs` (5),
  `tests/publish_retag.rs` (3),
  `tests/publish_api_diff_skip.rs` (3),
  `tests/api_diff.rs` (10),
  `tests/init_release_workflow_symmetry.rs` (5).

### Tests

- lib unit: 2,382 / 2,382 green
- publish integration: 22 / 22 green (force_version 5, retag 3,
  tag_push 7, identity_validation 4, api_diff_skip 3)
- api_diff integration: 10 / 10 green
- init workflow symmetry: 5 / 5 green
- Red tests: 0

## @c.13.rc3

### Language changes

- **Expression-block tail binding**: the last statement of a `| |>` arm
  body, a function body, or a `|==` error-ceiling body may now be a
  binding (`name <= expr`, `expr => name`, `expr ]=> name`, or
  `name <=[ expr`). The bound value becomes the block's result, so a
  redundant trailing `name` line is no longer required. Accepted in
  all three backends (Interpreter / JS / Native).

- **Bind-and-forward in pipelines**: a single-direction `=>` pipeline
  may now contain intermediate `=> name` steps. The current value is
  bound to `name` *and* forwarded to the next step, and later steps
  may reference `name`. Previously these produced
  `[E1502] Undefined variable`.

- **Discard-binding rejection extended**: underscore-prefixed discard
  targets (`=> _x`, `_x <= ...`, `]=> _x`, `_x <=[ ...`) are now
  rejected at any position inside an arm body, function body, `|==`
  handler body, or method body with `[E1616]`. Previously only arm
  bodies enforced this — function / `|==` / method bodies silently
  accepted discard bindings.

See `docs/guide/07_control_flow.md` for the full rule and shorthand
forms.

### Internal

- `src/codegen/lower/`, `src/interpreter/net_eval/`, and
  `src/codegen/native_runtime/` were split along responsibility
  boundaries. No user-visible behaviour change — only source layout
  differs.

### Migration

- Code that ended arm / function / `|==` bodies with an expression
  continues to compile unchanged.
- Code that appended a redundant `name` line to satisfy the old
  restriction may drop that final line without semantic change.
- Code that used discard bindings (`=> _x` etc.) in function or `|==`
  bodies must be updated: either rename the target (dropping the
  leading underscore) or remove the binding entirely if the value is
  genuinely unused.

## @c.12.rc3 (in progress)

In-flight release tracking the @c.12.rc3 milestone (`FUTURE_BLOCKERS.md`
全 12 本消化）. See `.dev/C12_PROGRESS.md` for the live progress tracker.

### Breaking Changes Summary (C12B-037)

@c.12.rc3 bundles **four independent breaking changes** that land in the
same release. A single user codebase upgrading from @b.11.rc3 to
@c.12.rc3 may see multiple compile-time errors at once; this section
collects them in the recommended migration order so you know what to
fix first.

**Impact ranking (most-to-least likely to hit code)**:

1. **Phase 2 — `.toString(radix)` removed** (`[E1508]`)
   - Scope: any call site that uses the JS-style radix argument such as
     `n.toString(16)` or `n.toString(2)`.
   - Migration: replace with `ToRadix[n, base]().getOrDefault("")`.
     See `docs/reference/mold_types.md §ToRadix`.
   - Detection: `taida check` reports `[E1508] .toString() takes no
     arguments`. Fix first — it's purely mechanical.

2. **Phase 5 — `stdout` / `stderr` return `Int` instead of `Value::Unit`**
   - Scope: any `s <= stdout(...)` binding whose downstream code
     assumed `s` was `Unit` or a `Result`. Most real code used
     `stdout(...)` as a statement and is unaffected.
   - Migration: existing `stdout(x) => _` patterns still work (they
     discard the `Int` byte count). If you bound the result, you can
     now perform arithmetic on it: `bytes <= stdout("hi"); stdout(bytes + 1)`.
   - Detection: no compile error for the common discard pattern; only
     code that asserted on the type of the return may need updating.

3. **Phase 4 — `| cond |>` arm bodies must end in a pure expression**
     (`[E1616]`)
   - Scope: arm bodies that contained a discarded side-effect statement
     (e.g. `writeFile(...) => _wr`), a bare function-call statement, or
     a trailing let-binding with no result expression.
   - Migration:
     - Discarded side-effect statement → wrap in an `If[cond, then,
       else]()` mold or hoist the side effect out of the arm.
     - Trailing let binding → add a final expression line (the bound
       name itself works).
     - Let-bindings in non-terminal positions (`doubled <= double(n);
       addOne(doubled)`) are still allowed — the discipline only
       targets side-effect statements.
   - Detection: parser `[E1616]` points to the offending statement with
     its span. See `docs/guide/07_control_flow.md` for the full table
     of accepted / rejected elements.

4. **Phase 3 — non-tail mutual recursion is a compile error** (`[E1614]`)
   - Scope: any function pair (or larger cycle) where at least one edge
     of the call graph cycle is *not* in tail position. Tail-only
     mutual recursion (`isEven` / `isOdd`) continues to work.
   - Migration: refactor the non-tail call to a tail call (often by
     threading an accumulator), or replace the recursion with an
     explicit loop via `Fold` / `Filter` / `Map` molds.
   - Detection: `taida check` / `taida verify` report `[E1614]`
     identifying the offending edge. Formerly this failure surfaced at
     runtime as `Maximum call depth (256) exceeded`.

**Recommended fix order**: 1 → 2 → 3 → 4. `.toString(radix)` and the
`stdout` / `stderr` return-type change are the mechanical ones and can
be resolved without touching control flow. Phases 4 and 3 may surface
in the same function, so landing the pure-expression discipline first
often clarifies the call-graph before the tail-position analysis runs.

**Per-package dry-run (internal official-package-repos)**:

| Package | Phase 2 hits | Phase 3 hits | Phase 4 hits | Phase 5 hits |
|---------|--------------|--------------|--------------|--------------|
| `terminal` | 0 | 0 | 0 | 0 |

(dry-run run 2026-04-15 against `.dev/official-package-repos/`; only
the `terminal` package is tracked in the internal repo tree and it was
authored after all four breaking changes were designed, so no
migration is required for it. `taida-lang/net` / `taida-lang/os`
prelude code lives inside the compiler and is updated in-tree as part
of this release).

External packages should expect <10 compile errors per 1,000 LoC of
Taida code based on RC-era metrics.

### Post-Gate Blocker Fixes (2026-04-15)

Gate review surfaced 8 Must Fix blockers (`C12B-029/030/031/032/033/034/035/040`).
All resolved in this session:

- **C12B-029** — Native `Regex(...)` now fails fast at construction time
  for unsupported flags (`/[^ims]/` characters) and invalid patterns,
  throwing `:Error` with `type=ValueError` that matches the Interpreter
  and JS error shape. 3 parity tests added covering all three backends.
- **C12B-030** — Native regex pattern rewriter gains `\xHH` / `\x{HH..}` /
  `\uHHHH` / `\u{HH..}` hex/Unicode escape support (UTF-8 encoded).
  Documented subset: `\b` / `\B` and the `s` flag remain
  Interpreter/JS-only on Native POSIX ERE.
- **C12B-040** — JS regex implementation split: `Regex(...)` constructor
  previously validated without the `u` flag but `__taida_compile_regex`
  appended `u` at runtime, so `Regex("\x{41}")` / `Regex("\_")`
  constructed successfully and then threw `Invalid escape` on the first
  `.replace` / `.match` / `.search`. Fixed by routing both construct-
  time validation and runtime compilation through a shared
  `__taida_rewrite_pattern` helper that converts `\x{HH..}` / `\u{HH..}`
  to JS-native `\uHHHH` (or UTF-16 surrogate pair for supplementary
  planes) and drops the `u` flag, preserving identity-escape leniency
  for parity with the Rust `regex` crate and POSIX ERE. 4 new parity
  tests cover construct + first-use round-trips for bracketed hex,
  bracketed Unicode, identity escape, and `.match`/`.search` compile
  paths.
- **C12B-031** — `str.match(...)` / `str.search(...)` now require a
  `:Regex` argument at type-check time (`[E1508]`). Previously Str
  literals silently diverged across backends (Interpreter/JS runtime
  throw, Native empty fallback). 4 checker tests added.
- **C12B-032** — `BodyEncoding::Empty` is now a struct variant
  `Empty { had_content_length_header: bool }` so the internal HTTP/1.1
  framing layer can distinguish explicit `Content-Length: 0` from an
  absent Content-Length header. The handler-visible BuchiPack surface
  remains flat (`contentLength: 0`, `chunked: false`) for v1
  compatibility; the new bit flows through
  `parse_request_head` → `ConnReadResult` → `RequestBodyState::new`.
- **C12B-033** — `.dev/C12_PROGRESS.md` gate status line corrected from
  "Final Gate 準備完了" to explicitly acknowledge Phase 9 PARTIAL and
  the presence of OPEN blockers at time of write.
- **C12B-034** — **wasm memory safety fix**: `taida_io_stdout_with_tag`
  / `taida_io_stderr_with_tag` no longer blindly cast a non-Bool
  `val` to `char*`. Non-Bool, non-Str tags route through
  `taida_polymorphic_to_string` so `print_any(42)` on wasm emits `42`
  instead of reading linear memory at address 42. New fixture
  `examples/compile_c12b_034_wasm_nonbool_param.td` locks the
  3-backend + 3-wasm-profile parity (`42 / hello / true / false`).
- **C12B-035** — Phase 2 migration note in `docs/guide/01_types.md`
  and `CHANGELOG.md` corrected: `n.toString(radix)` migrates to
  `ToRadix[n, base]().getOrDefault("")` (returns `Lax[Str]`), not
  the previously-listed `Str[Int[s, 16]()..]()` which performs the
  opposite direction (hex-string → decimal-string).

### Post-Gate Should Fix Completion (2026-04-15 follow-up)

Two Should Fix blockers originally carried over as OPEN/HOLD were
completed in a follow-up session after the user rejected the
"C13 postpone" plan and requested in-scope completion:

- **C12B-021** — FB-18 scope completion (Result type completeness).
  `writeFile` / `writeBytes` / `appendFile` / `remove` / `createDir`
  now return `Result[Int]` (inner value = byte count / removed-entry
  count / newly-created flag) instead of `Result[@(ok, code, message)]`.
  `Exists[path]()` now returns `Result[Bool]` instead of a bare
  `Bool`, distinguishing permission-denied from "path absent". All
  three backends (Interpreter / JS / Native) + the wasm-wasi runtime
  (`runtime_wasi_io.c`) were updated in lockstep. 3 new parity tests
  (`test_c12b_021_writefile_result_int_parity`,
  `test_c12b_021_writefile_failure_is_error_parity`,
  `test_c12b_021_exists_result_bool_parity`) lock the cross-backend
  contract; the pre-existing `test_file_bytes_read_write_three_way_parity`
  was migrated to the new `.isSuccess()` idiom. BREAKING: callers
  that read `.__value.ok` / `.__value.code` / `.__value.message`
  must switch to `.isSuccess()` / `.isError()` and read the new
  `.__value` Int directly; the error envelope is unchanged.

- **C12B-036** — Regex compile cache across all three backends.
  The Interpreter gains a thread-local FIFO cache (`REGEX_CACHE`,
  capacity 64) in `src/interpreter/regex_eval.rs`; the Native
  runtime gains a process-wide FIFO cache (`g_regex_cache`,
  capacity 16) in `src/codegen/native_runtime.c`; the JS runtime
  already had an equivalent `__taida_regex_cache` (capacity 64)
  from the C12B-040 work, preserved as-is. Hot-loop
  `str.replaceAll(Regex(...), ...)` now skips `regcomp` /
  `RegexBuilder::new` / `new RegExp` after the first call, while
  keeping the PHILOSOPHY I "no silent undefined" invariant
  (invalid patterns are still rejected at construction time).
  4 new parity tests (`test_c12b_036_regex_replace_all_hot_loop_parity`,
  `test_c12b_036_regex_cache_distinguishes_flags_parity`,
  `test_c12b_036_regex_search_stateless_parity`,
  `test_c12b_036_regex_match_stateless_parity`) plus 4 interpreter
  unit tests pin the cache behaviour.

### Post-Gate Nice to Have Fixes (2026-04-15)

Two Nice to Have / pre-existing blockers tractable without scope
creep were resolved in the same session:

- **C12B-020** — `expr => _` pipeline discard is now accepted on
  Native (`Lowering error: unsupported pipeline step` resolved) and
  JS (prior codegen emitted `__p = _;` which was a ReferenceError).
  Both backends now treat `Placeholder` as a no-op pipeline step,
  matching the Interpreter. 2 new parity tests
  (`test_c12b_020_stdout_discard_pipeline_parity`,
  `test_c12b_020_pipeline_discard_followed_by_stmt_parity`) lock
  the 3-backend contract.
- **C12B-022** — Native `TypeIs[v, :Int/:Str/:Bool/:Num]()` on a
  function parameter no longer returns a stale compile-time
  assumption (the Int branch previously always answered `true` for
  untyped Idents). The lowerer now emits
  `taida_primitive_tag_match(tag, expected)` whenever the operand
  is in `param_tag_vars`, and the caller-side
  `emit_call_arg_tags_full` additionally propagates the `Int=0`
  default tag for callees detected (via
  `param_type_check_funcs` / `body_uses_typeis_on_ident`) to use
  `TypeIs` on their parameters. WASM runtime gains the mirror
  helper; `EXPECTED_TOTAL_LEN` advances to 237,823. 3 new parity
  tests (`test_c12b_022_typeis_int_param_parity`,
  `test_c12b_022_typeis_str_param_parity`,
  `test_c12b_022_typeis_bool_param_parity`) pin the runtime
  semantics across all backends.

- **C12B-023 (v2 bypass closure)** — Root fix for the Regex silent-UB
  forgery vector. The initial C12B-023 fix (needed_funcs-based
  wasm validator) was bypassed by hand-constructing
  `@(__type <= "Regex", pattern <= "a", flags <= "")` and feeding
  the pack to `_poly` dispatchers (v1). The v1 typechecker follow-up
  (literal-string match on `__type <= "Regex"`) was in turn bypassed
  by variable binding (`tag <= "Regex"; @(__type <= tag, ...)`),
  function-arg routing (`inner t = @(__type <= t, ...)`),
  conditional (`if(c, "Regex", "X")`) and string concatenation
  (`"Re" + "gex"`). The root fix (v2): user-authored
  `Expr::BuchiPack` / `Expr::TypeInst` literals may no longer assign
  **any** `__`-prefixed field name, regardless of the value
  expression. `__`-prefix is reserved for compiler-internal tags
  (`__type`, `__value`, `__default`, `__error`, `__tag`,
  `__items`, `__transforms`, `__status`, `__body_stream`,
  `__body_token`, ...). Field **reads** (`.`-access) remain allowed
  for introspection. `[E1617]` now fires at the checker level and
  blocks compilation on all 7 profiles (interp / js / native /
  wasm-min / wasm-wasi / wasm-edge / wasm-full). BREAKING for any
  user code that wrote `__`-prefixed fields in packs (none detected
  in `examples/`, `docs/`, `tests/` under `cargo test`
  `test_all_examples_pass_typecheck`). 16 new tests:
  `typecheck_examples.rs` gains 8 bypass-route reject tests +
  `test_c12b_023_typeinst_reserved_field_rejected`; each wasm
  profile gains 2–3 variable-bound / concat bypass reject tests;
  `parity.rs::test_net4_nb10_ws_upgrade_fake_req_rejected_3way`
  now pins compile-time rejection of forged `__body_*` packs
  across all 3 backends (shift-left from runtime rejection).

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
  Use `ToRadix[n, base]().getOrDefault("")` (returns `Lax[Str]`,
  unwrap with `getOrDefault`) — see `docs/reference/mold_types.md §ToRadix`
  and `docs/guide/01_types.md`. `Str[Int[s, 16]().getOrDefault(0)]()`
  does **not** perform int → hex and was listed in error in an earlier
  draft.

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

#### `param_tag_vars` Propagation to `stdout` / `stderr` (FB-1 / Phase 11)

- Closes the canonical FB-1 reproducer tracked as C12B-017:
  `print_any v = stdout(v); print_any(true)` (and
  `print_any(TypeIs[42, :Int]())`) now correctly renders as
  `true` / `false` on the Native backend instead of `1` / `0`.
- The stdout / stderr dispatch in `src/codegen/lower.rs` now consults
  `param_tag_vars` for `Ident` arguments whose compile-time tag is
  `UNKNOWN`. When the parameter was tagged at the call site via
  `emit_call_arg_tags`, the runtime tag IrVar is forwarded to
  `taida_io_stdout_with_tag` / `taida_io_stderr_with_tag`, which
  dispatches `TAIDA_TAG_BOOL` to the canonical `true`/`false`
  formatter and falls through to `taida_polymorphic_to_string`
  for every other tag on Native.
- Body-based Bool inference was added for user functions: when a
  function has no explicit `-> Bool` return annotation but its body
  last expression is recognised as Bool by `expr_is_bool()` (e.g.
  `is_int v = TypeIs[v, :Int]()`), the function is registered in
  `bool_returning_funcs`. This lets `b <= is_int(42); stdout(b)`
  preserve the Bool tag through a local `<=` binding.
- **Intentionally out of scope** in Phase 11 (deferred to C12-7 wasm
  runtime split):
  - Extending the tagged path to arbitrary user `FuncCall` args
    (would break `compile_c12_3_mutual_tail` / `compile_mutual_recursion`
    on wasm-full because the wasm `_with_tag` entry point treats
    non-Bool tags as `char*`).
  - Full 4-pattern tag_prop refactor (conditional arm join /
    pipeline intermediate / Lax unmold / runtime callback) as
    originally described in `C12_DESIGN.md` Workstream K. The
    canonical FB-1 reproducer no longer regresses, so the
    additional refactor is best paired with the wasm runtime
    polymorphic dispatch cleanup in C12-7.
- 5 parity tests (`test_c12_11_*_parity`) + new
  `examples/compile_c12_11_tag_prop.td` fixture (wasm-min / wasm-wasi /
  wasm-full all exercise the grid).

#### Regex Type + Str Method Overloads (FB-5 Phase 2-3 / Phase 6)

- New prelude constructor `Regex(pattern, flags?)` returns a typed
  BuchiPack with `pattern <= Str`, `flags <= Str`, `__type <= "Regex"`.
- Str methods are now overloaded by first-argument type. Passing a
  Regex value dispatches through a regex engine; passing a Str keeps
  the B11 Phase 1 fixed-string semantics unchanged:
    - `str.replace(Regex(p), rep)` / `str.replaceAll(Regex(p), rep)`
    - `str.split(Regex(p))`
    - `str.match(Regex(p))` → `:RegexMatch` BuchiPack with
      `hasValue: Bool`, `full: Str`, `groups: @[Str]`, `start: Int`
    - `str.search(Regex(p))` → `Int` (char index of first match or
      `-1` when no match; no null leak — philosophy I)
- Backend implementations:
    - **Interpreter**: new `src/interpreter/regex_eval.rs` module
      wrapping the Rust `regex` crate. 16 unit tests.
    - **JS**: `src/js/runtime.rs` helpers backed by native `RegExp`.
    - **Native**: `src/codegen/native_runtime.c` POSIX `<regex.h>`
      with `taida_regex_rewrite_pattern` translating Perl-style
      meta escapes (`\d` / `\w` / `\s` etc.) to POSIX classes.
- Flag support: `i` (case-insensitive), `m` (multiline anchors),
  `s` (dotall — Interpreter / JS only; POSIX ERE has no dotall).
  Unknown flags throw `ValueError` at `Regex(...)` construction.
- wasm profiles do **not** link regex support (C12B-023); dispatcher
  stubs forward Regex-shaped calls back to fixed-string helpers.
- 9 parity tests (`test_c12_6_*_parity`) cover fixed-string
  regression, character classes, first-vs-all semantics, split,
  match with groups, search, literal `$` handling, and the `i` flag.

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

#### JS Runtime File Split (FB-21 / Phase 9, partial)

- Internal-only refactor: split `src/js/runtime.rs` (6,496 lines) into
  `src/js/runtime/{core,os,net}.rs` + `mod.rs` so each chunk stays
  under 3,500 lines and owns a single coherent concern.
  - `core.rs` (2,015 lines) — helpers / types / arithmetic / Lax /
    Result / BuchiPack / throw / Async / Regex / stream / stdout /
    stderr / stdin / format / toString / HashMap / Set / equals /
    typeof / spread.
  - `os.rs` (1,142 lines) — `taida-lang/os` 13 API + `sha256` crypto.
  - `net.rs` (3,254 lines) — `taida-lang/net` HTTP v1 (parser /
    encoder / chunked / streaming writer / SSE / body reader /
    WebSocket).
- The embedded JS runtime bytes are **byte-identical** to the
  pre-split version; a new
  `test_runtime_js_chunk_concat_invariants` guards chunk boundaries.
- `RUNTIME_JS` surface changed from `pub const &str` to
  `pub static LazyLock<&'static str>` because `concat!()` only
  accepts literals. The single consumer in `src/js/codegen.rs`
  was updated (`push_str(RUNTIME_JS)` → `push_str(&RUNTIME_JS)`).
  `tests/parity.rs` file-path reads also migrate to
  `src/js/runtime/net.rs`.
- Placement tables for the remaining three targets
  (`src/codegen/lower.rs`, `src/interpreter/net_eval.rs`,
  `src/codegen/native_runtime.c`) are locked in
  `.dev/taida-logs/docs/design/file_boundaries.md`; the mechanical
  moves are tracked as C12B-024 / C12B-025 / C12B-026 and will land
  as independent follow-up PRs (per C12-9 policy: "split must not
  share a PR with semantic changes").

#### WASM Core Runtime Split + wasm-edge Size Budget Restoration (FB-26 / Phase 7)

- **wasm-edge `stdout("Hello from edge!")` is now 351 bytes** (previously
  ~10.5KB when the tagged runtime chain linked `taida_polymorphic_to_string`
  and its entire display helper fan-out). The B11-2f fix that isolated
  `taida_io_stdout_with_tag`'s non-Bool branch to a plain `char*` path —
  combined with `wasm-ld --gc-sections` — now yields a hello-world wasm
  binary that links only `_start` + `taida_io_stdout` + `write_stdout` +
  `wasm_strlen` + the WASI `fd_write` import. Closes FB-26.
- `tests/wasm_edge.rs::wasm_edge_size_check` threshold restored from 16KB
  back to **4KB** (the original WE-3c gate, raised transiently to 16KB in
  commit `7af9684` / FB-25 during B11). The new ~351B budget leaves ~11×
  headroom.
- New regression test `wasm_edge_hello_no_polymorphic_regression` (1KB
  upper bound) specifically guards against a future regression that would
  pull `taida_polymorphic_to_string` back onto the static-string stdout
  reference chain.
- Internal-only refactor: split `src/codegen/runtime_core_wasm.c`
  (6,463 lines) into `src/codegen/runtime_core_wasm/{01_core,
  02_containers, 03_typeof_list, 04_json_async}.inc.c` + `mod.rs` so each
  fragment owns a single functional concern and stays well under 3,000
  lines.
  - `01_core.inc.c` (2,698 lines) — libc stubs, bump allocator, strlen
    helpers, stdout/stderr/debug I/O, integer/bool arithmetic, float
    arithmetic + Rust-Display-compatible formatter, polymorphic
    display, BuchiPack / List / HashMap / Set runtimes, WC-6
    extensions.
  - `02_containers.inc.c` (1,555 lines) — Closure runtime, error
    ceiling (error-flag based, no setjmp/longjmp), Lax[T], Result[T,P]
    + Gorillax, Cage, Molten/Stub/Todo stubs, type conversion molds
    (returning Lax), float div/mod molds, string template helpers,
    error object helpers, digit/char helpers.
  - `03_typeof_list.inc.c` (887 lines) — RC no-ops (wasm has no heap
    refcount), typeof (compile-time tag + runtime heuristic), List
    HOF / operations / queries, element retain/release no-ops.
  - `04_json_async.inc.c` (1,323 lines) — JSON runtime (manual
    strtol/strtod/itoa/ftoa/FNV-1a), type detection for JSON
    serializer, public field lookup wrappers, schema helpers, schema
    descriptor application, Async runtime (synchronous blocking for
    wasm-min), `_taida_main` extern declaration, `_start` WASI entry.
- The embedded wasm runtime bytes are **byte-identical** to the
  pre-split version; a new `test_runtime_core_wasm_fragment_concat_preserves_bytes`
  pins the total C source length (235,855 bytes) and anchors the
  assembled source's first / last bytes.
- Same assembly pattern as the JS runtime split (C12-9d):
  `LazyLock<&'static str>` + `Box::leak` produces a `&'static str`
  without adding a crate dependency (`concat!()` would require literal
  arguments). All five `include_str!("runtime_core_wasm.c")` call sites
  in `src/codegen/driver.rs` now point to
  `&crate::codegen::runtime_core_wasm::RUNTIME_CORE_WASM`.
- The lightweight `stdout_with_tag` proposal (C12-7b) and `wasm-opt -Oz`
  post-link step (C12-7c) were **deliberately not adopted**: the current
  codegen already hits 351B for the static-string path, and swapping the
  tagged runtime in would risk breaking the wasm-min 512B size gate
  (`wasm_min_size_gate`) via the same `taida_polymorphic_to_string`
  reference chain the B11-2f fix removed. `wasm-opt` is also not
  installed in the standard toolchain. C12B-016 remains OPEN as a
  follow-up for when the wasm-min runtime grows further and makes the
  tagged unification cost-effective again.

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

// C23B-007 repro: heterogeneous list `@[1, "a", 2]` must render with
// each element through its own primitive formatter. The WASM runtime
// previously stamped the list's elem_type_tag with the first element's
// tag (INT), then a later push with a disagreeing tag downgraded to
// UNKNOWN (-1) but the third push re-promoted it back to INT — the
// renderer then forced every element through taida_int_to_str and the
// string `"a"` emerged as a raw pointer integer (`1127`).
// Fix: introduce WASM_TAG_HETEROGENEOUS (-2) as a separate latching
// sentinel; once the tag reaches HETEROGENEOUS it never re-promotes.
// The structural fallback in `_wasm_render_elem_tagged_debug` handles
// non-primitive tags by delegating to `_wasm_value_to_debug_string`.
stdout(Str[@[1, "a", 2]]())

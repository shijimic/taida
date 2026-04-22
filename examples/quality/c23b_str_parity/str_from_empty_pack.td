// C23B-003 reopen 3 guard: `Str[@()]()` must produce `@()` on all 4
// backends. WASM previously returned the raw heap pointer of the
// `taida_pack_new(0)` allocation (e.g. "73088") because
// `_looks_like_pack` only matches `fc >= 1`. See
// `_looks_like_empty_pack` in `src/codegen/runtime_core_wasm/01_core.inc.c`.
stdout(Str[@()]())

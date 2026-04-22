// C23B-003 reopen 4 guard: dynamic Int expression that evaluates to 0
// must not be confused with the empty pack `@()`. The pre-fix WASM
// detector fired on any zero-slot heap allocation, so an arithmetic
// result of 0 could (depending on bump-allocator state) take either
// the address-heuristic path (rendered as `@()`) or the int fallback
// (rendered as `"0"`) — producing flaky output across builds. After
// the reopen-4 fix, `Str[5 - 5]()` always lowers to `taida_str_mold_int`
// (the subtraction is recognised as Int at compile time via
// `expr_is_int`), and even if it fell through, the magic-sentinel
// detector would never confuse the plain integer 0 with a pack.
stdout(Str[5 - 5]())

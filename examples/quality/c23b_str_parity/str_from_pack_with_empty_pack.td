// C23B-003 reopen 3 guard: a ぶちパック field carrying an empty pack
// `@()` must render the inner value as `@()`, not as a raw pointer
// integer. Exercises `_wasm_pack_to_string_full` → `_wasm_value_to_debug_string_full`
// → `_looks_like_empty_pack` on wasm.
p <= @(u <= @())
stdout(Str[p]())

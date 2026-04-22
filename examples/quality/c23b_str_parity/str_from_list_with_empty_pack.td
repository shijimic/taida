// C23B-003 reopen 3 guard: a list containing an empty pack `@()` must
// render the list item as `@()`, not as a raw pointer integer.
// Exercises the List branch in `_wasm_stdout_display_string` that
// recurses through `_wasm_value_to_debug_string_full` →
// `_looks_like_empty_pack`.
xs <= @[@()]
stdout(Str[xs]())

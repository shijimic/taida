// C23B-003 reopen 3 guard: a HashMap value of empty pack `@()` must
// render as `@()` inside the synthetic `BuchiPack(__entries, __type)`
// form. Exercises `_wasm_hashmap_to_display_string_full` →
// `_wasm_value_to_debug_string_full` → `_looks_like_empty_pack`.
m <= hashMap().set("u", @())
stdout(Str[m]())

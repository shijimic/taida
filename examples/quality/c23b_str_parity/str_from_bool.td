// C23B-001 guard: wasm `_wasm_pack_to_string_full` now renders the `__default`
// WASM_TAG_STR field as `""` instead of the raw pointer integer of the
// static empty-string literal. Bool primitive Str has no conversion error
// surface — the interesting part is the surrounding Lax shape.
stdout(Str[true]())
stdout(Str[false]())

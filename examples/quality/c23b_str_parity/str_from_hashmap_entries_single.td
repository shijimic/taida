// C23B-009 (2026-04-22): single-entry HashMap exercises the field-name
// registration path with the minimum non-empty payload. Pins that the
// pair pack's `key` / `value` field tags (STR / hashmap value_type_tag)
// stamp through to the wasm full-form renderer without regressing the
// Int primitive value rendering.
m <= hashMap().set("x", 42)
stdout(Str[m.entries()]())

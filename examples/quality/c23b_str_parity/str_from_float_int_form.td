// C23B-001 / C23B-004 guard: integer-form Float -> Str drops the trailing `.0`
// so the resulting Lax carries `__value <= "3"` / `"-5"` / `"0"` (interpreter's
// `f.to_string()` contract). wasm-wasi previously emitted `"3.0"` / `"-5.0"`
// via `taida_float_to_str`, which is the `Value::to_display_string` format
// used by `stdout(Float)`, not the `Str[...]()` format.
stdout(Str[3.0]())
stdout(Str[-5.0]())
stdout(Str[0.0]())

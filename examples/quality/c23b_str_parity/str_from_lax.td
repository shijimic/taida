// C23B-002 + C23B-003 guard: `Str[Int[3.0]()]()` must produce the
// interpreter's full-form nested Lax rendering
//   `@(hasValue <= true, __value <= 3, __default <= 0, __type <= "Lax")`
// (with the outer Lax wrapping the inner Lax's display string).
// JS previously returned the short-form `Lax(3)`; native / wasm returned
// raw pointer integers via `taida_str_mold_int`.
stdout(Str[Int[3.0]()]())

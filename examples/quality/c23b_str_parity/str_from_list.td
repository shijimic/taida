// C23B-002 guard: `Str[@[1, 2, 3]]()` must produce the interpreter's
// `@[1, 2, 3]` display string, not a raw pointer integer (native / wasm
// previously fell through to `taida_str_mold_int` / pointer stringify).
stdout(Str[@[1, 2, 3]]())

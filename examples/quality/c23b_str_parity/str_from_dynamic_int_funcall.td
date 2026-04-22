// C23B-003 reopen 4 guard: Int-returning user function call. The
// function `double` has an explicit `:Int` return annotation, which
// registers it in `int_returning_funcs` so `expr_is_int` recognises
// `double(36544)` as Int at compile time. Without the richer Int
// check, this would fall through to `taida_str_mold_any` and (on
// WASM) risk the empty-pack heuristic. The chosen argument lands the
// result (73088) inside the bump-allocator arena on a fresh
// `_start`, the exact shape that exposed the reopen-4 regression.
double n = n * 2 => :Int
stdout(Str[double(36544)]())

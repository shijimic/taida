// C23B-003 reopen 4 guard: `Str[<dynamic Int expression>]()` must render
// the integer value on all 4 backends. Previously the WASM
// `_looks_like_empty_pack` detector false-positive'd on Int sums whose
// bit pattern happened to land inside the bump arena and point at an
// 8-byte-aligned zero chunk — rendering integers such as 73088 as the
// string `@()` (the empty-pack literal). The detector now uses a magic
// sentinel (`WASM_EMPTY_PACK_MAGIC`) stamped by `taida_pack_new(0)`,
// and `lower_molds.rs` also uses the richer `expr_is_int` check so
// dynamic Int expressions short-circuit to `taida_str_mold_int` at
// compile time. Both safeguards must hold to keep this fixture GREEN.
a <= 36000
b <= 37088
stdout(Str[a + b]())

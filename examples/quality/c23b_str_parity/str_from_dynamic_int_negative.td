// C23B-003 reopen 4 guard: negative dynamic Int expression. The
// negation operator on an Int-typed binding produces a negative
// integer at runtime. `expr_is_int` recognises
// `UnaryOp::Neg(Ident in int_vars)` as Int, so this short-circuits
// through `taida_str_mold_int` and never touches the empty-pack
// heuristic.
x <= 42
stdout(Str[-x]())

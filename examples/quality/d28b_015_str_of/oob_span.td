// D28B-015: out-of-bounds span — strOf function-form must produce the SAME
// output as the existing StrOf mold-form on the same backend (D28B-001 Lock
// co-existence). Note that pre-existing native lowering clamps OOB spans
// (via `taida_slice_mold`), while interpreter / JS return empty string —
// this divergence is shared by mold-form and function-form (it is a
// runtime-helper property, not a function-vs-mold property). Therefore this
// fixture asserts equivalence (`equal: true`) rather than a fixed string.
raw <= "hi"
span <= @(start <= 0, len <= 100)
fn_result <= strOf(span, raw)
mold_result <= StrOf[span, raw]()
stdout(`equal: ${fn_result == mold_result}`)

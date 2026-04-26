// D28B-015: strOf(span, raw) function-form equivalence with StrOf[span, raw]() mold-form
// Both forms must yield identical results (D28B-001 Lock co-existence justification).
raw <= "the quick brown fox"
span <= @(start <= 4, len <= 5)
fn_result <= strOf(span, raw)
mold_result <= StrOf[span, raw]()
stdout(fn_result)
stdout(mold_result)
stdout(`equal: ${fn_result == mold_result}`)

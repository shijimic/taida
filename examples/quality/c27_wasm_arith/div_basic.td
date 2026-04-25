// C27B-021 (2026-04-25): Float Div mold parity smoke.
// Verifies the boxed-int64 ABI (`taida_div_mold_f`) returns a Lax with
// hasValue == true for non-zero divisor.
a <= 3.0
b <= 2.0
c <= Div[a, b]()
stdout(c.hasValue.toString())

// C27B-021 wD Round 1 review (Critical 1 regression guard):
// Mod[1000000.5, 3.14159265]() — bit-pattern parity check.
// Pre-fix wasm produced 0.14357849990483373 (precision loss in 9th
// significant digit) because `(double)qi * b` rounded once qi exceeded
// 2^53 mantissa precision. Post-fix wasm matches interp/native/js
// exactly.
// Reference (interp / native / js): 0.14357849993359384
Mod[1000000.5, 3.14159265]() ]=> r
debug(r)

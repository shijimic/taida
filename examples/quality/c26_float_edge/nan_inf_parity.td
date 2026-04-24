// C26B-011 (Phase 11) — IEEE 754 NaN / ±Infinity 3-backend parity
//
// Rust f64::Display (what the interpreter uses via `n.to_string()`):
//   NaN         -> "NaN"
//   +Infinity   -> "inf"
//   -Infinity   -> "-inf"
//
// Before fix:
//   - JS: `String(Infinity)` → "Infinity" (drift); `String(NaN)` OK.
//   - Native: `taida_debug_float` used `%g` (dropped `.0`, also "inf"/"-inf"
//     mismatch via formatter); `Sqrt[-1.0]() ]=> nan` left `nan` untagged,
//     `debug(nan)` fell through to `taida_debug_int` printing the f64
//     bit-pattern as an Int (e.g. -2251799813685248).
//
// Interpreter is the reference.
Sqrt[-1.0]() ]=> nan
debug(nan)
Div[nan, 2.0] ]=> nan2
debug(nan2)
Mod[nan, 3.0] ]=> nan3
debug(nan3)
// Overflow-produced Infinity via `Div[huge, tiny]`:
Div[1e308, 1e-308] ]=> big
debug(big)
// Negative Infinity:
Div[-1e308, 1e-308] ]=> negbig
debug(negbig)
// Mod with Inf divisor -> dividend (since it fits entirely):
Mod[1.5, big] ]=> m
debug(m)

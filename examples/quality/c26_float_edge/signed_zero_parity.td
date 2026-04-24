// C26B-011 (wS Round 6) — IEEE 754 signed-zero formatting parity.
//
// Rust f64::Display (interpreter reference via `n.to_string()` wrapped in
// Taida's `to_display_string` → `format!("{:.1}", n)` for finite
// integer-valued floats) renders:
//   +0.0 -> "0.0"
//   -0.0 -> "-0.0"
//
// Native before fix: `taida_float_to_str` early-returned "0.0" for any
// `a == 0.0` (both IEEE-754 zeros compare equal), so -0.0 rendered as
// "0.0" and parity with the interpreter was silently broken. The fix
// dispatches on `signbit(a)` so only the sign-bit-on zero emits the
// minus.
//
// JS path uses `__taida_float_render` which handles -0.0 via
// `Object.is(n, -0.0)` for the sign detection.
//
// Compute -0.0 arithmetically (−1.0 × 0.0 = −0.0 per IEEE-754), which
// is the portable way to produce it in any backend — direct `-0.0`
// literals are subject to pre-existing codegen divergences unrelated to
// this float-formatter parity contract.

neg_zero <= -1.0 * 0.0
debug(neg_zero)

pos_zero <= 1.0 * 0.0
debug(pos_zero)

// Verify that subsequent arithmetic on a -0.0 value preserves the sign
// per IEEE-754 (0.0 + -0.0 == +0.0, but -0.0 * 1.0 == -0.0).
neg_zero_again <= neg_zero * 1.0
debug(neg_zero_again)

sum_zeros <= pos_zero + neg_zero
debug(sum_zeros)

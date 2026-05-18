// D28B-009 (2026-04-26, wD Round 1) — NaN propagation through arithmetic
// molds, pinned across all 4 backends (interpreter / JS / native /
// wasm-wasi). Interpreter is the reference.
//
// Root cause that this fixture closes:
//   - wasm-wasi `taida_mod_mold_f` previously coerced any NaN input to
//     Lax-empty (default 0.0), so `Mod[NaN, 3.0]` rendered "0.0" while
//     native/JS/interpreter all rendered "NaN".
//   - wasm-wasi/full `fmt_g` extracted the f64 sign bit before checking
//     NaN, so any negative-NaN bit pattern (e.g. produced by `Sqrt[-1.0]`
//     piped through the mold layer) rendered as "-NaN".
//
// Both bugs were latent in @c.27 and surfaced as soon as this fixture
// was written for the D28 4-backend stable gate.

// (1) Sqrt[-1.0]() unmold -> NaN. `nan` carries the IEEE-754 quiet NaN
// produced by libm sqrt.
Sqrt[-1.0]() >=> nan
nan_value: Float <= nan
debug(nan_value)

// (2) NaN propagates through Div: NaN / 2.0 = NaN.
Div[nan_value, 2.0] >=> nan_div
debug(nan_div)

// (3) NaN propagates through Mod: NaN % 3.0 = NaN. Pre-fix wasi
// silently produced 0.0 here.
Mod[nan_value, 3.0] >=> nan_mod
debug(nan_mod)

// (4) Float `+` on a NaN-bearing Lax projects the NaN through the
// arithmetic.  `nan + 1.0 = NaN`.
nan_plus <= nan_value + 1.0
debug(nan_plus)

// (5) Float `*` on the same NaN.
nan_mul <= nan_value * 2.0
debug(nan_mul)

// (6) Float `-` on the same NaN.
nan_sub <= nan_value - 1.0
debug(nan_sub)

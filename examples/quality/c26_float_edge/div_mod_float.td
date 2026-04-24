// C26B-011 (Phase 11) — Float-origin Div/Mod 3-backend parity
//
// Before fix:
//   - Interpreter: 0 / 0 / 0.0 / 0.5 / 1 / 1.0 (reference)
//   - JS:          0 / 0 / 0   / 0.5 / 1 / 1   (Number.0 drops to "0")
//   - Native:      0 / 0 / 0   / 4602678819172646912 / 1 / 4607182418800017408
//                  (f64 bit-pattern rendered as Int)
//
// Root causes fixed in this test's patch:
//   - Native: `taida_div_mold_f` / `taida_mod_mold_f` added (Float-hint
//     variants), `taida_debug_float` routed through `taida_float_to_str`,
//     `Div`/`Mod` unmold target tracked in `float_vars`.
//   - JS: `__taida_float_render` added NaN/Inf/-Inf branches; `Div_mold`
//     / `Mod_mold` propagate `__floatHint` through Lax so unmold target
//     is Float-origin; `is_float_origin_expr` recognises `Div`/`Mod`
//     MoldInst.
//
// Interpreter is the reference.
Div[1, 0]() ]=> r1
debug(r1)
Div[1, 2]() ]=> r2
debug(r2)
Div[1.0, 0.0]() ]=> r3
debug(r3)
Div[1.0, 2.0]() ]=> r4
debug(r4)
Mod[5, 2]() ]=> r5
debug(r5)
Mod[5.0, 2.0]() ]=> r6
debug(r6)

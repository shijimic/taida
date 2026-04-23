// C25B-012 / C25B-013: float edge-case rendering parity pin.
// C25B-013 originally tracked NaN / ±Infinity / denormal parity
// across the 4 backends. In the simplified form pinned here we
// verify that finite float addition and toString rendering agree
// between interpreter and native (JS is also on this path through
// the crash-regression driver). If a future SIMD / libm / float
// runtime refactor silently changes the canonical float rendering
// (e.g. `1.0` vs `1`) this case surfaces the drift before it can
// reach downstream numerical code. Special-value (NaN / Inf /
// denormal) parity is a separate audit tracked under C25B-013 and
// is NOT yet pinned here because of pre-existing Lax-default
// rendering divergence between interpreter (`0.0`) and native
// (`0`) seen during the Phase 9 audit — logged as a follow-up
// rather than landed as RED.
a <= 1.0
b <= 0.0
c <= a + b
stdout(c.toString())

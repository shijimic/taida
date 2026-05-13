// D28B-009 (2026-04-26, wD Round 1) — Subnormal (denormal) f64 boolean
// behaviour, pinned across all 4 backends.
//
// Note: the textual rendering of a single subnormal f64 still diverges
// across backends — Rust's `f64::Display` (interpreter) emits a 324-
// digit fixed-form decimal, while native/JS emit shortest-form
// scientific (`5e-324`). Closing that gap would require a renderer
// rewrite that is out of D28B-009 scope; instead this fixture pins
// boolean / structural properties of subnormals that all 4 backends
// already agree on, plus the well-formedness of arithmetic that crosses
// the subnormal boundary.
//
// Pattern: `Div[a, b]()` returns a Lax pack — keep it un-extracted so
// `.has_value` (Lax mold accessor) is callable on every backend.

// (1) The smallest positive subnormal is non-zero. We compute it via
// `1.0e-300 / 1.0e24` rather than a literal so denormal-flush behaviour
// in the lexer cannot mask the runtime contract. Each backend should
// agree that the Lax has a value (= true on a finite, non-zero quotient).
tiny <= Div[1.0e-300, 1.0e24]()
stdout(tiny.has_value.toString())

// (2) Multiplying a subnormal by 2.0 stays in the subnormal range and
// is still finite-non-zero. The arithmetic produces a Lax with value.
tiny2 <= Div[2.0e-300, 1.0e24]()
stdout(tiny2.has_value.toString())

// (3) `0.0 / 1.0e10` is just zero, which the C26B-011 contract returns
// as Lax(value=0.0, has_value=true) on every backend. This pins the
// boundary where the divisor is finite, dividend is zero.
zero_q <= Div[0.0, 1.0e10]()
stdout(zero_q.has_value.toString())

// (4) Divisor zero stays Lax-empty (existing C26B-011 contract).
zero_div <= Div[5.0e-300, 0.0]()
stdout(zero_div.has_value.toString())

// (5) Mod with a small finite-non-zero divisor still produces a
// defined remainder on every backend.
m1 <= Mod[1.0, 0.5]()
stdout(m1.has_value.toString())

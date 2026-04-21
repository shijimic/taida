// C21B-seed-04 (2026-04-22 reopen): JS Float-origin propagation through
// local binding. The Taida interpreter distinguishes `Value::Float(3.0)`
// from `Value::Int(3)`, so `Float[x]()` after `x <= 3.0` yields a
// `Lax[3.0]` buchi pack whose `__value` renders with `.0`. JS `Number`
// cannot distinguish `3` from `3.0` at runtime, so codegen must carry
// the Float-origin tag forward to the `Float[...]()` mold call site.
//
// Expected (4 backends):
//   @(hasValue <= true, __value <= 3.0, __default <= 0.0, __type <= "Lax")
//   @(hasValue <= true, __value <= 3, __default <= 0, __type <= "Lax")
//
// Pre-fix JS output:
//   @(hasValue <= true, __value <= 3, __default <= 0, __type <= "Lax")  // BUG: should be 3.0
//   @(hasValue <= true, __value <= 3, __default <= 0, __type <= "Lax")
x <= 3.0
stdout(Float[x]())
stdout(Int[x]())

// C21B-seed-04 (2026-04-22 reopen): Float-origin must propagate through
// a user function call result bound to a local name, then through a
// subsequent `Float[y]()` mold call site. One level deeper than the
// direct `stdout(triple(4.0))` case the Phase 5 original fix covered.
//
// Expected (4 backends):
//   @(hasValue <= true, __value <= 12.0, __default <= 0.0, __type <= "Lax")
triple x: Float =
  x * 3.0
=> :Float

y <= triple(4.0)
stdout(Float[y]())

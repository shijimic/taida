// E30B-004 / E30 Phase 6: declare-only function field defaultFn returns
// Str default ("") when called without explicit binding.
//
// Lock-D verdict (E30 Phase 0, 2026-04-28): defaultFn for `Str => :Str`
// returns the Str default (empty string). All four backends materialise
// the synthetic `defaultFn` and the call yields `""` (printed as
// nothing followed by newline).
Pilot = @(name: Str, greet: Str => :Str)
p <= Pilot(name <= "Rei")
result <= p.greet("hello")
stdout(result.length().toString())

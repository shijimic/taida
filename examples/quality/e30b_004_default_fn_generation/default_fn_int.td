// E30B-004 / E30 Phase 6: declare-only function field defaultFn returns
// Int default (0) when called without explicit binding.
//
// Lock-D verdict (E30 Phase 0, 2026-04-28): defaultFn for `Int => :Int`
// returns the Int default (0). All four backends materialise the
// synthetic `defaultFn` and the call yields `0`.
Counter = @(value: Int, getNext: Int => :Int)
c <= Counter(value <= 10)
n <= c.getNext(0)
stdout(n.toString())

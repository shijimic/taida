// E30B-004 / E30 Phase 6: declare-only function field defaultFn returns
// Bool default (false) when called without explicit binding.
//
// Lock-D verdict (E30 Phase 0, 2026-04-28).
Predicate = @(label: Str, check: Int => :Bool)
p <= Predicate(label <= "any")
b <= p.check(0)
stdout(b.toString())

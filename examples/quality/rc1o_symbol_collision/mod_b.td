// RC-1o: Symbol collision — module B
// Has _helper and compute (same names as mod_a)
// _helper triples: x * 3

_helper x: Int = x * 3 => :Int

compute x: Int = _helper(x) + 2 => :Int

<<< @(compute)

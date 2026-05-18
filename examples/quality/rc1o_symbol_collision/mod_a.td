// RC-1o: Symbol collision — module A
// Has _helper and compute (same names as mod_b)
// _helper doubles: x * 2

_helper x: Int = x * 2 => :Int

compute x: Int = _helper(x) + 1 => :Int

<<< @(compute)

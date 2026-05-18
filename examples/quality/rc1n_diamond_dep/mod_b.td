// RC-1n: Diamond dependency — module B (imports D)

>>> ./mod_d.td => @(shared)

_b_helper x: Str = shared(x) => :Str

fromB x: Str = "B:" + _b_helper(x) => :Str

<<< @(fromB)

// RC-1n: Diamond dependency — module C (imports D)

>>> ./mod_d.td => @(shared)

_c_helper x: Str = shared(x) => :Str

fromC x: Str = "C:" + _c_helper(x) => :Str

<<< @(fromC)

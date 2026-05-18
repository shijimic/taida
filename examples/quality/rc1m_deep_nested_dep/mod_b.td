// RC-1m: Deep nested dependency — module B (imports C)

>>> ./mod_c.td => @(wrapC)

_internal_b x: Str = "B(" + wrapC(x) + ")" => :Str

wrapB x: Str = _internal_b(x) => :Str

<<< @(wrapB)

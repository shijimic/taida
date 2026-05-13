// RC-1m: Deep nested dependency — module B (imports C)

>>> ./mod_c.td => @(wrapC)

_internal_b x = "B(" + wrapC(x) + ")" => :Str

wrapB x = _internal_b(x) => :Str

<<< @(wrapB)

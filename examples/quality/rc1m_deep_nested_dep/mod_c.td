// RC-1m: Deep nested dependency — module C (imports D)

>>> ./mod_d.td => @(wrapD)

_internal_c x = "C(" + wrapD(x) + ")" => :Str

wrapC x = _internal_c(x) => :Str

<<< @(wrapC)

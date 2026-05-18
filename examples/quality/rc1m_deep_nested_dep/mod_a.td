// RC-1m: Deep nested dependency — module A (imports B)

>>> ./mod_b.td => @(wrapB)

wrapA x: Str = "A(" + wrapB(x) + ")" => :Str

<<< @(wrapA)

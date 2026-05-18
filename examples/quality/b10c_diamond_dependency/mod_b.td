// B-10c mod_b: imports mod_d
>>> ./mod_d.td => @(shared)

fromB x: Str = "B:" + shared(x) => :Str

<<< @(fromB)

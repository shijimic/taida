// B-10h mod_a: imports mod_b (circular)
>>> ./mod_b.td => @(world)

hello x = "Hello:" + world(x) => :Str

<<< @(hello)

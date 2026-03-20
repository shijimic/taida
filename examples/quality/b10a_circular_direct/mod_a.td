// B-10a mod_a: imports mod_b (which imports mod_a back)
>>> ./mod_b.td => @(world)

hello x = "Hello:" + world(x) => :Str

<<< @(hello)

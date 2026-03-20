// B-10a mod_b: imports mod_a back (circular!)
>>> ./mod_a.td => @(hello)

world x = "World:" + x => :Str

<<< @(world)

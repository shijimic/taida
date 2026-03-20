// B-10b mod_c: imports mod_a back (circular!)
>>> ./mod_a.td => @(start)

bottom x = "C:" + x => :Str

<<< @(bottom)

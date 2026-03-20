// Quality Test: B-10c — Diamond dependency (not circular, should succeed)
// A -> B, A -> C, B -> D, C -> D

>>> ./mod_b.td => @(fromB)
>>> ./mod_c.td => @(fromC)

stdout(fromB("x"))
stdout(fromC("y"))

// Expected: B:shared:x
// Expected: C:shared:y

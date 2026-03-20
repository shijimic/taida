// Quality Test: B-10h — Cross-backend circular import detection
// Same circular pattern as B-10a, run on all 3 backends for error parity

>>> ./mod_a.td => @(hello)

stdout(hello("test"))

// Expected: all 3 backends detect circular import with error

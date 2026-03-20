// Quality Test: B-10a — Direct circular import (A -> B -> A)
// Expects: circular import error from all 3 backends

>>> ./mod_a.td => @(hello)

stdout(hello("test"))

// Expected: circular import error

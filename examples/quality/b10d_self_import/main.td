// Quality Test: B-10d — Self import (A -> A)
// Expects: circular import error

>>> ./main.td => @(greet)

stdout(greet())

// Expected: circular import error

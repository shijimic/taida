// Quality Test: B-10f — Circular import with closure export
// Expects: circular import error, closure resolution should not crash

>>> ./mod_a.td => @(makeGreeter)

greeter <= makeGreeter("Hi")
stdout(greeter("World"))

// Expected: circular import error (not a crash)

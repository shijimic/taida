// RC-1m: Deep nested dependency test — main entry point
// Chain: main -> A -> B -> C -> D (4 levels of module dependency)
// Each module has internal functions that should be namespaced correctly.
// Expected output: A(B(C(D(hello))))

>>> ./mod_a.td => @(wrapA)

stdout(wrapA("hello"))

// QF-34: Bool mold default value should be "false" not "0"
// Verifies Bool["maybe"]() ]=> result outputs "false" on all 3 backends.

// Direct unmold
Bool["maybe"]() ]=> a
stdout(a)

// Unmold via variable
x <= Bool["hello"]()
x ]=> b
stdout(b)

// Lax hasValue should also output as Bool
stdout(x.has_value)

// Success case
Bool[true]() ]=> c
stdout(c)

// Int-to-Bool mold
Bool[0]() ]=> d
stdout(d)

Bool[1]() ]=> e
stdout(e)

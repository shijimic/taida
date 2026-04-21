// C20B-014 (ROOT-17): User-defined functions called via mold syntax
// `Fn[args]()` must dispatch to the function, not silently wrap the
// arguments in a `@(__value, __type)` mold struct.
//
// Pre-C20B-014 this file reproduced the Hachikuma TUI crash at 81
// call sites (`CursorMoveTo[r, c]()`, `PadWidth[t, w]()`, …): the
// Interpreter wrapped the value, Native failed at lowering with
// "unsupported mold type", and only JS happened to work. Post-fix,
// all three backends must produce the same output as the equivalent
// `Fn(args)` calls.

cursorMoveTo row col = "[" + row.toString() + ";" + col.toString() + "H" => :Str

square n = n * n => :Int

concatWith sep a b = a + sep + b => :Str

// Mold syntax -- must invoke the function and return its value.
mold1 <= cursorMoveTo[1, 1]()
mold2 <= square[5]()
mold3 <= concatWith["-", "x", "y"]()

// Plain function-call syntax -- reference output.
call1 <= cursorMoveTo(1, 1)
call2 <= square(5)
call3 <= concatWith("-", "x", "y")

stdout("mold1=")
stdout(mold1)
stdout("\n")
stdout("call1=")
stdout(call1)
stdout("\n")
stdout("mold2=")
stdout(mold2.toString())
stdout("\n")
stdout("call2=")
stdout(call2.toString())
stdout("\n")
stdout("mold3=")
stdout(mold3)
stdout("\n")
stdout("call3=")
stdout(call3)
stdout("\n")

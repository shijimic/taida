// C25B-015 scenario 2: a lambda (`_ x = ...`) raises an error inside
// its body; the outer `|==` catches it and the caller scope keeps
// working normally on all three backends.
//
// This is the scenario C20B-015 / C20B-017 originally hardened for the
// interpreter only. The parity regression target here is that the JS
// backend's `try/finally` and the native backend's `setjmp/longjmp`
// both release the lambda frame when the lambda body throws, and do
// not accidentally leak a borrow into the outer scope.
//
// The lambda captures `tag` from its enclosing function (`trial`). If a
// backend leaked the lambda's captured `tag` into the caller scope,
// `tail` at top-level would bind unexpectedly.
//
// Expected: "caught:oops;tag=T tail:still-alive" followed by two
// newlines.

Error => MyError = @()

throwOops msg =
  MyError(type <= "MyError", message <= msg).throw()
  ""
=> :Str

trial input =
  |== error: Error =
    tag <= "T"
    "caught:" + error.message + ";tag=" + tag
  => :Str
  tag <= "T"
  inner x = throwOops("oops") + " " + tag + " " + x => :Str
  inner(input)
=> :Str

msg <= trial("input")
tail <= "still-alive"
stdout(msg + " tail:" + tail)
stdout("\n")

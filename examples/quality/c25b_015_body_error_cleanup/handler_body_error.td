// C25B-015 scenario 3: the body of an `|==` handler itself throws a
// secondary error; the outer `|==` catches it. This stresses the
// nested-handler cleanup that C20B-017 (ROOT-20) fixed in the
// interpreter and that the JS / native backends must also handle
// symmetrically.
//
// Pre-C20B-017 interpreter bug: when the inner handler body raised a
// secondary error, the inner handler's pushed scope was never popped
// and the caller's closure scope survived across top-level inputs.
// The parity regression target here is that the JS `try/finally` and
// native `setjmp/longjmp` both clean up the inner handler frame on
// the secondary throw path too.
//
// Expected: "outer:second inner-tag=gone" followed by a newline.

Error => FirstError = @()
Error => SecondError = @()

throwFirst msg =
  FirstError(type <= "FirstError", message <= msg).throw()
  ""
=> :Str

throwSecond msg =
  SecondError(type <= "SecondError", message <= msg).throw()
  ""
=> :Str

trial input =
  |== outer: Error =
    "outer:" + outer.message + " inner-tag=gone"
  => :Str
  |== inner: Error =
    chained <= throwSecond("second")
    "inner-should-not-be-seen:" + chained
  => :Str
  throwFirst("first")
=> :Str

msg <= trial("input")
stdout(msg)
stdout("\n")

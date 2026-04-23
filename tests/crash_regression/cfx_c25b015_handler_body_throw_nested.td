// C25B-012 / C25B-015: trimmed reproduction of C20B-015 / C20B-017
// scope-cleanup symmetry. A throw deep in a 3-frame call chain must
// unwind cleanly through interpreter eval.rs, JS try/finally, and
// native setjmp/longjmp and let the caller scope survive the catch.
// The canonical parity suite lives in
// tests/c25b_015_body_error_cleanup_parity.rs (Phase 6 driver);
// this corpus entry provides an independent minimal reproduction so
// the crash driver catches it even when the parity suite is skipped
// (e.g. node not available, release build not configured).

Error => MyError = @()

throwBoom msg =
  MyError(type <= "MyError", message <= msg).throw()
  ""
=> :Str

deepest n = throwBoom("boom") => :Str
middle n = deepest(n) => :Str
top n = middle(n) => :Str

trial input =
  |== error: Error =
    "caught:" + error.message
  => :Str
  top(input)
=> :Str

stdout(trial(0 - 1))
stdout("tail=still-alive")

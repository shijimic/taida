// C25B-015 scenario 1: a throw inside a deeply nested call unwinds
// through three frames (deepest -> middle -> top -> catch) cleanly on
// all three backends. Regression guard that the error-ceiling handler
// pop is symmetric in the interpreter (`eval.rs`), the JS codegen
// (`try/finally` in generated JS), and the native backend
// (`setjmp/longjmp` with scope cleanup in `core.c`).
//
// Expected: "caught:boom tail:still-alive" followed by newline.
// The "tail" field proves the caller scope survives the unwind and
// keeps executing normally after the handler runs.

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

msg <= trial(0 - 1)
tail <= "still-alive"
stdout(msg + " tail:" + tail)
stdout("\n")

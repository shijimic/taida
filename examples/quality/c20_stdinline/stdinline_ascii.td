// C20-2: a plain ASCII line round-trips through `stdinLine`. Confirms
// the Async[Lax[Str]] surface works end-to-end and that `]=>` properly
// unmolds the Async into a Lax.
//
// Driver: pipe `hello\n` into stdin.

stdinLine("") ]=> line
stdout("hasValue=" + line.hasValue().toString())
stdout("val=" + line.getOrDefault("<empty>"))

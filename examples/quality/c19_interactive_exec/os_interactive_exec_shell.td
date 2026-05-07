// C19: execShellInteractive 3-backend parity test.
//
// Asserts 3-backend agreement on the success / failure outcome. See
// `os_interactive_run.td` for why the error-path fields are not probed.

r1 <= execShellInteractive("exit 0")
stdout("r1.hasValue=" + r1.hasValue.toString())
r1 ]=> r1V
stdout("r1.code=" + r1V.code.toString())

r2 <= execShellInteractive("exit 3")
stdout("r2.hasValue=" + r2.hasValue.toString())

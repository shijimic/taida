// C19: runInteractive 3-backend parity test.
//
// Asserts that Interpreter / JS / Native agree on the success / failure
// outcome of `runInteractive` via the Gorillax `hasValue` flag and the
// success-path code. This file does NOT exercise the real TTY
// passthrough — CI has no TTY. The passthrough behaviour is validated
// manually via Hachikuma B-006 smoke (`runInteractive("nvim", ...)`).
//
// Why not probe `err.code` / `err.kind` here?
// Native captures ProcessError as a BuchiPack whose inner accessor for
// `code` / `kind` is not part of the 3-backend parity contract for the
// captured-variant `run` either (pre-existing native limitation). We pin
// exactly the fields that are uniformly reachable: hasValue + success code.
//
// Covered paths:
// - Exit 0 (success, Gorillax.ok, code = 0)
// - Exit 7 (Gorillax.err, hasValue = false)
// - ENOENT (Gorillax.err, hasValue = false)

r1 <= runInteractive("/bin/sh", @["-c", "exit 0"])
stdout("r1.hasValue=" + r1.hasValue.toString())
r1 ]=> r1V
stdout("r1.code=" + r1V.code.toString())

r2 <= runInteractive("/bin/sh", @["-c", "exit 7"])
stdout("r2.hasValue=" + r2.hasValue.toString())

r3 <= runInteractive("/nonexistent/taida_c19_os_interactive_run_xyz", @[])
stdout("r3.hasValue=" + r3.hasValue.toString())

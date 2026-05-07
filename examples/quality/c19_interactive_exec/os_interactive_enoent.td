// C19B-001 / C19B-002: 3-backend failure-path parity for
// `runInteractive` when the target program does not exist (ENOENT).
//
// 現状この probe は internal-field reject の対象。Gorillax の
// error-field (`type` / `kind` / `code`) を user code から取得する
// 公式 accessor がまだ無いため、`tests/c19_interactive_exec.rs`
// 側で該当 3 entry が `#[ignore]` 化されている。Historical contract は
// 以下に保持し、公式 error-field accessor が land したら通常の
// probe pattern (`|==` catch + record return) に migrate して
// ignore を外す。
//
// Pins the IoError contract on the failure path:
// - hasValue = false
// - error.type   = "IoError"      (not ProcessError)
// - error.kind   = "not_found"    (classify_io_error_kind for ENOENT)
// - error.code   = 2              (POSIX ENOENT)
//
// Why this matters:
//   Before C19B-001 the Native backend collapsed execvp failure into a
//   child `_exit(127)` and the parent surfaced that as a generic
//   ProcessError(127) — indistinguishable from a program that merely
//   exited with 127. The errno-pipe fix lets Native push the child's
//   errno to the parent and build a proper IoError, matching Interpreter
//   and JS. Before C19B-002 the Native Gorillax field-hash mismatch made
//   error-field access silently unreachable; the HASH___ERROR fix
//   unblocks this parity test.

r <= runInteractive("/nonexistent/taida_c19_xyz_does_not_exist", @[])
stdout("hasValue=" + r.hasValue.toString())
stdout("type=" + r.__error.type.toString())
stdout("kind=" + r.__error.kind.toString())
stdout("code=" + r.__error.code.toString())

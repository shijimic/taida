// 3-backend failure-path parity for `runInteractive` when the target
// program does not exist (ENOENT).
//
// Pins the IoError contract on the failure path:
// - has_value = false
// - error.type   = "IoError"      (not ProcessError)
// - error.kind   = "not_found"    (classify_io_error_kind for ENOENT)
// - error.code   = 2              (POSIX ENOENT)

r <= runInteractive("/nonexistent/taida_c19_xyz_does_not_exist", @[])
stdout("has_value=" + r.has_value.toString())
r.errorInfo() ]=> err
stdout("type=" + err.type.toString())
stdout("kind=" + err.kind.toString())
stdout("code=" + err.code.toString())

// C21B-seed-04 (2026-04-22 reopen): Float-origin local binding must carry
// through plain `stdout(x)` and `x.toString()` as well. Matches the
// interpreter's `Value::Float(3.0).to_string()` = "3.0".
//
// Expected (4 backends):
//   3.0
//   3.0
x <= 3.0
stdout(x)
stdout(x.toString())

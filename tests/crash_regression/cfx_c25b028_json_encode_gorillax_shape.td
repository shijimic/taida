// C25B-012 / C25B-028: `jsonEncode(Gorillax)` shape parity regression
// guard. Interpreter emits
//   {"__error":{},"__value":42,"hasValue":true}
// and native/js previously diverged on key ordering + boolean encoding
// of `hasValue`. Fixed by commit 48d26da; pinned here so any future
// json encoder refactor retains 3-backend parity.
g <= Gorillax[42]()
stdout(jsonEncode(g))

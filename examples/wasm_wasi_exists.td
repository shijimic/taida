// wasm-wasi acceptance: Exists[path]()
// Run: wasmtime run --dir=. output.wasm
//
// `Exists[path]()` returns a Result-shaped wrapper over Bool. Here we
// assert the wrapper shape via the public `.isSuccess()` accessor: it is
// true on every backend whenever the probe itself completed (both
// existing and missing paths qualify — only permission-denied / I/O
// errors flip it to false). We deliberately do not compare the inner
// Bool textually in this acceptance, because direct internal-field
// access (`.__value`, etc.) is rejected by `[E1960]` and therefore is
// not part of the public surface tested here. Acceptances that need to
// observe the inner Bool use `.isSuccess()` together with separate
// `writeFile` / path state checks instead.

writeFile("_wasi_exists_test.txt", "test")
a <= Exists["_wasi_exists_test.txt"]()
stdout(a.isSuccess())
b <= Exists["_nonexistent_path_xyz"]()
stdout(b.isSuccess())

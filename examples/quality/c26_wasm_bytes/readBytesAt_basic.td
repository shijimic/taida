// C26B-020 柱 3 smoke: readBytesAt on wasm-wasi / wasm-full
//
// The test harness (tests/c26b_020_wasm_bytes_at.rs) pre-creates
// `_c26b_020_wasi_tmp.bin` as a 16-byte payload "ABCDEFGHIJKLMNOP",
// then invokes this fixture under wasmtime.  That keeps the Taida
// source independent of the `Bytes[...]()` constructor mold, which
// is implemented only on wasm-full (not on wasm-wasi) and is out of
// scope for this smoke.
//
// Exercises three readBytesAt cases and prints `.hasValue` for each:
//   (a) offset=0,  len=4    -> success, 4 bytes
//   (b) offset=12, len=8    -> success, truncated tail (4 bytes)
//   (c) offset=32, len=4    -> success, empty (beyond EOF)

chunkA <= readBytesAt("_c26b_020_wasi_tmp.bin", 0, 4)
stdout(chunkA.hasValue.toString())

chunkB <= readBytesAt("_c26b_020_wasi_tmp.bin", 12, 8)
stdout(chunkB.hasValue.toString())

chunkC <= readBytesAt("_c26b_020_wasi_tmp.bin", 32, 4)
stdout(chunkC.hasValue.toString())

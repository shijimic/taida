// C27B-020 (2026-04-25): wasm-side `bytes.length()` regression guard.
// The harness pre-creates `_c27b_020_len_tmp.bin` (16 bytes "ABCDEFGHIJKLMNOP")
// in CWD; this fixture reads 4 bytes and prints the Bytes length.
// Expected: 4 on every backend (interpreter / JS / native / wasm-wasi /
// wasm-full). Before the fix wasm-full silently returned 0 because
// `taida_polymorphic_length` lacked a Bytes branch.

chunk <= readBytesAt("_c27b_020_len_tmp.bin", 0, 4)
chunk ]=> bytes
stdout(bytes.length().toString())

// C27B-020 (2026-04-25): wasm-side BytesCursor + Take + U32LEDecode chain.
// The harness pre-creates `_c27b_020_chain_tmp.bin` containing exactly the
// 4 bytes "TEST" (0x54 0x45 0x53 0x54). Little-endian decode of those 4
// bytes is 0x54534554 = 1414743380.
//
// Before C27B-020 the fix this fixture was rejected by wasm-wasi at
// compile time (`taida_bytes_cursor_new` not registered) and produced 0
// on wasm-full because `taida_polymorphic_length` mis-dispatched Bytes.

chunk <= readBytesAt("_c27b_020_chain_tmp.bin", 0, 4)
chunk ]=> bytes
cursor <= BytesCursor[bytes](offset <= 0)
BytesCursorTake[cursor, 4]() ]=> step
v <= U32LEDecode[step.value]().getOrDefault(0)
stdout(v.toString())

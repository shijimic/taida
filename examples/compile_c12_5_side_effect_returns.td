// C12-5 (FB-18) parity fixture — `stdout` / `stderr` return the byte count
// (Int) of the written payload instead of `Value::Unit`. This example
// exercises the new contract across the 3 backends (Interpreter / JS /
// Native) and documents the exact byte counts we pin.
//
// PHILOSOPHY alignment:
//   I. null/undefined の完全排除 — `Value::Unit` is no longer observable
//      from Taida surface. Every side-effect function now returns a value
//      with a default (Int defaults to 0).
//
// Byte-count semantics:
//   - Counted: UTF-8 byte length of the rendered payload (what the user
//     handed to `stdout`).
//   - Excluded: the trailing newline added by the runtime for display.

// ── Basic ASCII byte counts ──────────────────────────────
n1 <= stdout("hi")
stdout(n1)
// expected: "hi" then "2"

n2 <= stdout("hello")
stdout(n2)
// expected: "hello" then "5"

// ── Empty payload ────────────────────────────────────────
n3 <= stdout("")
stdout(n3)
// expected: "" then "0"

// ── Non-Str arg — byte count matches display string ──────
n4 <= stdout(42)
stdout(n4)
// expected: "42" then "2"

n5 <= stdout(true)
stdout(n5)
// expected: "true" then "4"

// ── Arithmetic on stdout return — must be Int, not Unit ──
n6 <= stdout("ok")
total <= n6 + 1
stdout(total)
// expected: "ok" then "3"

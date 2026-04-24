// C26B-020 柱 3 error-path smoke: readBytesAt on wasm-wasi / wasm-full
//
// Exercises the failure branches.  Each call must produce a Lax value
// so `.hasValue` is always printable (never crashes the runtime).
//
// (a) missing file                  -> Lax failure (hasValue = false)
// (b) negative offset               -> Lax failure
// (c) negative length               -> Lax failure
// (d) len over the 64 MB ceiling    -> Lax failure
//
// The test harness does not pre-create any file for this fixture.

missing <= readBytesAt("_c26b_020_does_not_exist.bin", 0, 4)
stdout(missing.hasValue.toString())

negOff <= readBytesAt("_c26b_020_does_not_exist.bin", -1, 4)
stdout(negOff.hasValue.toString())

negLen <= readBytesAt("_c26b_020_does_not_exist.bin", 0, -1)
stdout(negLen.hasValue.toString())

tooBig <= readBytesAt("_c26b_020_does_not_exist.bin", 0, 70000000)
stdout(tooBig.hasValue.toString())

// compile_c12b_034_wasm_nonbool_param.td — C12B-034 regression fixture
//
// Non-Bool values flowing through a user function parameter into
// `stdout(v)` must be memory-safe on every backend. Before C12B-034
// the wasm `taida_io_stdout_with_tag` entry point cast non-Bool `val`
// directly to `char*`, which crashed (SIGSEGV / memory corruption) when
// `val` was e.g. an Int. After the fix, non-Bool non-Str tags route
// through `taida_polymorphic_to_string` so the failure mode is
// well-defined and matches the Interpreter / Native / JS display.
//
// 3 backend (interp / JS / native) + 3 wasm profile (min / wasi / full)
// must all produce the same output.

printInt v: Int =
    stdout(v)
=> :Int

printStr v: Str =
    stdout(v)
=> :Int

printBool v: Bool =
    stdout(v)
=> :Int

printInt(42)
printStr("hello")
printBool(true)
printBool(false)

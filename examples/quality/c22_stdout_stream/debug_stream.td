// C22-2 / C22B-002: `debug()` routes to **stdout** in stream mode
// (not stderr), matching the JS backend (`console.log`) and the
// Native backend (`taida_debug_*` → `printf`).
//
// The IMPL_SPEC originally proposed routing `debug` to stderr for
// symmetry with `stderr()`, but doing so would have broken
// 3-backend parity on `test_native_compile_parity` and every other
// harness that diffs captured stdout across backends. The
// interpreter was the outlier, not the reference, so the
// correction landed on the interpreter side: stream-mode `debug`
// writes to stdout with `writeln!` + `flush`.
//
// The value passed to `debug` is also returned, so it can sit
// inside a pipeline without being discarded. This fixture stays on
// the Str form (single-arg, no label) because that shape is
// observably identical on all three backends: the labeled form
// (`debug(value, "tag")`) renders `[tag] Type: repr` on the
// Interpreter but one plain line per argument on JS — a separate
// tracked gap, not covered by this parity pin.

stdout("begin")
debug("trace-a")
stdout("middle")
debug("trace-b")
stdout("end")

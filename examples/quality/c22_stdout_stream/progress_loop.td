// C22-2 / C22B-002: stream-mode `stdout()` must flush each line as
// it is written, not buffer everything until `eval_program` returns.
//
// Before C22 the interpreter pushed every `stdout(...)` call into a
// `Vec<String>` that was drained after evaluation finished, so a
// long-running CLI script produced nothing until it exited. This
// hid progress output, spinner frames, and printf-debugging.
//
// The 2-mode split introduced by C22 leaves REPL / in-process tests
// on the buffered path (observably identical to the pre-C22
// behaviour) and routes `taida <file>` / `taida run <file>` through
// the stream path (`writeln!` + `flush`). JS (`console.log`) and
// Native (`printf` via `taida_stdout_*`) are already streaming, so
// the three backends now agree on one observable byte sequence.
//
// This fixture emits four progress-style lines in order. The parity
// harness asserts that Interpreter / JS / Native all produce the
// same output (the file-level `expected`).

emit label: Str =
  stdout("step=" + label)
=> :Int

// Four sequential emissions simulate a progress loop. Using a
// `Map[list, emit]()` rather than an inline `Fold` callback keeps
// the example compatible across backends (Fold side-effect bodies
// with block syntax currently trip the parser; see c22_fold probe).
labels <= @["1", "2", "3", "4"]
Map[labels, emit]() ]=> _emitted

stdout("done")

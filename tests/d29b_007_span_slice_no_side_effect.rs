//! D29B-007 (Phase 2 / Track-α, Lock-D D2): SpanSlice raw-arg side-effect
//! suppression.
//!
//! `SpanSlice[span, raw, start, end]()` derives a sub-span pack
//! `@(start: Int, len: Int)` purely from the input span pack and the integer
//! bounds. The `raw` (2nd type-arg) is **not** read by the operation — its
//! presence in the signature is a documentation-level type marker, not a
//! value-level dependency. Pre-fix, `mold_eval.rs:3058` evaluated `raw` and
//! immediately dropped it (`let _raw = ...`), which silently fired any
//! side-effects in the expression tree. That violated the "type-only check"
//! contract documented at `docs/reference/net_api.md §4.4` and was promoted
//! to **Must Fix** by the Phase 0 user verdict (Lock-D Option D2).
//!
//! This test pins the post-fix invariant by passing a side-effecting function
//! call as `raw` and asserting that the side-effect (a `stdout(...)` write)
//! does NOT appear in the interpreter's captured output buffer.
//!
//! # Why a function call rather than an inline block
//!
//! Taida does not expose mutable cells at the surface level, so we observe
//! side-effects via the interpreter's buffered output. A user-defined
//! function that calls `stdout(...)` on entry serves as the side-effect
//! tracer: if the function body executes, the trace marker shows up in
//! `interp.output`; if the eval is skipped, the marker is absent.

use taida::interpreter::Interpreter;
use taida::parser::parse;

/// Helper: parse + run a program in buffered mode and return the joined
/// output as a single String.
fn run_capture(source: &str) -> String {
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "parse errors: {:?}", errors);
    let mut interp = Interpreter::new();
    let result = interp.eval_program(&program);
    assert!(result.is_ok(), "interpreter error: {:?}", result.err());
    interp.output.join("\n")
}

/// Sanity check: the side-effect function actually fires when called
/// directly (i.e. our tracer mechanism works). This isolates the post-fix
/// behaviour test below from "the trace is broken" false positives.
#[test]
fn d29b_007_tracer_fires_when_function_called_directly() {
    let source = r#"
trace b =
  stdout("EVAL_RAW")
  b
=> :Str

raw <= "GET /api HTTP/1.1"
result <= trace(raw)
stdout(result)
"#;
    let out = run_capture(source);
    assert!(
        out.contains("EVAL_RAW"),
        "tracer baseline failed: expected 'EVAL_RAW' in output, got: {out:?}"
    );
}

/// Post-fix invariant (Lock-D D2): `SpanSlice` must NOT evaluate the `raw`
/// argument expression. A side-effecting function passed as `raw` must not
/// fire.
#[test]
fn d29b_007_span_slice_does_not_evaluate_raw_arg() {
    let source = r#"
trace b =
  stdout("EVAL_RAW")
  b
=> :Str

span <= @(start <= 4, len <= 8)
raw <= "GET /api/foo HTTP/1.1"
sub <= SpanSlice[span, trace(raw), 1, 4]()
stdout(sub.start.toString())
stdout(sub.len.toString())
"#;
    let out = run_capture(source);

    // The sub-span arithmetic must still be correct: base (start=4, len=8),
    // sub-bounds (1, 4) -> new_start = 4 + 1 = 5, new_len = 4 - 1 = 3.
    // Mirrors `c26b_016_span_slice_parity` first case (5/3).
    assert!(
        out.contains("5") && out.contains("3"),
        "SpanSlice arithmetic broken (expected start=5,len=3 in output): {out:?}"
    );

    // The decisive check: the side-effect tracer must NOT have fired.
    assert!(
        !out.contains("EVAL_RAW"),
        "D29B-007 regression: SpanSlice evaluated its `raw` argument, firing \
         the tracer side-effect. Lock-D D2 requires `raw` to be skipped \
         (type-only check). Output: {out:?}"
    );
}

/// Edge case: confirm that even repeated `SpanSlice` calls in a tight loop
/// do not fire the tracer. This mirrors the per-request hot-path usage in a
/// router and ensures the eval-skip is structural, not a one-off.
#[test]
fn d29b_007_span_slice_repeated_calls_no_side_effect() {
    let source = r#"
trace b =
  stdout("EVAL_RAW")
  b
=> :Str

span <= @(start <= 0, len <= 8)
raw <= "GET /api/foo HTTP/1.1"
sub1 <= SpanSlice[span, trace(raw), 0, 3]()
sub2 <= SpanSlice[span, trace(raw), 1, 5]()
sub3 <= SpanSlice[span, trace(raw), 2, 7]()
stdout((sub1.start + sub2.start + sub3.start).toString())
"#;
    let out = run_capture(source);

    assert!(
        !out.contains("EVAL_RAW"),
        "D29B-007 regression: repeated SpanSlice calls fired the tracer. \
         Output: {out:?}"
    );
}

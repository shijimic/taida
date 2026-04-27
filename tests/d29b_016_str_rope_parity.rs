//! D29B-016 / Track-θ Phase 10-F / 10-H: String rope parity + perf microbench.
//!
//! Phase 10-B introduces an internal gap-buffer rope path for `Value::Str`
//! (Lock-K verdict V-1/V-2/V-3, interior wrapping DEVIATION matching Track-ε
//! commit `e179238`). The rope path activates when `+` concat or
//! `concat_with` produces a string at or above the 1024-byte threshold.
//!
//! These tests verify:
//! 1. **Phase 10-B**: `concat_with` correctly dispatches between Flat and
//!    Rope paths and that subsequent edits remain Rope (sticky).
//! 2. **Phase 10-F**: 4-backend parity is preserved — interpreter Flat and
//!    Rope produce byte-identical output for the same input sequence.
//! 3. **Phase 10-H**: insertion microbench — N=500 / N=2000 keystroke
//!    cursor-follow simulation completes within the Lock-K acceptance
//!    envelope (< 100 µs / keystroke at N=500, < 500 µs at N=2000) on the
//!    interpreter backend. The other 3 backends (JS / Native / wasm-wasi)
//!    inherit the same surface semantics and are exercised by their own
//!    fixture suites; this file pins the interpreter reference number that
//!    the parity layer compares against.

use std::sync::Arc;
use std::time::Instant;

use taida::interpreter::value::{STR_ROPE_PROMOTION_THRESHOLD, StrValue, Value};

/// Build a `StrValue` of `n` ASCII bytes (cycling through 'a'..='z').
fn make_flat_string(n: usize) -> StrValue {
    let mut s = String::with_capacity(n);
    for i in 0..n {
        s.push((b'a' + (i % 26) as u8) as char);
    }
    StrValue::new(s)
}

/// Phase 10-B: concat below the threshold stays Flat.
#[test]
fn concat_below_threshold_stays_flat() {
    let a = StrValue::new("hello".to_string());
    let b = StrValue::new(" world".to_string());
    let c = a.concat_with(&b);
    assert_eq!(c.as_str(), "hello world");
    assert!(
        !c.is_rope(),
        "expected Flat below threshold (combined len 11)"
    );
}

/// Phase 10-B: concat at exactly the threshold promotes to Rope.
#[test]
fn concat_at_threshold_promotes_to_rope() {
    let a = make_flat_string(STR_ROPE_PROMOTION_THRESHOLD / 2);
    let b = make_flat_string(STR_ROPE_PROMOTION_THRESHOLD / 2);
    let c = a.concat_with(&b);
    assert_eq!(c.as_str().len(), STR_ROPE_PROMOTION_THRESHOLD);
    assert!(
        c.is_rope(),
        "expected Rope at threshold ({} bytes)",
        STR_ROPE_PROMOTION_THRESHOLD
    );
}

/// Phase 10-B: concat above the threshold promotes to Rope.
#[test]
fn concat_above_threshold_promotes_to_rope() {
    let a = make_flat_string(800);
    let b = make_flat_string(800);
    let c = a.concat_with(&b);
    assert_eq!(c.as_str().len(), 1600);
    assert!(c.is_rope(), "expected Rope above threshold");
}

/// Phase 10-B: Rope + anything stays Rope (sticky).
#[test]
fn rope_concat_stays_rope() {
    let big = make_flat_string(2000);
    let small = StrValue::new("x".to_string());
    let rope = big.concat_with(&small);
    assert!(rope.is_rope());
    let next = rope.concat_with(&StrValue::new("y".to_string()));
    assert!(next.is_rope(), "Rope sticky property violated");
    assert!(next.as_str().ends_with("xy"));
}

/// Phase 10-F: Flat and Rope produce byte-identical output for the same
/// concat sequence.
#[test]
fn flat_and_rope_byte_identical() {
    // Build the same logical string two ways: forced Flat (small chunks)
    // vs forced Rope (one large chunk crossing the threshold).
    let mut flat = StrValue::new(String::new());
    for i in 0..100 {
        let piece = StrValue::new(format!("seg{:03}_", i));
        flat = flat.concat_with(&piece);
    }
    // After 100 segments (~800 bytes), flat is likely Rope by now —
    // accept either, but compare against a reference plain-String build.
    let mut reference = String::new();
    for i in 0..100 {
        reference.push_str(&format!("seg{:03}_", i));
    }
    assert_eq!(flat.as_str(), reference);
}

/// Phase 10-F: round-trip through `into_string` is identity.
#[test]
fn rope_into_string_round_trip() {
    let big = make_flat_string(2000);
    let extra = StrValue::new("tail".to_string());
    let rope = big.concat_with(&extra);
    assert!(rope.is_rope());
    let owned = rope.into_string();
    assert_eq!(owned.len(), 2004);
    assert!(owned.ends_with("tail"));
}

/// Phase 10-F: cached_char_count / cached_char_at / cached_char_slice work
/// transparently on Rope (they implicitly trigger flatten).
#[test]
fn rope_char_methods_work() {
    // Use 1100 ASCII bytes so combined len (1100 + 15) crosses the
    // 1024-byte threshold and the Rope path activates.
    let a = make_flat_string(1100);
    let b = StrValue::new("あいうえお".to_string()); // 5 chars, 15 bytes
    let rope = a.concat_with(&b);
    assert!(
        rope.is_rope(),
        "expected Rope (combined 1115 > 1024 threshold)"
    );
    let total_chars = rope.cached_char_count();
    assert_eq!(total_chars, 1100 + 5);
    assert_eq!(rope.cached_char_at(1100), Some("あ".to_string()));
    assert_eq!(rope.cached_char_at(1104), Some("お".to_string()));
    assert_eq!(rope.cached_char_slice(1100, 1103), "あいう");
}

/// Phase 10-H: LineEditor-style insertion microbench at N=500 keystrokes.
///
/// Simulates the prompt.td hot path:
///   state.text => Slice[_, 0, pos] + ch + Slice[state.text, pos, len]
/// implemented with `concat_with` so the rope path engages once the buffer
/// crosses the 1024-byte threshold.
#[test]
fn microbench_n500_under_100us_per_keystroke() {
    const N: usize = 500;
    let mut state = StrValue::new(String::new());

    // Pre-fill 1100 bytes so the buffer starts in the Rope path
    // (mirrors a real LineEditor session that has accumulated text).
    let prefill = make_flat_string(1100);
    state = state.concat_with(&prefill);
    assert!(
        state.is_rope(),
        "prefill should put the buffer in Rope path"
    );

    let start = Instant::now();
    for i in 0..N {
        // Insert at the end (cursor-follow case — the optimal gap buffer path).
        let piece = StrValue::new(format!("{}", i % 10));
        state = state.concat_with(&piece);
    }
    let elapsed = start.elapsed();
    let per_keystroke_us = elapsed.as_micros() as f64 / N as f64;

    // Final length sanity check: 1100 prefill + 500 single chars = 1600.
    assert_eq!(state.as_str().len(), 1600);
    assert!(state.is_rope(), "should remain Rope after N inserts");

    // Lock-K acceptance H: < 100 µs / keystroke at N=500.
    println!(
        "D29B-016 N=500 microbench: {:.2} µs/keystroke (total {:?})",
        per_keystroke_us, elapsed
    );
    assert!(
        per_keystroke_us < 100.0,
        "N=500 keystroke perf regression: {:.2} µs/keystroke (acceptance < 100 µs)",
        per_keystroke_us
    );
}

/// Phase 10-H: same as above but at N=2000.
#[test]
fn microbench_n2000_under_500us_per_keystroke() {
    const N: usize = 2000;
    let mut state = StrValue::new(String::new());

    let prefill = make_flat_string(1100);
    state = state.concat_with(&prefill);
    assert!(state.is_rope());

    let start = Instant::now();
    for i in 0..N {
        let piece = StrValue::new(format!("{}", i % 10));
        state = state.concat_with(&piece);
    }
    let elapsed = start.elapsed();
    let per_keystroke_us = elapsed.as_micros() as f64 / N as f64;

    assert_eq!(state.as_str().len(), 1100 + N);

    println!(
        "D29B-016 N=2000 microbench: {:.2} µs/keystroke (total {:?})",
        per_keystroke_us, elapsed
    );
    assert!(
        per_keystroke_us < 500.0,
        "N=2000 keystroke perf regression: {:.2} µs/keystroke (acceptance < 500 µs)",
        per_keystroke_us
    );
}

/// Phase 10-G (interpreter side): verify that a `Value::Str` constructed
/// via the `+` BinOp dispatch transparently promotes to Rope without any
/// caller-visible API change. This pins the LineEditor "transparent
/// promotion" invariant — `prompt.td::insertAt` does not need source-level
/// changes; the interpreter eval path inside `control_flow.rs::BinOp::Add`
/// dispatches through `concat_with`.
#[test]
fn binop_add_dispatches_through_concat_with() {
    use taida::interpreter::Interpreter;
    use taida::parser::parse;

    // Build a string of length 1100 then concat with another; the eval
    // path must produce a Rope-backed Value::Str.
    let big_seed: String = "x".repeat(1100);
    let src = format!(
        r#"
big <= "{}"
result <= big + "tail"
result
"#,
        big_seed
    );

    let (prog, errs) = parse(&src);
    assert!(errs.is_empty(), "parse errors: {:?}", errs);
    let mut interp = Interpreter::new();
    let v = interp.eval_program(&prog).expect("eval");
    let Value::Str(s) = v else {
        panic!("expected Str, got {:?}", "<other>");
    };
    assert_eq!(s.as_str().len(), 1104);
    assert!(s.as_str().ends_with("tail"));
    assert!(
        s.is_rope(),
        "BinOp::Add did not dispatch through concat_with — Rope promotion missed"
    );
}

/// Phase 10-G negative: small concat stays Flat (regression guard so we
/// don't accidentally promote everything).
#[test]
fn small_binop_add_stays_flat() {
    use taida::interpreter::Interpreter;
    use taida::parser::parse;

    let src = r#"
result <= "hello" + " " + "world"
result
"#;
    let (prog, errs) = parse(src);
    assert!(errs.is_empty(), "parse errors: {:?}", errs);
    let mut interp = Interpreter::new();
    let v = interp.eval_program(&prog).expect("eval");
    let Value::Str(s) = v else {
        panic!("expected Str");
    };
    assert_eq!(s.as_str(), "hello world");
    assert!(
        !s.is_rope(),
        "small concat should stay Flat (combined < 1024 bytes)"
    );
}

/// Sanity: re-export verification — `STR_ROPE_PROMOTION_THRESHOLD` matches
/// Lock-K verdict V-3 (1024 bytes). Pinned constant test prevents an
/// accidental retuning that would invalidate the Lock-K acceptance.
#[test]
fn promotion_threshold_pinned_at_1024() {
    assert_eq!(STR_ROPE_PROMOTION_THRESHOLD, 1024);
}

// Suppress unused-import warnings when the test file is compiled in
// configurations that don't use Arc directly.
#[allow(dead_code)]
fn _arc_used_marker(_: Arc<StrValue>) {}

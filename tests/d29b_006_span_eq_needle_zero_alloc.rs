//! D29B-006 (Phase 2 / Track-α): Span* mold needle zero-allocation guard.
//!
//! Before this fix, `SpanEquals` / `SpanStartsWith` / `SpanContains` evaluated
//! the `needle` (3rd type-arg) into an owned `Vec<u8>` via `Value::str_take`
//! / `Value::bytes_take`. These helpers attempt `Arc::try_unwrap` first and
//! fall back to a deep clone when the Arc is shared. The shared-Arc case is
//! the realistic hot-path scenario: in router code the needle is typically a
//! variable bound at module scope (e.g. `methodGet <= "GET"`) and reused
//! across many requests. Each `SpanEquals[..., methodGet]()` invocation
//! against the bound variable would then `try_unwrap` a refcount > 1 Arc,
//! fall through to the deep-clone branch, and allocate a fresh
//! `Vec<u8>::with_capacity(needle.len()) + memcpy`.
//!
//! That violated contract C (`docs/reference/net_api.md §4.2`: "memcmp 相当 /
//! **zero allocation**"). Track-α rewrites the three Span* sites to borrow
//! `&[u8]` from the inner `Arc<StrValue>` / `Arc<Vec<u8>>` directly, removing
//! the per-call allocation entirely.
//!
//! # Methodology
//!
//! We use a `dhat` heap profiler in testing mode to count allocations. The
//! test parses two byte-similar programs once each and runs them through the
//! interpreter — but the **decisive comparison** is between two programs
//! that drive `SpanEquals` against a **bound variable** (shared-Arc, the
//! pre-fix slow path) with two different needle lengths. If the post-fix
//! borrowing path is taken, the per-byte alloc cost vanishes and the
//! `total_bytes` delta between the two programs is bounded by a constant.
//! Pre-fix the delta would scale linearly with the needle-length
//! difference.

use taida::interpreter::Interpreter;
use taida::parser::parse;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn run(source: &str) {
    let (program, errors) = parse(source);
    assert!(errors.is_empty(), "parse errors: {:?}", errors);
    let mut interp = Interpreter::new();
    let result = interp.eval_program(&program);
    assert!(result.is_ok(), "interpreter error: {:?}", result.err());
}

/// The shared-Arc path of `SpanEquals` (i.e. needle is a bound variable, not
/// an inline literal) must not produce a `Vec<u8>` whose size scales with
/// the needle length. We compare two near-identical programs whose needle
/// length differs by 64 bytes; post-fix the resulting `total_bytes` delta
/// must be far below the per-call linear cost the pre-fix code would incur.
#[test]
fn span_equals_needle_alloc_does_not_scale_with_length() {
    let _profiler = dhat::Profiler::builder().testing().build();

    // Pre-warm: parse + interpret a no-op so any one-shot caches settle.
    run("x <= 1\nstdout(x.toString())\n");

    // Both programs bind `needle` to a literal first, then pass the bound
    // variable to `SpanEquals`. Binding-then-passing forces the inner
    // `Arc<StrValue>` to be shared (refcount >= 2) at the moment
    // `SpanEquals` evaluates its 3rd type-arg, which is exactly the
    // condition under which the pre-fix `Arc::try_unwrap` would fail and
    // fall back to a per-byte deep clone.
    let pad_short = "X"; // 1 byte
    let pad_long = "X".repeat(257); // 257 bytes
    // Pad the *short* source body to match the long source body's lexical
    // length, so that parser-level allocation cost (token strings, AST node
    // sizes for the binding RHS) is approximately equal in both runs. We
    // achieve this by padding with a comment line of equivalent length.
    let comment_pad = "// ".to_string() + &"-".repeat(257 - 1);

    let src_short = format!(
        r#"
{comment_pad}
needle <= "{pad_short}"
raw <= "GET /api"
span <= @(start <= 0, len <= 3)
result <= SpanEquals[span, raw, needle]()
stdout(needle.length().toString())
stdout(result.toString())
"#
    );
    let src_long = format!(
        r#"
needle <= "{pad_long}"
raw <= "GET /api"
span <= @(start <= 0, len <= 3)
result <= SpanEquals[span, raw, needle]()
stdout(needle.length().toString())
stdout(result.toString())
"#
    );

    // Sanity: the two source bodies are roughly the same length (within a
    // small constant for the padding line). This keeps parser-level alloc
    // cost roughly equal in both runs.
    let len_diff = (src_long.len() as isize - src_short.len() as isize).unsigned_abs();
    assert!(
        len_diff < 16,
        "source-length match precondition violated: short={} long={} diff={len_diff}",
        src_short.len(),
        src_long.len()
    );

    let pre_short = dhat::HeapStats::get();
    run(&src_short);
    let post_short = dhat::HeapStats::get();
    let short_bytes = post_short.total_bytes.saturating_sub(pre_short.total_bytes);

    let pre_long = dhat::HeapStats::get();
    run(&src_long);
    let post_long = dhat::HeapStats::get();
    let long_bytes = post_long.total_bytes.saturating_sub(pre_long.total_bytes);

    // Pre-fix: the difference would include at minimum the 256-byte needle
    // payload (the deep-cloned `Vec<u8>` from the shared Arc), plus its
    // header. Post-fix: only the binding-side `Value::str(s.clone())`
    // allocation differs — that's already O(needle.len()) regardless of the
    // SpanEquals path, so we cannot expect a "0 byte" delta. We instead
    // assert the delta is bounded by a small multiple of the needle-length
    // difference (1× for the binding allocation, plus slack), proving the
    // SpanEquals call itself contributes no further per-byte cost.
    let needle_diff = pad_long.len() - pad_short.len(); // 256
    let delta = long_bytes.saturating_sub(short_bytes);

    // Pre-fix bound: each SpanEquals call would add ~256 bytes (the cloned
    // Vec<u8>). On top of the binding allocation (also ~256 bytes), the
    // pre-fix total delta would be >= 2× needle_diff. Post-fix the delta
    // converges to ~1× needle_diff (binding only) plus minor overhead for
    // the longer interpolated source.
    let upper: u64 = (needle_diff as f64 * 1.6) as u64;
    assert!(
        delta < upper,
        "D29B-006 regression: SpanEquals shared-Arc needle path allocates a \
         per-byte copy of the needle. short={short_bytes}B long={long_bytes}B \
         delta={delta}B needle_diff={needle_diff}B upper={upper}B (must be \
         < 1.6× needle_diff; pre-fix would be >= 2× needle_diff). Contract \
         C requires zero-allocation on the Span* hot path."
    );
}

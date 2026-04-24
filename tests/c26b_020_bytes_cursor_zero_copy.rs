//! C26B-020 柱 2 (@c.26, Round 5 wO): Bytes Arc migration zero-copy guard.
//!
//! Value::Bytes was migrated from `Vec<u8>` to `Arc<Vec<u8>>` so that
//! Value::clone() on a Bytes is an atomic refcount increment instead of
//! a full byte-by-byte memcpy. This unblocks `BytesCursorTake` zero-copy
//! hot paths, where GB-scale buffers are threaded through every
//! `take(size)` step.
//!
//! # Acceptance
//!
//! Before Arc migration: each `BytesCursor.take(size)` call invoked
//! `parse_bytes_cursor` which internally `.clone()`d the full buffer
//! (O(N) where N = full buffer size). For a 1 GB buffer sliced into
//! 64 × 16 MB chunks, this was 64 × 1 GB = 64 GB of memcpy just for
//! the refcount.
//!
//! After Arc migration: `parse_bytes_cursor` returns `Arc::clone(&v)`
//! which is O(1) atomic refcount bump. 64 calls × O(1) = O(1) for
//! the cursor-threading. The remaining O(N) per-chunk copy (`to_vec()`
//! of the actual chunk bytes) is O(chunk_size), not O(full_buffer).
//!
//! Target: 1 GB × 64 chunk (= 16 MB per chunk) must complete within
//! 2 seconds on a reasonable laptop. This is conservative: with Arc
//! migration, the total work is ~1 GB memcpy (the chunks themselves);
//! pre-migration would have been ~65 GB.
//!
//! # Layout proof
//!
//! This test also asserts that `Value::Bytes.clone()` produces an
//! `Arc::ptr_eq`-true sibling — demonstrating refcount bump rather
//! than deep copy. This is the *read-side* O(1) invariant.

use std::sync::Arc;
use std::time::Instant;

use taida::interpreter::value::Value;

/// Arc migration read-side invariant: `Value::clone()` on a Bytes must
/// share the underlying buffer via `Arc::ptr_eq`.
#[test]
fn bytes_clone_is_refcount_bump_not_deep_copy() {
    let data = vec![0u8; 1024 * 1024]; // 1 MB
    let v1 = Value::bytes(data);
    let v2 = v1.clone();
    // Both should point to the same Arc allocation.
    match (&v1, &v2) {
        (Value::Bytes(a), Value::Bytes(b)) => {
            assert!(
                Arc::ptr_eq(a, b),
                "Value::clone() on Bytes must be Arc::clone (refcount bump), \
                 not a deep memcpy. C26B-020 柱 2 requires zero-copy read."
            );
        }
        _ => panic!("expected Value::Bytes on both"),
    }
}

/// Stress test: simulate BytesCursorTake hot path over a large buffer
/// with many cursor-threading steps. Must complete in < 2 seconds.
///
/// Kept at 256 MB × 16 chunks (= 16 MB each) for CI. The parametric
/// scaling to 1 GB × 64 is exercised locally via the `TAIDA_BIG_BYTES=1`
/// environment variable, matching the original acceptance criterion.
#[test]
fn bytes_cursor_threading_is_bounded_time() {
    let (total_size, num_chunks) = if std::env::var("TAIDA_BIG_BYTES").is_ok() {
        (1024 * 1024 * 1024usize, 64) // 1 GB × 64 chunks (acceptance)
    } else {
        (256 * 1024 * 1024usize, 16) // 256 MB × 16 chunks (CI-safe)
    };
    let chunk_size = total_size / num_chunks;

    // Build initial Bytes.
    let data = vec![0u8; total_size];
    let initial = Value::bytes(data);

    let start = Instant::now();

    // Simulate cursor-threading: clone the Value N times (each clone is
    // the refcount bump), and peek at each chunk range.
    let mut current_offset: usize = 0;
    let mut total_read: usize = 0;
    for _ in 0..num_chunks {
        // This mirrors what parse_bytes_cursor does internally: Arc::clone
        // of the inner buffer + index into a chunk.
        let clone = initial.clone();
        if let Value::Bytes(arc) = clone {
            // Chunk access via Arc deref (no copy).
            let end = (current_offset + chunk_size).min(arc.len());
            let chunk_slice = &arc[current_offset..end];
            total_read += chunk_slice.len();
            current_offset = end;
        } else {
            panic!("Value::Bytes clone lost its variant");
        }
    }

    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis();

    // Read amount must match the full buffer (chunks are contiguous).
    assert_eq!(total_read, total_size);

    // Timing ceiling: the cursor-threading with Arc refcount bumps must
    // be dominated by whatever constant-time bookkeeping (not by buffer
    // copies). 2 seconds is a generous ceiling for both CI-safe and big
    // variants: the pre-Arc deep-copy path would have taken 16-64x longer.
    let ceiling_ms = if total_size >= 1 << 30 { 2000 } else { 500 };
    assert!(
        elapsed_ms < ceiling_ms,
        "BytesCursor threading over {} MB × {} chunks took {} ms \
         (ceiling {} ms). Arc::clone on Value::Bytes must be O(1).",
        total_size / (1024 * 1024),
        num_chunks,
        elapsed_ms,
        ceiling_ms,
    );
    eprintln!(
        "C26B-020 柱 2 threading: {} MB × {} chunks = {} ms (ceiling {} ms)",
        total_size / (1024 * 1024),
        num_chunks,
        elapsed_ms,
        ceiling_ms
    );
}

/// COW write-side invariant: when `bytes_take` is called on a uniquely
/// owned `Arc`, it returns the inner `Vec<u8>` without allocation via
/// `Arc::try_unwrap` fast path. When the Arc is shared, it deep-clones.
///
/// This exercises the `ByteSet` / `Concat` / `Utf8Decode` hot paths
/// that previously took `Vec<u8>` by move.
#[test]
fn bytes_take_is_cow_fast_path_when_unique() {
    // Unique ownership: bytes_take should unwrap without allocating.
    let v = Value::bytes(vec![1, 2, 3, 4]);
    if let Value::Bytes(arc) = v {
        // Capture the pointer before unwrap.
        let before_ptr = arc.as_ptr();
        let out = Value::bytes_take(arc);
        // Same backing allocation (try_unwrap returned the original Vec).
        assert_eq!(out.as_ptr(), before_ptr);
        assert_eq!(out, vec![1, 2, 3, 4]);
    } else {
        panic!("expected Value::Bytes");
    }

    // Shared ownership: bytes_take must deep-clone.
    let v1 = Value::bytes(vec![5, 6, 7]);
    let v2 = v1.clone();
    if let (Value::Bytes(a), Value::Bytes(b)) = (v1, v2) {
        assert!(Arc::ptr_eq(&a, &b));
        let out_a = Value::bytes_take(a);
        // `b` is still alive, so `out_a` is a deep-clone. Content matches.
        assert_eq!(out_a, vec![5, 6, 7]);
        // `b` still points to the original allocation (unchanged).
        assert_eq!(&**b, &[5u8, 6, 7]);
    } else {
        panic!("expected Value::Bytes on both");
    }
}

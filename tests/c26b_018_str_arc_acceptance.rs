//! C26B-018 (A) / Round 6 wP acceptance tests: `Value::Str` interior
//! `Arc<String>` migration (COW foundation).
//!
//! These tests demonstrate that `Value::Str.clone()` is an `Arc::clone`
//! (refcount bump, O(1)) rather than a deep-copy of the underlying
//! String bytes. They parallel `tests/c26b_020_bytes_cursor_zero_copy.rs`
//! (wO Bytes migration) and are the read-side half of the Cluster 4
//! Arc + try_unwrap COW abstraction (Round 3 wG LOCKED).
//!
//! The char-index cache optimization goal for C26B-018 (A) is tracked
//! as a follow-up; this iteration lands the interior Arc foundation so
//! clone-heavy hot paths (Str in BuchiPack copies, `req.method.clone()`,
//! closure env captures, match-binding clones) stop deep-copying.

use std::sync::Arc;
use taida::interpreter::value::Value;

/// `Value::clone()` on a `Value::Str` must be an atomic refcount bump,
/// not a deep-copy of the inner `String`. The two `Arc<String>` payloads
/// should be pointer-equal (`Arc::ptr_eq`).
#[test]
fn str_clone_is_refcount_bump_not_deep_copy() {
    // Use a large String so a deep-copy would be observable as a distinct
    // allocation.
    let payload: String = "taida-lang-c26-round6-wp-".repeat(4096); // ~100 KB
    let original = Value::str(payload);
    let shared = original.clone();

    // Both handles must carry the *same* heap allocation — the Arc was
    // cloned (refcount + 1), not the String.
    let (a, b) = match (&original, &shared) {
        (Value::Str(a), Value::Str(b)) => (a, b),
        _ => panic!("expected both handles to be Value::Str"),
    };

    assert!(
        Arc::ptr_eq(a, b),
        "Value::clone on Value::Str must share the underlying Arc<String> (pointer-equal)"
    );
    assert_eq!(a.as_str(), b.as_str());
}

/// A 10 000-element list of `Value::Str` clones must all share the same
/// underlying allocation. If deep-copy leaked in (e.g. someone reintroduced
/// `(**s).clone()` in a hot path), the heap would blow up proportionally;
/// under Arc COW, the 10k clones collectively cost one String allocation.
#[test]
fn str_clone_scales_as_refcount_not_heap_alloc() {
    let payload: String = "X".repeat(1024);
    let original = Value::str(payload);
    let handles: Vec<Value> = (0..10_000).map(|_| original.clone()).collect();
    let Value::Str(base_arc) = &original else {
        panic!("original must be Str");
    };
    for (i, h) in handles.iter().enumerate() {
        let Value::Str(other) = h else {
            panic!("handle {} lost its Str shape", i);
        };
        assert!(
            Arc::ptr_eq(base_arc, other),
            "handle {} is not Arc-shared with original",
            i
        );
    }
    // Arc strong count includes: original (1) + 10_000 handles = 10_001.
    assert_eq!(Arc::strong_count(base_arc), 10_001);
}

/// `Value::str_take` takes ownership of the inner `String` via
/// `Arc::try_unwrap` when unique; else clones. Verify both paths.
#[test]
fn str_take_is_cow_fast_path_when_unique() {
    // Unique case: no other Arc refs, try_unwrap succeeds without clone.
    let v = Value::str("unique-string".to_string());
    let Value::Str(arc) = v else {
        panic!("expected Str");
    };
    assert_eq!(Arc::strong_count(&arc), 1);
    let taken = Value::str_take(arc);
    assert_eq!(taken, "unique-string");

    // Shared case: try_unwrap fails, falls back to (*arc).clone().
    // The returned String must still equal the shared content.
    let original = Value::str("shared-string".to_string());
    let hold = original.clone(); // keep Arc alive, refcount=2
    let Value::Str(arc2) = original else {
        panic!("expected Str");
    };
    assert!(Arc::strong_count(&arc2) >= 2);
    let taken2 = Value::str_take(arc2);
    assert_eq!(taken2, "shared-string");

    // Holding handle is still intact and still reads the same content.
    let Value::Str(holder) = &hold else {
        panic!("hold must still be Str");
    };
    assert_eq!(holder.as_str(), "shared-string");
}

/// Value::str constructor yields a fresh Arc with strong_count == 1.
#[test]
fn str_constructor_starts_with_unique_refcount() {
    let v = Value::str("fresh".to_string());
    let Value::Str(arc) = v else {
        panic!("expected Str");
    };
    assert_eq!(Arc::strong_count(&arc), 1);
}

/// Display / Debug / equality / ordering semantics must be preserved
/// after the Arc migration — Arc<T> forwards read traits transparently.
#[test]
fn str_surface_semantics_preserved_under_arc() {
    let a = Value::str("hello".to_string());
    let b = Value::str("hello".to_string());
    let c = Value::str("world".to_string());

    // Equality
    assert_eq!(a, b);
    assert_ne!(a, c);

    // Ordering
    assert!(a < c);
    assert!(c > a);

    // Display
    assert_eq!(a.to_string(), "hello");

    // Truthiness (empty Str is false)
    assert!(a.is_truthy());
    assert!(!Value::default_str().is_truthy());
}

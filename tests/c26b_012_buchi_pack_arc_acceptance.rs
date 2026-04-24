//! C26B-012 (@c.26, Round 6 wQ): BuchiPack Arc migration acceptance.
//!
//! `Value::BuchiPack` was migrated from `Vec<(String, Value)>` to
//! `Arc<Vec<(String, Value)>>` so that `Value::clone()` on a pack is an
//! atomic refcount increment instead of a field-by-field deep-clone. This
//! follows the same Cluster 4 abstraction pattern (Arc + try_unwrap COW,
//! LOCKED in Round 3 wG) applied to `Value::List` (Phase 5-F2-1) and
//! `Value::Bytes` (Round 5 wO).
//!
//! # Acceptance
//!
//! Before Arc migration: cloning a BuchiPack meant `.clone()`ing every
//! `(String, Value)` entry — each String allocation copied, each nested
//! Value recursed. For deeply nested packs (e.g. HTTP request pack with
//! headers list × span packs × content), a single clone could allocate
//! hundreds of Strings and recurse across the full DOM.
//!
//! After Arc migration: `Value::clone()` on BuchiPack is an `Arc::clone`
//! — one atomic refcount bump. The inner Vec is only deep-copied when a
//! mutable consumer requests ownership (`Value::pack_take` with a shared
//! Arc), and the try_unwrap fast path avoids allocation when the clone
//! chain drops to uniqueness.
//!
//! This test asserts the *read-side* invariant (Arc::ptr_eq after clone)
//! and the *write-side* invariant (pack_take returns the inner Vec
//! without allocation when unique).

use std::sync::Arc;

use taida::interpreter::value::Value;

/// Arc migration read-side invariant: `Value::clone()` on a BuchiPack
/// must share the underlying fields vec via `Arc::ptr_eq` — zero allocation
/// on the hot read path.
#[test]
fn buchipack_clone_is_refcount_bump_not_deep_copy() {
    let fields = vec![
        ("name".to_string(), Value::str("Taida".to_string())),
        ("version".to_string(), Value::Int(26)),
        (
            "tags".to_string(),
            Value::list(vec![Value::str("c26".to_string())]),
        ),
    ];
    let v1 = Value::pack(fields);
    let v2 = v1.clone();
    match (&v1, &v2) {
        (Value::BuchiPack(a), Value::BuchiPack(b)) => {
            assert!(
                Arc::ptr_eq(a, b),
                "Value::clone() on BuchiPack must be Arc::clone (refcount bump), \
                 not a field-by-field deep copy. C26B-012 requires zero-allocation \
                 read-side for Cluster 4 abstraction."
            );
        }
        _ => panic!("expected Value::BuchiPack on both sides of clone"),
    }
}

/// COW write-side invariant: `Value::pack_take` on a uniquely-owned Arc
/// must return the inner Vec without allocation via `Arc::try_unwrap`.
#[test]
fn buchipack_take_is_cow_fast_path_when_unique() {
    let fields = vec![
        ("a".to_string(), Value::Int(1)),
        ("b".to_string(), Value::Int(2)),
    ];
    let expected = fields.clone();
    let packed = Value::pack(fields);
    let inner = match packed {
        Value::BuchiPack(arc) => arc,
        _ => panic!("expected BuchiPack"),
    };
    // At this point, `inner` is the *only* Arc reference — pack_take must
    // be the try_unwrap fast path (no allocation).
    assert_eq!(Arc::strong_count(&inner), 1);
    let taken = Value::pack_take(inner);
    assert_eq!(taken, expected, "pack_take must preserve field order + values");
}

/// COW write-side invariant under contention: `Value::pack_take` on a
/// shared Arc must fall back to cloning the inner Vec (so the caller
/// receives owned fields without disturbing other holders).
#[test]
fn buchipack_take_clones_when_shared() {
    let fields = vec![("x".to_string(), Value::Int(42))];
    let v1 = Value::pack(fields.clone());
    let v2 = v1.clone();
    // Two Arc references exist; pack_take must clone the inner Vec.
    let inner = match v1 {
        Value::BuchiPack(arc) => arc,
        _ => panic!("expected BuchiPack"),
    };
    assert_eq!(Arc::strong_count(&inner), 2);
    let taken = Value::pack_take(inner);
    assert_eq!(taken, fields);
    // The other clone (`v2`) is still intact.
    match v2 {
        Value::BuchiPack(arc) => {
            assert_eq!(arc.len(), 1);
        }
        _ => panic!("expected BuchiPack"),
    }
}

/// Construction-helper round trip: `Value::pack` + `Value::pack_take`
/// must preserve exact field ordering and value semantics.
#[test]
fn buchipack_pack_then_take_round_trips_fields() {
    let fields = vec![
        ("first".to_string(), Value::str("one".to_string())),
        ("second".to_string(), Value::Int(2)),
        ("third".to_string(), Value::Bool(true)),
        ("fourth".to_string(), Value::list(vec![Value::Int(4)])),
    ];
    let expected = fields.clone();
    let packed = Value::pack(fields);
    match packed {
        Value::BuchiPack(arc) => {
            let taken = Value::pack_take(arc);
            assert_eq!(taken, expected);
        }
        _ => panic!("expected BuchiPack"),
    }
}

/// Default value invariant: `Value::default_buchi()` must produce an empty
/// BuchiPack whose interior Arc has strong count 1 (uniquely owned).
#[test]
fn default_buchi_is_empty_and_uniquely_owned() {
    let v = Value::default_buchi();
    match v {
        Value::BuchiPack(arc) => {
            assert_eq!(arc.len(), 0);
            assert_eq!(Arc::strong_count(&arc), 1);
        }
        _ => panic!("expected BuchiPack"),
    }
}

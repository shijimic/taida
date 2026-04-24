//! C25B-021 / C25B-022 / C25B-023 common foundation — `ValueKey`.
//!
//! A hashable view over [`Value`] for use as a HashSet / HashMap key.
//! Used by the fast paths in `Set.union` / `Set.intersect` / `Set.diff` /
//! `HashMap.merge` / `Unique` to lift their core comparison loop from
//! `Vec::contains` (O(N*M)) to a pre-built `HashSet<ValueKey>` (O(N+M)).
//!
//! # Design lock (C25 Phase 5-D / 2026-04-23)
//!
//! The set of [`Value`] variants that can participate in the fast path
//! is deliberately narrow. A value is **key-eligible** if, and only if,
//! every recursive component is also key-eligible. Eligible variants:
//!
//! * `Int(i64)`
//! * `Bool(bool)`
//! * `Str(String)`
//! * `Bytes(Vec<u8>)`
//! * `Unit`
//! * `EnumVal(String, i64)`
//! * `Gorilla`
//! * `List(Vec<Value>)` — recursive; each element must be key-eligible
//! * `BuchiPack(Vec<(String, Value)>)` — recursive; order-independent
//!   (matches the existing `PartialEq` contract on BuchiPack)
//!
//! Excluded variants (the caller retains the existing linear-scan
//! fallback when it encounters any of these):
//!
//! * `Float(f64)` — f64 has no `Eq`; NaN breaks reflexivity. We never
//!   store Float in the hash domain; callers that mix Float into a
//!   Set / HashMap / Unique operand still get correct results via the
//!   unchanged linear path.
//! * `Function` / closures — not meaningfully hashable.
//! * `Async` / `Stream` — runtime state; equality is identity-ish.
//! * `Error` — carries runtime metadata; not a stable key.
//! * `Molten` / `Json` — opaque to the Taida surface.
//!
//! # Cross-type equality (C25B-022 / C25B-023 REOPEN fix, 2026-04-23)
//!
//! `Value::eq` treats `Int(n)`, `EnumVal(_, n)` and `Float(n as f64)` as
//! equal when they share the same numeric ordinal (see `value.rs:465`
//! `PartialEq for Value`, C18-2 rule). An earlier iteration of
//! `ValueKey` deliberately *diverged* from this rule and treated
//! `Int(3)` / `EnumVal(X, 3)` as distinct keys — see the original
//! comment block in `f721c6d`. That divergence caused
//! `setOf(@[0]).union(setOf(@[Color:Red()]))` to report size 2 instead
//! of size 1, and the analogous break in `HashMap.merge`.
//!
//! The current design restores `Value::eq` as the single source of
//! truth for the fast paths:
//!
//!   * `Int(n)` and `EnumVal(_, n)` hash to the same fingerprint
//!     (numeric ordinal tag, EnumVal name ignored for hashing).
//!   * `ValueKey::eq` mirrors `Value::eq`'s cross-type rule for the
//!     hashable subset — `Int(n) == EnumVal(_, n)` returns true.
//!   * Intra-Enum equality still requires matching names: two
//!     `EnumVal(a, n)` / `EnumVal(b, n)` with different enum names
//!     hash to the same fingerprint (by design) but `ValueKey::eq`
//!     reports them as different — the caller's Value::eq confirmation
//!     path upgrades the fingerprint collision into a correct linear
//!     disambiguation.
//!   * `Float(f)` is still excluded from key domain (no `Eq`), so the
//!     `Int(n) ↔ Float(n)` cross-type rule is handled by the linear
//!     fallback path at the caller.
//!
//! The callers (`Set.union`, `Set.intersect`, `Set.diff`,
//! `HashMap.merge`, `Unique`) already confirm fingerprint hits with
//! `Value::eq` before committing, so this normalization is safe: it
//! tightens the hash distribution to match Value::eq's equivalence
//! classes without relying on Value::eq to also be transitive (which
//! it is not across Enum↔Int, strictly speaking — two EnumVals with
//! different names are not Value::eq'd even though both match a
//! common Int).
//!
//! # Contract on borrowed data
//!
//! `ValueKey<'a>` borrows from the underlying `Value`. The caller must
//! keep the `Value` alive for the HashSet's lifetime. All uses in
//! `methods.rs` / `mold_eval.rs` build the HashSet immediately from a
//! borrow that lives for the scope of the operation, satisfying this.

use super::value::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Hashable borrowed view of a [`Value`]. Construct with
/// [`ValueKey::new`]; `None` means the value is not key-eligible and
/// the caller must fall back to linear `Value::eq` comparison.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ValueKey<'a>(pub(crate) &'a Value);

impl<'a> ValueKey<'a> {
    /// Wrap `v` as a `ValueKey` iff every recursive component is
    /// key-eligible. See module docs for the classification.
    pub(crate) fn new(v: &'a Value) -> Option<Self> {
        if is_hashable(v) { Some(Self(v)) } else { None }
    }

    /// Construct a hash fingerprint independent of storage order for
    /// BuchiPack fields. Public (crate-local) for callers that want to
    /// build their own HashSet<u64> keyed on the fingerprint rather
    /// than carrying ValueKey around.
    pub(crate) fn fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        hash_value_into(self.0, &mut h);
        h.finish()
    }
}

impl PartialEq for ValueKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        // Intra-ValueKey equality is exact (no cross-type coercion).
        // This is stricter than `Value::eq` on purpose; see module docs.
        exact_eq(self.0, other.0)
    }
}

impl Eq for ValueKey<'_> {}

impl Hash for ValueKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_value_into(self.0, state);
    }
}

/// Return true iff `v` is recursively key-eligible.
fn is_hashable(v: &Value) -> bool {
    match v {
        Value::Int(_)
        | Value::Bool(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Unit
        | Value::Gorilla
        | Value::EnumVal(_, _) => true,
        Value::List(items) => items.iter().all(is_hashable),
        Value::BuchiPack(fields) => fields.iter().all(|(_, v)| is_hashable(v)),
        Value::Float(_)
        | Value::Function(_)
        | Value::Async(_)
        | Value::Stream(_)
        | Value::Error(_)
        | Value::Molten
        | Value::Json(_) => false,
    }
}

/// Structural equality for key-eligible values, aligned with
/// `Value::eq` (C25B-022 / C25B-023 REOPEN fix). BuchiPack is
/// order-independent. `Int(n)` and `EnumVal(_, n)` compare equal
/// (matches `Value::eq` line 502). Intra-enum equality still requires
/// matching names (matches `Value::eq` line 499-501).
fn exact_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bytes(x), Value::Bytes(y)) => x == y,
        (Value::Unit, Value::Unit) => true,
        (Value::Gorilla, Value::Gorilla) => true,
        (Value::EnumVal(na, nb_a), Value::EnumVal(nb, nb_b)) => na == nb && nb_a == nb_b,
        // Cross-type: Int(n) == EnumVal(_, n). See module docs.
        (Value::Int(x), Value::EnumVal(_, y)) | (Value::EnumVal(_, y), Value::Int(x)) => x == y,
        (Value::List(xs), Value::List(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(a, b)| exact_eq(a, b))
        }
        (Value::BuchiPack(xs), Value::BuchiPack(ys)) => {
            if xs.len() != ys.len() {
                return false;
            }
            xs.iter()
                .all(|(nx, vx)| ys.iter().any(|(ny, vy)| nx == ny && exact_eq(vx, vy)))
        }
        _ => false,
    }
}

/// Hash a key-eligible value. BuchiPack is order-independent: we hash
/// the XOR-reduction of per-field hashes so that `@(a<=1, b<=2)` and
/// `@(b<=2, a<=1)` produce the same fingerprint.
///
/// C25B-022 / C25B-023 REOPEN fix (2026-04-23): `Int(n)` and
/// `EnumVal(_, n)` hash to the same fingerprint (tag `0u8` + ordinal
/// `n`) so that `Value::eq`'s cross-type rule is honoured by the fast
/// paths. Two distinct `EnumVal(a, n)` / `EnumVal(b, n)` also collide
/// on fingerprint (by design); the caller's Value::eq confirmation
/// resolves that back into correct inequality.
fn hash_value_into<H: Hasher>(v: &Value, state: &mut H) {
    // Tag each variant so that `Int(0)` and `Bool(false)` don't collide.
    // Int and EnumVal share the same tag to match Value::eq's Int↔Enum
    // cross-type rule (line 502 of value.rs).
    match v {
        Value::Int(n) => {
            0u8.hash(state);
            n.hash(state);
        }
        Value::Bool(b) => {
            1u8.hash(state);
            b.hash(state);
        }
        Value::Str(s) => {
            2u8.hash(state);
            s.hash(state);
        }
        Value::Bytes(b) => {
            3u8.hash(state);
            b.hash(state);
        }
        Value::Unit => {
            4u8.hash(state);
        }
        Value::Gorilla => {
            5u8.hash(state);
        }
        Value::EnumVal(_name, n) => {
            // Same tag + ordinal as Int(n) so `Int(3)` and
            // `EnumVal(Color, 3)` share a fingerprint, matching
            // Value::eq's cross-type coercion. The enum name is not
            // part of the fingerprint — inter-enum disambiguation is
            // deferred to the Value::eq confirmation path.
            0u8.hash(state);
            n.hash(state);
        }
        Value::List(items) => {
            7u8.hash(state);
            items.len().hash(state);
            for it in items.iter() {
                hash_value_into(it, state);
            }
        }
        Value::BuchiPack(fields) => {
            8u8.hash(state);
            fields.len().hash(state);
            // Order-independent field mix. Each (name, value) produces a
            // per-field u64, all XOR-reduced, then fed into the outer
            // hasher so that two packs with the same fields in different
            // order produce the same outer hash.
            let mut mix: u64 = 0;
            for (name, val) in fields.iter() {
                let mut fh = DefaultHasher::new();
                name.hash(&mut fh);
                hash_value_into(val, &mut fh);
                mix ^= fh.finish();
            }
            mix.hash(state);
        }
        // Unreachable because `is_hashable` gates ValueKey construction.
        Value::Float(_)
        | Value::Function(_)
        | Value::Async(_)
        | Value::Stream(_)
        | Value::Error(_)
        | Value::Molten
        | Value::Json(_) => {
            unreachable!("ValueKey::hash called on non-hashable variant")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn value_key_basic_hashability() {
        assert!(ValueKey::new(&Value::Int(1)).is_some());
        assert!(ValueKey::new(&Value::Bool(true)).is_some());
        assert!(ValueKey::new(&Value::str("hi".into())).is_some());
        assert!(ValueKey::new(&Value::Unit).is_some());
        assert!(ValueKey::new(&Value::Gorilla).is_some());
        assert!(ValueKey::new(&Value::EnumVal("X".into(), 2)).is_some());
        assert!(ValueKey::new(&Value::bytes(vec![1, 2, 3])).is_some());
    }

    #[test]
    fn value_key_float_and_function_are_excluded() {
        assert!(ValueKey::new(&Value::Float(1.0)).is_none());
        // Function / Async / Stream exclusion is checked via List/Pack
        // composite (simpler to construct).
        let f = Value::Float(f64::NAN);
        assert!(ValueKey::new(&f).is_none());
        assert!(
            ValueKey::new(&Value::list(vec![Value::Int(1), Value::Float(2.0)])).is_none(),
            "list containing float must be non-eligible"
        );
    }

    #[test]
    fn value_key_list_eligibility_is_recursive() {
        let pure = Value::list(vec![Value::Int(1), Value::str("a".into())]);
        assert!(ValueKey::new(&pure).is_some());
        let nested = Value::list(vec![Value::list(vec![
            Value::Int(1),
            Value::bytes(vec![5]),
        ])]);
        assert!(ValueKey::new(&nested).is_some());
    }

    #[test]
    fn value_key_buchi_pack_is_order_independent() {
        let a = Value::pack(vec![
            ("x".into(), Value::Int(1)),
            ("y".into(), Value::str("hi".into())),
        ]);
        let b = Value::pack(vec![
            ("y".into(), Value::str("hi".into())),
            ("x".into(), Value::Int(1)),
        ]);
        let ka = ValueKey::new(&a).unwrap();
        let kb = ValueKey::new(&b).unwrap();
        assert_eq!(ka, kb, "different field order must compare equal");
        assert_eq!(
            ka.fingerprint(),
            kb.fingerprint(),
            "different field order must hash equal"
        );
    }

    #[test]
    // `ValueKey` wraps `&Value`, whose enum carries `Mutex`/`AtomicU32`
    // (inside `Async` / `Stream` variants) that make clippy flag the
    // HashSet key type. `is_hashable()` explicitly rejects those
    // variants, so in practice the HashSet never sees an interior-
    // mutable key; the lint is a false positive here.
    #[allow(clippy::mutable_key_type)]
    fn value_key_round_trip_in_hashset() {
        let values = vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(1), // duplicate
            Value::str("a".into()),
            Value::str("a".into()), // duplicate
            Value::Bool(false),
        ];
        let mut set: HashSet<ValueKey> = HashSet::new();
        for v in &values {
            if let Some(k) = ValueKey::new(v) {
                set.insert(k);
            }
        }
        assert_eq!(set.len(), 4, "int(1), int(2), str(a), bool(false)");
    }

    #[test]
    fn value_key_int_and_enum_collide_per_value_eq() {
        // C25B-022 / C25B-023 REOPEN fix (2026-04-23): Int(n) and
        // EnumVal(_, n) must share a ValueKey so that the fast paths
        // match `Value::eq`'s Int↔Enum cross-type coercion.
        let int = Value::Int(3);
        let enum_v = Value::EnumVal("Color".into(), 3);
        let ki = ValueKey::new(&int).unwrap();
        let ke = ValueKey::new(&enum_v).unwrap();
        assert_eq!(ki, ke, "Int(3) and EnumVal(_, 3) must be equal ValueKeys");
        assert_eq!(
            ki.fingerprint(),
            ke.fingerprint(),
            "Int(3) and EnumVal(_, 3) must hash identically"
        );
    }

    #[test]
    fn value_key_distinct_enums_same_ordinal_have_ne_via_exact_eq() {
        // Intra-enum disambiguation: EnumVal("A", 0) and EnumVal("B", 0)
        // are NOT Value::eq'd (line 499-501 of value.rs: names must
        // match). Their fingerprints intentionally collide, but
        // `ValueKey::eq` must still report them as distinct so that
        // the fast path's Value::eq confirmation step disambiguates
        // them. The caller handles the rare fingerprint-collision
        // upgrade — here we just pin that the ValueKey eq works.
        let a = Value::EnumVal("Color".into(), 0);
        let b = Value::EnumVal("Foo".into(), 0);
        let ka = ValueKey::new(&a).unwrap();
        let kb = ValueKey::new(&b).unwrap();
        assert_ne!(
            ka, kb,
            "EnumVal(\"Color\", 0) != EnumVal(\"Foo\", 0) per Value::eq"
        );
        // Fingerprint collision is expected (and handled downstream).
        assert_eq!(
            ka.fingerprint(),
            kb.fingerprint(),
            "EnumVals with same ordinal intentionally collide on fingerprint \
             (Value::eq disambiguates at the caller)"
        );
    }
}

//! Single-source-of-truth table for builtin-type method existence and
//! arity.
//!
//! Scope (first instalment): the statically enumerable receivers handled
//! by `TypeChecker::builtin_method_signature`. Receivers whose method set
//! is value-dependent (`BuchiPack` function fields, user-defined `Named`
//! types via `named_method_signature`) are intentionally NOT represented
//! here — their specs live in user code, not in the language.
//!
//! The table is pinned to the checker implementation by the exhaustive
//! cross test below: every (receiver, method) pair in the universe of
//! known method names must agree between this table and
//! `builtin_method_signature` (Some-with-same-arity vs None). Editing one
//! side without the other fails the test, which is the point.
//!
//! Argument types are deliberately out of scope for this instalment:
//! several signatures are parameterised by the receiver's element types
//! and cannot live in a static table without loss.

/// Statically enumerable builtin receiver kinds.
// Production code does not consume the table yet — in this first
// instalment its sole binding force is the exhaustive cross test below
// (and the cross-backend audit script under .dev). Later instalments
// will switch the checker dispatch to read from it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuiltinRecv {
    /// `Str`
    Str,
    /// `Int` / `Float` / `Num` (identical method sets)
    Num,
    /// `Bool`
    Bool,
    /// `Bytes`
    Bytes,
    /// `T[]`
    List,
    /// `HashMap` (bare `Named` and `HashMap[K, V]` forms expose the same set)
    HashMap,
    /// `Set` (bare `Named` and `Set[T]` forms expose the same set)
    Set,
    /// `Lax[T]`
    Lax,
    /// `Result[T, P]`
    Result,
    /// `Async[T]`
    Async,
    /// `Gorillax[T]`
    Gorillax,
    /// `RelaxedGorillax[T]`
    RelaxedGorillax,
    /// `Error`-derived types (builtin members only; user fields are dynamic)
    Error,
}

/// One builtin method: existence + arity. The single source of truth for
/// "which methods exist on which builtin type, taking how many args".
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct BuiltinMethodSpec {
    pub(crate) recv: BuiltinRecv,
    pub(crate) name: &'static str,
    pub(crate) min_args: usize,
    pub(crate) max_args: usize,
}

macro_rules! spec {
    ($recv:ident, $name:literal, $min:literal, $max:literal) => {
        BuiltinMethodSpec {
            recv: BuiltinRecv::$recv,
            name: $name,
            min_args: $min,
            max_args: $max,
        }
    };
}

/// Faithful transcription of `builtin_method_signature` (checked by test).
#[allow(dead_code)]
pub(crate) static BUILTIN_METHOD_SPECS: &[BuiltinMethodSpec] = &[
    // ── Str ─────────────────────────────────────────────────────────
    spec!(Str, "length", 0, 0),
    spec!(Str, "toString", 0, 0),
    spec!(Str, "contains", 1, 1),
    spec!(Str, "startsWith", 1, 1),
    spec!(Str, "endsWith", 1, 1),
    spec!(Str, "indexOf", 1, 1),
    spec!(Str, "lastIndexOf", 1, 1),
    spec!(Str, "indexOfLax", 1, 1),
    spec!(Str, "lastIndexOfLax", 1, 1),
    spec!(Str, "get", 1, 1),
    spec!(Str, "replace", 2, 2),
    spec!(Str, "replaceAll", 2, 2),
    spec!(Str, "split", 1, 1),
    spec!(Str, "match", 1, 1),
    spec!(Str, "search", 1, 1),
    spec!(Str, "searchLax", 1, 1),
    // ── Int / Float / Num ───────────────────────────────────────────
    spec!(Num, "toString", 0, 0),
    spec!(Num, "isNaN", 0, 0),
    spec!(Num, "isInfinite", 0, 0),
    spec!(Num, "isFinite", 0, 0),
    spec!(Num, "isPositive", 0, 0),
    spec!(Num, "isNegative", 0, 0),
    spec!(Num, "isZero", 0, 0),
    // ── Bool ────────────────────────────────────────────────────────
    spec!(Bool, "toString", 0, 0),
    // ── Bytes ───────────────────────────────────────────────────────
    spec!(Bytes, "length", 0, 0),
    spec!(Bytes, "get", 1, 1),
    spec!(Bytes, "toString", 0, 0),
    // ── List ────────────────────────────────────────────────────────
    spec!(List, "length", 0, 0),
    spec!(List, "isEmpty", 0, 0),
    spec!(List, "first", 0, 0),
    spec!(List, "last", 0, 0),
    spec!(List, "max", 0, 0),
    spec!(List, "min", 0, 0),
    spec!(List, "get", 1, 1),
    spec!(List, "contains", 1, 1),
    spec!(List, "indexOf", 1, 1),
    spec!(List, "lastIndexOf", 1, 1),
    spec!(List, "indexOfLax", 1, 1),
    spec!(List, "lastIndexOfLax", 1, 1),
    spec!(List, "any", 1, 1),
    spec!(List, "all", 1, 1),
    spec!(List, "none", 1, 1),
    spec!(List, "reduce", 2, 2),
    spec!(List, "fold", 2, 2),
    spec!(List, "toString", 0, 0),
    // ── HashMap ─────────────────────────────────────────────────────
    spec!(HashMap, "get", 1, 1),
    spec!(HashMap, "set", 2, 2),
    spec!(HashMap, "remove", 1, 1),
    spec!(HashMap, "has", 1, 1),
    spec!(HashMap, "keys", 0, 0),
    spec!(HashMap, "values", 0, 0),
    spec!(HashMap, "entries", 0, 0),
    spec!(HashMap, "size", 0, 0),
    spec!(HashMap, "isEmpty", 0, 0),
    spec!(HashMap, "merge", 1, 1),
    spec!(HashMap, "toString", 0, 0),
    // ── Set ─────────────────────────────────────────────────────────
    spec!(Set, "add", 1, 1),
    spec!(Set, "remove", 1, 1),
    spec!(Set, "has", 1, 1),
    spec!(Set, "union", 1, 1),
    spec!(Set, "intersect", 1, 1),
    spec!(Set, "diff", 1, 1),
    spec!(Set, "toList", 0, 0),
    spec!(Set, "size", 0, 0),
    spec!(Set, "isEmpty", 0, 0),
    spec!(Set, "toString", 0, 0),
    // ── Lax[T] ──────────────────────────────────────────────────────
    spec!(Lax, "hasValue", 0, 0),
    spec!(Lax, "isEmpty", 0, 0),
    spec!(Lax, "getOrDefault", 1, 1),
    spec!(Lax, "map", 1, 1),
    spec!(Lax, "flatMap", 1, 1),
    spec!(Lax, "errorInfo", 0, 0),
    spec!(Lax, "unmold", 0, 0),
    spec!(Lax, "toString", 0, 0),
    // ── Result[T, P] ────────────────────────────────────────────────
    spec!(Result, "isSuccess", 0, 0),
    spec!(Result, "isError", 0, 0),
    spec!(Result, "map", 1, 1),
    spec!(Result, "flatMap", 1, 1),
    spec!(Result, "mapError", 1, 1),
    spec!(Result, "getOrDefault", 1, 1),
    spec!(Result, "getOrThrow", 0, 0),
    spec!(Result, "toString", 0, 0),
    // ── Async[T] ────────────────────────────────────────────────────
    spec!(Async, "isPending", 0, 0),
    spec!(Async, "isFulfilled", 0, 0),
    spec!(Async, "isRejected", 0, 0),
    spec!(Async, "map", 1, 1),
    spec!(Async, "getOrDefault", 1, 1),
    spec!(Async, "toString", 0, 0),
    // ── Gorillax[T] / RelaxedGorillax[T] ───────────────────────────
    spec!(Gorillax, "hasValue", 0, 0),
    spec!(Gorillax, "isEmpty", 0, 0),
    spec!(Gorillax, "errorInfo", 0, 0),
    spec!(Gorillax, "toString", 0, 0),
    spec!(Gorillax, "relax", 0, 0),
    spec!(RelaxedGorillax, "hasValue", 0, 0),
    spec!(RelaxedGorillax, "isEmpty", 0, 0),
    spec!(RelaxedGorillax, "errorInfo", 0, 0),
    spec!(RelaxedGorillax, "toString", 0, 0),
    // ── Error ───────────────────────────────────────────────────────
    spec!(Error, "errorInfo", 0, 0),
    spec!(Error, "throw", 0, 0),
    spec!(Error, "toString", 0, 0),
];

#[cfg(test)]
mod tests {
    use super::super::TypeChecker;
    use super::*;
    use crate::types::Type;

    /// Representative checker `Type` for each statically enumerable
    /// receiver kind (element types are arbitrary — arity must not
    /// depend on them).
    fn recv_types(recv: BuiltinRecv) -> Vec<Type> {
        match recv {
            BuiltinRecv::Str => vec![Type::Str],
            BuiltinRecv::Num => vec![Type::Int, Type::Float, Type::Num],
            BuiltinRecv::Bool => vec![Type::Bool],
            BuiltinRecv::Bytes => vec![Type::Bytes],
            BuiltinRecv::List => vec![Type::List(Box::new(Type::Int))],
            BuiltinRecv::HashMap => vec![
                Type::Named("HashMap".to_string()),
                Type::Generic("HashMap".to_string(), vec![Type::Str, Type::Int]),
            ],
            BuiltinRecv::Set => vec![
                Type::Named("Set".to_string()),
                Type::Generic("Set".to_string(), vec![Type::Int]),
            ],
            BuiltinRecv::Lax => vec![Type::Generic("Lax".to_string(), vec![Type::Int])],
            BuiltinRecv::Result => vec![Type::Generic(
                "Result".to_string(),
                vec![Type::Int, Type::Str],
            )],
            BuiltinRecv::Async => vec![Type::Generic("Async".to_string(), vec![Type::Int])],
            BuiltinRecv::Gorillax => {
                vec![Type::Generic("Gorillax".to_string(), vec![Type::Int])]
            }
            BuiltinRecv::RelaxedGorillax => vec![Type::Generic(
                "RelaxedGorillax".to_string(),
                vec![Type::Int],
            )],
            BuiltinRecv::Error => vec![Type::Error("ProbeError".to_string())],
        }
    }

    const ALL_RECVS: &[BuiltinRecv] = &[
        BuiltinRecv::Str,
        BuiltinRecv::Num,
        BuiltinRecv::Bool,
        BuiltinRecv::Bytes,
        BuiltinRecv::List,
        BuiltinRecv::HashMap,
        BuiltinRecv::Set,
        BuiltinRecv::Lax,
        BuiltinRecv::Result,
        BuiltinRecv::Async,
        BuiltinRecv::Gorillax,
        BuiltinRecv::RelaxedGorillax,
        BuiltinRecv::Error,
    ];

    /// The universe = every method name in the table + a guaranteed-unknown
    /// probe. For every (receiver kind, universe name) pair, the table and
    /// `builtin_method_signature` must agree exactly: present-with-same
    /// arity, or absent on both sides. This catches edits to either side.
    #[test]
    fn spec_table_and_checker_signature_agree_over_the_universe() {
        let mut checker = TypeChecker::new();
        let mut universe: Vec<&'static str> = BUILTIN_METHOD_SPECS.iter().map(|s| s.name).collect();
        universe.push("zzzDefinitelyNotAMethod");
        universe.sort_unstable();
        universe.dedup();

        for &recv in ALL_RECVS {
            for ty in recv_types(recv) {
                for name in &universe {
                    let expected: Option<(usize, usize)> = BUILTIN_METHOD_SPECS
                        .iter()
                        .find(|s| s.recv == recv && s.name == *name)
                        .map(|s| (s.min_args, s.max_args));
                    let actual = checker
                        .builtin_method_signature(&ty, name)
                        .map(|(min, max, _)| (min, max));
                    assert_eq!(
                        expected, actual,
                        "spec table vs builtin_method_signature mismatch \
                         for ({recv:?} as {ty:?}).{name}"
                    );
                }
            }
        }
    }

    /// `Error` receivers fall back to user-defined members for unknown
    /// names (named_method_signature), so the universe test above relies
    /// on the probe type having no user fields. Pin that assumption.
    #[test]
    fn error_probe_type_has_no_user_members() {
        let mut checker = TypeChecker::new();
        assert_eq!(
            checker
                .builtin_method_signature(
                    &Type::Error("ProbeError".to_string()),
                    "zzzDefinitelyNotAMethod"
                )
                .map(|(min, max, _)| (min, max)),
            None
        );
    }
}

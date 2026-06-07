//! Arity/existence + return-kind mirror of the checker's two builtin
//! method paths (`TypeChecker::builtin_method_signature` for arity,
//! `TypeChecker::infer_method_return_type` for the argless return
//! type) — instalments 1 and 2 of the builtin method spec table.
//!
//! Scope: exactly the statically enumerable receivers of those two
//! functions. Explicitly NOT covered (they live on other paths):
//! value-dependent receivers (`BuchiPack` function fields, user-defined
//! `Named` members via `named_method_signature`), the `Stream` receiver
//! (known only to the return-type path `infer_method_return_type`), the
//! universal `toString` fallback, the `errorInfo` allow-list special
//! case in `check_method_args`, and the args-aware return refinements
//! in `infer_method_return_type_with_args` (Lax/Result/Async lambda
//! plumbing; `List.reduce`/`fold` whose return is the init argument's
//! type). A claim of "method existence SSOT" must wait until those
//! paths are unified.
//!
//! The table is pinned to the checker implementation by the exhaustive
//! cross tests below: for every (receiver, method) pair in the universe
//! of known method names, arity must agree with
//! `builtin_method_signature` (Some-with-same-arity vs None) and the
//! rendered `ReturnKind` must equal `infer_method_return_type` exactly
//! (absent entries ⇒ `Type::Unknown`). Editing one side without the
//! other fails the tests, which is the point.
//!
//! Argument types are deliberately out of scope for this instalment:
//! several signatures are parameterised by the receiver's element types
//! and cannot live in a static table without loss.

use crate::types::Type;

/// Statically enumerable builtin receiver kinds.
// Production code does not consume the table yet — in this first
// instalment its sole binding force is the exhaustive cross test below
// (and the cross-backend audit script under .dev). Later instalments
// will switch the checker dispatch to read from it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

/// How a builtin method's return type on the checker's argless path
/// (`infer_method_return_type`) derives from the receiver type.
///
/// Several variants are receiver-shape-aware because the checker
/// degrades differently for bare `Named` receivers vs parameterised
/// `Generic` ones (e.g. bare `HashMap.values` → `Str[]`→no, `Any[]`;
/// `HashMap[K, V].values` → `V[]`). Each variant is used only under the
/// receiver kinds it names; `render_return_kind` matches on the
/// receiver's concrete shape and reproduces those degradations exactly.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ReturnKind {
    /// `Int`
    Int,
    /// `Bool`
    Bool,
    /// `Str`
    Str,
    /// `Lax[Int]`
    LaxInt,
    /// `Lax[Str]`
    LaxStr,
    /// `Lax[ErrorInfo]`
    LaxErrorInfo,
    /// `Str[]`
    ListStr,
    /// `RegexMatch` (named pack)
    RegexMatch,
    /// The receiver type itself, verbatim (bare `Named` forms included)
    Receiver,
    /// `List[T]` → `Lax[T]`
    LaxOfListElem,
    /// bare `HashMap` → `Lax[Any]`; `HashMap[K, V]` → `Lax[V]`
    LaxOfMapValue,
    /// The receiver's first type argument (`Lax[T]`/`Result[T, _]`/
    /// `Async[T]` → `T`)
    FirstTypeArg,
    /// bare `HashMap` → `Str[]`; `HashMap[K, V]` → `K[]`
    ListOfMapKeys,
    /// bare `HashMap` → `Any[]`; `HashMap[K, V]` → `V[]`
    ListOfMapValues,
    /// bare `HashMap` → `Any[]`; `HashMap[K, V]` → `Unknown[]`
    ListOfMapEntries,
    /// bare `Set` → `Unknown[]`; `Set[T]` → `T[]`
    ListOfSetElem,
    /// `Gorillax[T]` → `RelaxedGorillax[T]`
    Relaxed,
    /// Structurally `Type::Unknown` on the argless path. Closed set
    /// (see test): `Error.throw` diverges; `List.reduce`/`fold` return
    /// their init argument's type, resolvable only on the args-aware
    /// path (`infer_method_return_type_with_args`).
    Unknown,
}

/// One builtin method: existence + arity + argless return kind. The
/// single source of truth for "which methods exist on which builtin
/// type, taking how many args, returning what".
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct BuiltinMethodSpec {
    pub(crate) recv: BuiltinRecv,
    pub(crate) name: &'static str,
    pub(crate) min_args: usize,
    pub(crate) max_args: usize,
    pub(crate) ret: ReturnKind,
}

/// Expand a `ReturnKind` against a concrete receiver type, reproducing
/// the argless return rules of `infer_method_return_type` exactly —
/// including the bare-`Named` degradations. Test-bound in this
/// instalment; the dispatch rewrite will make the checker read from it.
#[allow(dead_code)]
pub(crate) fn render_return_kind(kind: ReturnKind, recv: &Type) -> Type {
    fn lax(t: Type) -> Type {
        Type::Generic("Lax".to_string(), vec![t])
    }
    match kind {
        ReturnKind::Int => Type::Int,
        ReturnKind::Bool => Type::Bool,
        ReturnKind::Str => Type::Str,
        ReturnKind::LaxInt => lax(Type::Int),
        ReturnKind::LaxStr => lax(Type::Str),
        ReturnKind::LaxErrorInfo => lax(Type::Named("ErrorInfo".to_string())),
        ReturnKind::ListStr => Type::List(Box::new(Type::Str)),
        ReturnKind::RegexMatch => Type::Named("RegexMatch".to_string()),
        ReturnKind::Receiver => recv.clone(),
        ReturnKind::LaxOfListElem => match recv {
            Type::List(inner) => lax((**inner).clone()),
            _ => Type::Unknown,
        },
        ReturnKind::LaxOfMapValue => match recv {
            Type::Named(_) => lax(Type::Any),
            Type::Generic(_, args) => lax(args.get(1).cloned().unwrap_or(Type::Unknown)),
            _ => Type::Unknown,
        },
        ReturnKind::FirstTypeArg => match recv {
            Type::Generic(_, args) => args.first().cloned().unwrap_or(Type::Unknown),
            _ => Type::Unknown,
        },
        ReturnKind::ListOfMapKeys => match recv {
            Type::Named(_) => Type::List(Box::new(Type::Str)),
            Type::Generic(_, args) => {
                Type::List(Box::new(args.first().cloned().unwrap_or(Type::Unknown)))
            }
            _ => Type::Unknown,
        },
        ReturnKind::ListOfMapValues => match recv {
            Type::Named(_) => Type::List(Box::new(Type::Any)),
            Type::Generic(_, args) => {
                Type::List(Box::new(args.get(1).cloned().unwrap_or(Type::Unknown)))
            }
            _ => Type::Unknown,
        },
        ReturnKind::ListOfMapEntries => match recv {
            Type::Named(_) => Type::List(Box::new(Type::Any)),
            Type::Generic(_, _) => Type::List(Box::new(Type::Unknown)),
            _ => Type::Unknown,
        },
        ReturnKind::ListOfSetElem => match recv {
            Type::Named(_) => Type::List(Box::new(Type::Unknown)),
            Type::Generic(_, args) => {
                Type::List(Box::new(args.first().cloned().unwrap_or(Type::Unknown)))
            }
            _ => Type::Unknown,
        },
        ReturnKind::Relaxed => match recv {
            Type::Generic(_, args) => Type::Generic("RelaxedGorillax".to_string(), args.clone()),
            _ => Type::Unknown,
        },
        ReturnKind::Unknown => Type::Unknown,
    }
}

macro_rules! spec {
    ($recv:ident, $name:literal, $min:literal, $max:literal, $ret:ident) => {
        BuiltinMethodSpec {
            recv: BuiltinRecv::$recv,
            name: $name,
            min_args: $min,
            max_args: $max,
            ret: ReturnKind::$ret,
        }
    };
}

/// Faithful transcription of `builtin_method_signature` (arity columns)
/// and `infer_method_return_type` (return-kind column), checked by test.
#[allow(dead_code)]
pub(crate) static BUILTIN_METHOD_SPECS: &[BuiltinMethodSpec] = &[
    // ── Str ─────────────────────────────────────────────────────────
    spec!(Str, "length", 0, 0, Int),
    spec!(Str, "toString", 0, 0, Str),
    spec!(Str, "contains", 1, 1, Bool),
    spec!(Str, "startsWith", 1, 1, Bool),
    spec!(Str, "endsWith", 1, 1, Bool),
    spec!(Str, "indexOf", 1, 1, Int),
    spec!(Str, "lastIndexOf", 1, 1, Int),
    spec!(Str, "indexOfLax", 1, 1, LaxInt),
    spec!(Str, "lastIndexOfLax", 1, 1, LaxInt),
    spec!(Str, "get", 1, 1, LaxStr),
    spec!(Str, "replace", 2, 2, Str),
    spec!(Str, "replaceAll", 2, 2, Str),
    spec!(Str, "split", 1, 1, ListStr),
    spec!(Str, "match", 1, 1, RegexMatch),
    spec!(Str, "search", 1, 1, Int),
    spec!(Str, "searchLax", 1, 1, LaxInt),
    // ── Int / Float / Num ───────────────────────────────────────────
    spec!(Num, "toString", 0, 0, Str),
    spec!(Num, "isNaN", 0, 0, Bool),
    spec!(Num, "isInfinite", 0, 0, Bool),
    spec!(Num, "isFinite", 0, 0, Bool),
    spec!(Num, "isPositive", 0, 0, Bool),
    spec!(Num, "isNegative", 0, 0, Bool),
    spec!(Num, "isZero", 0, 0, Bool),
    // ── Bool ────────────────────────────────────────────────────────
    spec!(Bool, "toString", 0, 0, Str),
    // ── Bytes ───────────────────────────────────────────────────────
    spec!(Bytes, "length", 0, 0, Int),
    spec!(Bytes, "get", 1, 1, LaxInt),
    spec!(Bytes, "toString", 0, 0, Str),
    // ── List ────────────────────────────────────────────────────────
    spec!(List, "length", 0, 0, Int),
    spec!(List, "isEmpty", 0, 0, Bool),
    spec!(List, "first", 0, 0, LaxOfListElem),
    spec!(List, "last", 0, 0, LaxOfListElem),
    spec!(List, "max", 0, 0, LaxOfListElem),
    spec!(List, "min", 0, 0, LaxOfListElem),
    spec!(List, "get", 1, 1, LaxOfListElem),
    spec!(List, "contains", 1, 1, Bool),
    spec!(List, "indexOf", 1, 1, Int),
    spec!(List, "lastIndexOf", 1, 1, Int),
    spec!(List, "indexOfLax", 1, 1, LaxInt),
    spec!(List, "lastIndexOfLax", 1, 1, LaxInt),
    spec!(List, "any", 1, 1, Bool),
    spec!(List, "all", 1, 1, Bool),
    spec!(List, "none", 1, 1, Bool),
    spec!(List, "reduce", 2, 2, Unknown),
    spec!(List, "fold", 2, 2, Unknown),
    spec!(List, "toString", 0, 0, Str),
    // ── HashMap ─────────────────────────────────────────────────────
    spec!(HashMap, "get", 1, 1, LaxOfMapValue),
    spec!(HashMap, "set", 2, 2, Receiver),
    spec!(HashMap, "remove", 1, 1, Receiver),
    spec!(HashMap, "has", 1, 1, Bool),
    spec!(HashMap, "keys", 0, 0, ListOfMapKeys),
    spec!(HashMap, "values", 0, 0, ListOfMapValues),
    spec!(HashMap, "entries", 0, 0, ListOfMapEntries),
    spec!(HashMap, "size", 0, 0, Int),
    spec!(HashMap, "isEmpty", 0, 0, Bool),
    spec!(HashMap, "merge", 1, 1, Receiver),
    spec!(HashMap, "toString", 0, 0, Str),
    // ── Set ─────────────────────────────────────────────────────────
    spec!(Set, "add", 1, 1, Receiver),
    spec!(Set, "remove", 1, 1, Receiver),
    spec!(Set, "has", 1, 1, Bool),
    spec!(Set, "union", 1, 1, Receiver),
    spec!(Set, "intersect", 1, 1, Receiver),
    spec!(Set, "diff", 1, 1, Receiver),
    spec!(Set, "toList", 0, 0, ListOfSetElem),
    spec!(Set, "size", 0, 0, Int),
    spec!(Set, "isEmpty", 0, 0, Bool),
    spec!(Set, "toString", 0, 0, Str),
    // ── Lax[T] ──────────────────────────────────────────────────────
    spec!(Lax, "hasValue", 0, 0, Bool),
    spec!(Lax, "isEmpty", 0, 0, Bool),
    spec!(Lax, "getOrDefault", 1, 1, FirstTypeArg),
    spec!(Lax, "map", 1, 1, Receiver),
    spec!(Lax, "flatMap", 1, 1, Receiver),
    spec!(Lax, "errorInfo", 0, 0, LaxErrorInfo),
    spec!(Lax, "unmold", 0, 0, FirstTypeArg),
    spec!(Lax, "toString", 0, 0, Str),
    // ── Result[T, P] ────────────────────────────────────────────────
    spec!(Result, "isSuccess", 0, 0, Bool),
    spec!(Result, "isError", 0, 0, Bool),
    spec!(Result, "map", 1, 1, Receiver),
    spec!(Result, "flatMap", 1, 1, Receiver),
    spec!(Result, "mapError", 1, 1, Receiver),
    spec!(Result, "getOrDefault", 1, 1, FirstTypeArg),
    spec!(Result, "getOrThrow", 0, 0, FirstTypeArg),
    spec!(Result, "toString", 0, 0, Str),
    // ── Async[T] ────────────────────────────────────────────────────
    spec!(Async, "isPending", 0, 0, Bool),
    spec!(Async, "isFulfilled", 0, 0, Bool),
    spec!(Async, "isRejected", 0, 0, Bool),
    spec!(Async, "map", 1, 1, Receiver),
    spec!(Async, "getOrDefault", 1, 1, FirstTypeArg),
    spec!(Async, "toString", 0, 0, Str),
    // ── Gorillax[T] / RelaxedGorillax[T] ───────────────────────────
    spec!(Gorillax, "hasValue", 0, 0, Bool),
    spec!(Gorillax, "isEmpty", 0, 0, Bool),
    spec!(Gorillax, "errorInfo", 0, 0, LaxErrorInfo),
    spec!(Gorillax, "toString", 0, 0, Str),
    spec!(Gorillax, "relax", 0, 0, Relaxed),
    spec!(RelaxedGorillax, "hasValue", 0, 0, Bool),
    spec!(RelaxedGorillax, "isEmpty", 0, 0, Bool),
    spec!(RelaxedGorillax, "errorInfo", 0, 0, LaxErrorInfo),
    spec!(RelaxedGorillax, "toString", 0, 0, Str),
    // ── Error ───────────────────────────────────────────────────────
    spec!(Error, "errorInfo", 0, 0, LaxErrorInfo),
    spec!(Error, "throw", 0, 0, Unknown),
    spec!(Error, "toString", 0, 0, Str),
];

#[cfg(test)]
mod tests {
    use super::super::TypeChecker;
    use super::*;

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

    /// `.find()`-based comparison above would silently accept a duplicate
    /// (recv, name) entry with a conflicting arity — forbid duplicates.
    #[test]
    fn spec_table_has_no_duplicate_entries() {
        let mut seen = std::collections::HashSet::new();
        for s in BUILTIN_METHOD_SPECS {
            assert!(
                seen.insert((s.recv, s.name)),
                "duplicate spec entry: ({:?}, {})",
                s.recv,
                s.name
            );
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

    /// Return-kind universe cross test (the return-path mirror of the
    /// arity test above): for every (receiver type, universe name) pair
    /// the rendered table entry must equal `infer_method_return_type`
    /// exactly — including absences (no table entry ⇒ the argless path
    /// yields `Type::Unknown`). This subsumes the cross-path drift gate:
    /// a method known to one checker path but not the other shows up as
    /// a mismatch here (the class of skew that produced the runtime-only
    /// `Async.unmold` divergence).
    #[test]
    fn spec_table_and_return_type_path_agree_over_the_universe() {
        let checker = TypeChecker::new();
        let mut universe: Vec<&'static str> = BUILTIN_METHOD_SPECS.iter().map(|s| s.name).collect();
        universe.push("zzzDefinitelyNotAMethod");
        universe.sort_unstable();
        universe.dedup();

        for &recv in ALL_RECVS {
            for ty in recv_types(recv) {
                for name in &universe {
                    let expected = BUILTIN_METHOD_SPECS
                        .iter()
                        .find(|s| s.recv == recv && s.name == *name)
                        .map(|s| render_return_kind(s.ret, &ty))
                        .unwrap_or(Type::Unknown);
                    let actual = checker.infer_method_return_type(&ty, name);
                    assert_eq!(
                        expected, actual,
                        "return-kind table vs infer_method_return_type mismatch \
                         for ({recv:?} as {ty:?}).{name}"
                    );
                }
            }
        }
    }

    /// The argless return path is `Unknown` for exactly three table
    /// entries — keep the set closed so a new `Unknown` entry forces a
    /// design look (either the method belongs on the args-aware path,
    /// like reduce/fold whose return is the init argument's type, or it
    /// diverges, like throw).
    #[test]
    fn unknown_return_kind_entries_are_a_closed_set() {
        let unknowns: std::collections::HashSet<(BuiltinRecv, &str)> = BUILTIN_METHOD_SPECS
            .iter()
            .filter(|s| s.ret == ReturnKind::Unknown)
            .map(|s| (s.recv, s.name))
            .collect();
        let expected: std::collections::HashSet<(BuiltinRecv, &str)> = [
            (BuiltinRecv::List, "reduce"),
            (BuiltinRecv::List, "fold"),
            (BuiltinRecv::Error, "throw"),
        ]
        .into_iter()
        .collect();
        assert_eq!(unknowns, expected);
    }

    /// Pin the known asymmetries between the checker's two method paths
    /// so any future change to either side surfaces here and forces the
    /// spec table (and the cross-backend audit) to be revisited:
    /// - `Stream` is a return-path-only receiver (arity path: None).
    /// - `Async.unmold` is implemented by the interp and native runtimes
    ///   but known to NEITHER checker path (reachable only via
    ///   `--no-check`; recorded as a parity-hole finding).
    #[test]
    fn known_path_asymmetries_hold() {
        let mut checker = TypeChecker::new();
        let stream = Type::Generic("Stream".to_string(), vec![Type::Int]);
        assert_eq!(
            checker
                .builtin_method_signature(&stream, "length")
                .map(|(min, max, _)| (min, max)),
            None,
            "arity path unexpectedly learned Stream.length — add Stream \
             to the spec table"
        );
        assert_eq!(
            checker.infer_method_return_type(&stream, "length"),
            Type::Int
        );

        let async_ty = Type::Generic("Async".to_string(), vec![Type::Int]);
        assert_eq!(
            checker
                .builtin_method_signature(&async_ty, "unmold")
                .map(|(min, max, _)| (min, max)),
            None,
            "arity path unexpectedly learned Async.unmold — update the \
             spec table and the cross-backend audit notes"
        );
        assert_eq!(
            checker.infer_method_return_type(&async_ty, "unmold"),
            Type::Unknown,
            "return path unexpectedly learned Async.unmold — update the \
             spec table and the cross-backend audit notes"
        );
    }
}

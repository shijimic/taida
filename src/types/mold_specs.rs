//! Builtin mold specification registry.
//!
//! This module centralizes builtin mold metadata. Codegen uses the
//! return-kind portion for runtime type tags, and the type checker can
//! use the same entry for arity and option validation. The shape is
//! intentionally data-first: adding a builtin mold should mean adding
//! or updating one registry row, not copying name lists across backends.
//!
//! Older lowering code treated every `MoldInst` as Pack-tagged, which
//! made runtime display dispatch misclassify Str-returning molds such as
//! `Upper`, `Lower`, `Trim`, and `Join`. Keeping the return tag here lets
//! callers choose the correct runtime display path directly.
//!
//! Tag values match the `TAIDA_TAG_*` constants in `native_runtime.c`
//! and the `WASM_TAG_*` constants in `runtime_core_wasm.c`:
//!
//! - `0` — Int
//! - `1` — Float
//! - `2` — Bool
//! - `3` — Str
//! - `4` — Pack
//! - `5` — List
//! - `6` — Closure
//!
//! A mold whose return tag depends on its arguments (e.g. `Slice`,
//! `Concat`, `Abs`, `Map`, `Filter`, `If`) returns `MoldReturnKind::Dynamic`
//! and callers fall back to argument-based inference or the UNKNOWN tag
//! (`-1`). User-defined molds (not in this table) always return Pack.
//!
/// Known return-type kind for a builtin mold.
///
/// `Dynamic` marks molds whose return type is determined by argument
/// types (e.g. `Concat`, `Slice`, `Abs`, `Map`). Such cases are resolved
/// at each call site; this table only fixes the cases that are constant
/// regardless of arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoldReturnKind {
    Int,
    Float,
    Bool,
    Str,
    Pack,
    List,
    /// Return type depends on arguments; caller must infer.
    Dynamic,
}

/// Coarse checker-side kind for a positional `[]` argument or option value.
///
/// These kinds deliberately stay small. They describe public mold contracts
/// that are stable across backends; richer return-type inference still lives
/// in the checker because it can depend on the actual argument expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoldArgKind {
    Any,
    Bool,
    Function,
    Int,
    Str,
    NullaryFunction,
    UnaryFunction,
    UnaryPredicate,
    BinaryFunction,
    List,
    ListOrStream,
    Numeric,
}

/// Whether a builtin mold can cross a CPU worker boundary.
///
/// The default is intentionally `Unsafe`; only molds with explicit evidence
/// may be promoted to `Pure` or `Transparent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerSafety {
    Pure,
    Transparent,
    Unsafe,
}

/// Why a builtin mold is disallowed inside a CPU worker body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerMoldBoundary {
    Structural,
    Effectful,
    HostBoundary,
    NestedAsync,
}

/// Named `()` option accepted by a builtin mold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoldOptionSpec {
    pub name: &'static str,
    pub kind: MoldArgKind,
}

/// Builtin mold metadata shared by return-tag propagation and checker
/// front-door validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoldSpec {
    pub name: &'static str,
    pub arity_min: usize,
    pub arity_max: Option<usize>,
    pub arg_kinds: &'static [MoldArgKind],
    pub return_kind: MoldReturnKind,
    pub options: &'static [MoldOptionSpec],
    pub worker_safety: WorkerSafety,
    pub worker_boundary: WorkerMoldBoundary,
    /// Some legacy molds still need permissive arity until their runtime
    /// contracts are audited. `true` means the checker should enforce the
    /// registry arity now.
    pub checker_enforced: bool,
}

impl MoldSpec {
    pub const fn exact(
        name: &'static str,
        arity: usize,
        arg_kinds: &'static [MoldArgKind],
        return_kind: MoldReturnKind,
    ) -> Self {
        Self {
            name,
            arity_min: arity,
            arity_max: Some(arity),
            arg_kinds,
            return_kind,
            options: &[],
            worker_safety: WorkerSafety::Unsafe,
            worker_boundary: WorkerMoldBoundary::Structural,
            checker_enforced: false,
        }
    }

    pub const fn range(
        name: &'static str,
        min: usize,
        max: Option<usize>,
        arg_kinds: &'static [MoldArgKind],
        return_kind: MoldReturnKind,
    ) -> Self {
        Self {
            name,
            arity_min: min,
            arity_max: max,
            arg_kinds,
            return_kind,
            options: &[],
            worker_safety: WorkerSafety::Unsafe,
            worker_boundary: WorkerMoldBoundary::Structural,
            checker_enforced: false,
        }
    }

    pub const fn enforce_checker(mut self) -> Self {
        self.checker_enforced = true;
        self
    }

    pub const fn with_options(mut self, options: &'static [MoldOptionSpec]) -> Self {
        self.options = options;
        self
    }

    pub const fn with_worker_safety(mut self, worker_safety: WorkerSafety) -> Self {
        self.worker_safety = worker_safety;
        self
    }

    pub const fn with_worker_boundary(mut self, worker_boundary: WorkerMoldBoundary) -> Self {
        self.worker_boundary = worker_boundary;
        self
    }

    pub fn accepts_arity(&self, arity: usize) -> bool {
        arity >= self.arity_min && self.arity_max.is_none_or(|max| arity <= max)
    }

    pub fn arity_description(&self) -> String {
        match (self.arity_min, self.arity_max) {
            (min, Some(max)) if min == max => min.to_string(),
            (min, Some(max)) => format!("{}-{}", min, max),
            (min, None) => format!("at least {}", min),
        }
    }
}

impl MoldReturnKind {
    /// Return the runtime type tag for this kind, or `None` if the kind
    /// is Dynamic (argument-dependent).
    pub fn tag(self) -> Option<i64> {
        match self {
            MoldReturnKind::Int => Some(0),
            MoldReturnKind::Float => Some(1),
            MoldReturnKind::Bool => Some(2),
            MoldReturnKind::Str => Some(3),
            MoldReturnKind::Pack => Some(4),
            MoldReturnKind::List => Some(5),
            MoldReturnKind::Dynamic => None,
        }
    }
}

const ANY1: &[MoldArgKind] = &[MoldArgKind::Any];
const ANY2: &[MoldArgKind] = &[MoldArgKind::Any, MoldArgKind::Any];
const ANY3: &[MoldArgKind] = &[MoldArgKind::Any, MoldArgKind::Any, MoldArgKind::Any];
const ANY4: &[MoldArgKind] = &[
    MoldArgKind::Any,
    MoldArgKind::Any,
    MoldArgKind::Any,
    MoldArgKind::Any,
];
const NULLARY_FUNCTION1: &[MoldArgKind] = &[MoldArgKind::NullaryFunction];
const LIST1: &[MoldArgKind] = &[MoldArgKind::List];
const LIST_UNARY_FUNCTION: &[MoldArgKind] =
    &[MoldArgKind::ListOrStream, MoldArgKind::UnaryFunction];
const LIST_UNARY_PREDICATE: &[MoldArgKind] =
    &[MoldArgKind::ListOrStream, MoldArgKind::UnaryPredicate];
const LIST_ANY: &[MoldArgKind] = &[MoldArgKind::List, MoldArgKind::Any];
const LIST_OR_STREAM_ANY: &[MoldArgKind] = &[MoldArgKind::ListOrStream, MoldArgKind::Any];
const LIST_ANY_BINARY_FUNCTION: &[MoldArgKind] = &[
    MoldArgKind::List,
    MoldArgKind::Any,
    MoldArgKind::BinaryFunction,
];
const LIST_OR_STREAM_PREDICATE: &[MoldArgKind] =
    &[MoldArgKind::ListOrStream, MoldArgKind::UnaryPredicate];
const ASYNC_NUM: &[MoldArgKind] = &[MoldArgKind::Any, MoldArgKind::Numeric];

const BYTES_CURSOR_OPTIONS: &[MoldOptionSpec] = &[MoldOptionSpec {
    name: "offset",
    kind: MoldArgKind::Int,
}];
const TRIM_OPTIONS: &[MoldOptionSpec] = &[
    MoldOptionSpec {
        name: "start",
        kind: MoldArgKind::Bool,
    },
    MoldOptionSpec {
        name: "end",
        kind: MoldArgKind::Bool,
    },
];
const REPLACE_OPTIONS: &[MoldOptionSpec] = &[MoldOptionSpec {
    name: "all",
    kind: MoldArgKind::Bool,
}];
const SLICE_OPTIONS: &[MoldOptionSpec] = &[
    MoldOptionSpec {
        name: "start",
        kind: MoldArgKind::Int,
    },
    MoldOptionSpec {
        name: "end",
        kind: MoldArgKind::Int,
    },
];
const PAD_OPTIONS: &[MoldOptionSpec] = &[
    MoldOptionSpec {
        name: "side",
        kind: MoldArgKind::Str,
    },
    MoldOptionSpec {
        name: "char",
        kind: MoldArgKind::Str,
    },
];
const RESULT_OPTIONS: &[MoldOptionSpec] = &[MoldOptionSpec {
    name: "throw",
    kind: MoldArgKind::Any,
}];
const BYTES_OPTIONS: &[MoldOptionSpec] = &[MoldOptionSpec {
    name: "fill",
    kind: MoldArgKind::Int,
}];
const DIV_OPTIONS: &[MoldOptionSpec] = &[MoldOptionSpec {
    name: "default",
    kind: MoldArgKind::Any,
}];
const TODO_OPTIONS: &[MoldOptionSpec] = &[
    MoldOptionSpec {
        name: "id",
        kind: MoldArgKind::Any,
    },
    MoldOptionSpec {
        name: "task",
        kind: MoldArgKind::Any,
    },
    MoldOptionSpec {
        name: "sol",
        kind: MoldArgKind::Any,
    },
    MoldOptionSpec {
        name: "unm",
        kind: MoldArgKind::Any,
    },
];
const HTTP_REQUEST_OPTIONS: &[MoldOptionSpec] = &[
    MoldOptionSpec {
        name: "headers",
        kind: MoldArgKind::Any,
    },
    MoldOptionSpec {
        name: "body",
        kind: MoldArgKind::Any,
    },
];
const SORT_OPTIONS: &[MoldOptionSpec] = &[
    MoldOptionSpec {
        name: "reverse",
        kind: MoldArgKind::Bool,
    },
    MoldOptionSpec {
        name: "desc",
        kind: MoldArgKind::Bool,
    },
    MoldOptionSpec {
        name: "by",
        kind: MoldArgKind::UnaryFunction,
    },
];
const UNIQUE_OPTIONS: &[MoldOptionSpec] = &[MoldOptionSpec {
    name: "by",
    kind: MoldArgKind::UnaryFunction,
}];

pub static MOLD_SPECS: &[MoldSpec] = &[
    // Primitive conversion / wrappers.
    MoldSpec::range("Int", 1, Some(2), ANY2, MoldReturnKind::Pack),
    MoldSpec::exact("Float", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("Bool", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("Str", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("Bytes", 1, ANY1, MoldReturnKind::Pack).with_options(BYTES_OPTIONS),
    MoldSpec::exact("UInt8", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("Char", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("CodePoint", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("Utf8Encode", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("Utf8Decode", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("U16BE", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("U16LE", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("U32BE", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("U32LE", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("U16BEDecode", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("U16LEDecode", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("U32BEDecode", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("U32LEDecode", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("BytesCursor", 1, ANY1, MoldReturnKind::Pack)
        .with_options(BYTES_CURSOR_OPTIONS),
    MoldSpec::exact("BytesCursorRemaining", 1, ANY1, MoldReturnKind::Int),
    MoldSpec::exact("BytesCursorTake", 2, ANY2, MoldReturnKind::Pack),
    MoldSpec::exact("BytesCursorU8", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::range("Lax", 0, Some(1), ANY1, MoldReturnKind::Pack)
        .with_worker_safety(WorkerSafety::Transparent),
    MoldSpec::range("Result", 1, Some(2), ANY2, MoldReturnKind::Pack)
        .with_options(RESULT_OPTIONS)
        .with_worker_safety(WorkerSafety::Transparent),
    MoldSpec::exact("Async", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync),
    MoldSpec::exact("AsyncReject", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync),
    MoldSpec::exact("AsyncTask", 1, NULLARY_FUNCTION1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync)
        .enforce_checker(),
    MoldSpec::exact("Gorillax", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_safety(WorkerSafety::Transparent),
    MoldSpec::exact("RelaxedGorillax", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_safety(WorkerSafety::Transparent),
    MoldSpec::exact("Stream", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync),
    MoldSpec::exact("StreamFrom", 1, LIST1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync),
    MoldSpec::range("Optional", 0, Some(1), ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("Molten", 0, &[], MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("Stub", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::range("TODO", 0, Some(4), ANY4, MoldReturnKind::Pack).with_options(TODO_OPTIONS),
    MoldSpec::exact("Cage", 2, ANY2, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("CageRilla", 2, ANY2, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("HostCall", 2, ANY2, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("HostStep", 2, ANY2, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("HostCapability", 2, ANY2, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("JSRilla", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("FileRilla", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("BuildRilla", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("JSON", 2, ANY2, MoldReturnKind::Pack),
    MoldSpec::exact("JSGet", 2, ANY2, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("JSCall", 3, ANY3, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("JSCallAsync", 3, ANY3, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("JSNew", 3, ANY3, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("JSSet", 2, ANY2, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("JSBind", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    MoldSpec::exact("JSSpread", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::HostBoundary),
    // Numeric / math.
    MoldSpec::exact("Sqrt", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Pow", 2, ANY2, MoldReturnKind::Float),
    MoldSpec::range("Log", 1, Some(2), ANY2, MoldReturnKind::Float),
    MoldSpec::exact("Exp", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Ln", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Log2", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Log10", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Sin", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Cos", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Tan", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Asin", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Acos", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Atan", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Atan2", 2, ANY2, MoldReturnKind::Float),
    MoldSpec::exact("Sinh", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Cosh", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Tanh", 1, ANY1, MoldReturnKind::Float),
    MoldSpec::exact("Floor", 1, ANY1, MoldReturnKind::Int),
    MoldSpec::exact("Ceil", 1, ANY1, MoldReturnKind::Int),
    MoldSpec::exact("Round", 1, ANY1, MoldReturnKind::Int),
    MoldSpec::exact("Truncate", 1, ANY1, MoldReturnKind::Int),
    MoldSpec::exact("Abs", 1, ANY1, MoldReturnKind::Dynamic),
    MoldSpec::exact("Clamp", 3, ANY3, MoldReturnKind::Dynamic),
    MoldSpec::exact("Div", 2, ANY2, MoldReturnKind::Pack).with_options(DIV_OPTIONS),
    MoldSpec::exact("Mod", 2, ANY2, MoldReturnKind::Pack),
    // String / bytes.
    MoldSpec::exact("TypeName", 1, ANY1, MoldReturnKind::Str),
    MoldSpec::exact("Upper", 1, ANY1, MoldReturnKind::Str),
    MoldSpec::exact("Lower", 1, ANY1, MoldReturnKind::Str),
    MoldSpec::exact("Trim", 1, ANY1, MoldReturnKind::Str).with_options(TRIM_OPTIONS),
    MoldSpec::exact("Replace", 3, ANY3, MoldReturnKind::Str).with_options(REPLACE_OPTIONS),
    MoldSpec::exact("ReplaceAll", 3, ANY3, MoldReturnKind::Str),
    MoldSpec::exact("Repeat", 2, ANY2, MoldReturnKind::Str),
    MoldSpec::exact("Pad", 2, ANY2, MoldReturnKind::Str).with_options(PAD_OPTIONS),
    MoldSpec::exact("PadLeft", 3, ANY3, MoldReturnKind::Str),
    MoldSpec::exact("PadRight", 3, ANY3, MoldReturnKind::Str),
    MoldSpec::exact("Join", 2, ANY2, MoldReturnKind::Str),
    MoldSpec::exact("ToFixed", 2, ANY2, MoldReturnKind::Str),
    MoldSpec::exact("ToRadix", 2, ANY2, MoldReturnKind::Pack),
    MoldSpec::exact("StrOf", 2, ANY2, MoldReturnKind::Str),
    MoldSpec::exact("ByteSlice", 3, ANY3, MoldReturnKind::Str),
    MoldSpec::exact("StringRepeatJoin", 3, ANY3, MoldReturnKind::Str),
    MoldSpec::exact("ByteLength", 1, ANY1, MoldReturnKind::Int),
    MoldSpec::exact("ByteAt", 2, ANY2, MoldReturnKind::Pack),
    MoldSpec::exact("CharAt", 2, ANY2, MoldReturnKind::Pack),
    MoldSpec::exact("ByteSet", 3, ANY3, MoldReturnKind::Pack),
    MoldSpec::exact("BytesToList", 1, ANY1, MoldReturnKind::List),
    MoldSpec::range("Slice", 1, Some(3), ANY3, MoldReturnKind::Dynamic).with_options(SLICE_OPTIONS),
    MoldSpec::range("Concat", 2, None, ANY2, MoldReturnKind::Dynamic),
    MoldSpec::exact("Chars", 1, ANY1, MoldReturnKind::List),
    MoldSpec::exact("Split", 2, ANY2, MoldReturnKind::List),
    // Bool / type predicates.
    MoldSpec::exact("TypeIs", 2, ANY2, MoldReturnKind::Bool),
    MoldSpec::exact("TypeExtends", 2, ANY2, MoldReturnKind::Bool),
    MoldSpec::exact("Exists", 1, ANY1, MoldReturnKind::Pack),
    MoldSpec::exact("Contains", 2, ANY2, MoldReturnKind::Bool),
    MoldSpec::exact("SpanEquals", 3, ANY3, MoldReturnKind::Bool),
    MoldSpec::exact("SpanStartsWith", 3, ANY3, MoldReturnKind::Bool),
    MoldSpec::exact("SpanContains", 3, ANY3, MoldReturnKind::Bool),
    MoldSpec::exact("SpanSlice", 4, ANY4, MoldReturnKind::Pack),
    // Bit / ordinal.
    MoldSpec::exact("BitAnd", 2, ANY2, MoldReturnKind::Int),
    MoldSpec::exact("BitOr", 2, ANY2, MoldReturnKind::Int),
    MoldSpec::exact("BitXor", 2, ANY2, MoldReturnKind::Int),
    MoldSpec::exact("BitNot", 1, ANY1, MoldReturnKind::Int),
    MoldSpec::exact("ShiftL", 2, ANY2, MoldReturnKind::Pack),
    MoldSpec::exact("ShiftR", 2, ANY2, MoldReturnKind::Pack),
    MoldSpec::exact("ShiftRU", 2, ANY2, MoldReturnKind::Pack),
    MoldSpec::exact("Ordinal", 1, ANY1, MoldReturnKind::Int),
    // Collections and HOF molds.
    MoldSpec::exact("Length", 1, ANY1, MoldReturnKind::Int),
    MoldSpec::exact("Count", 2, LIST_UNARY_PREDICATE, MoldReturnKind::Int),
    MoldSpec::exact("Find", 2, LIST_UNARY_PREDICATE, MoldReturnKind::Pack),
    MoldSpec::exact("FindIndex", 2, LIST_UNARY_PREDICATE, MoldReturnKind::Int),
    MoldSpec::exact(
        "FindIndexLax",
        2,
        LIST_UNARY_PREDICATE,
        MoldReturnKind::Pack,
    ),
    MoldSpec::exact("IndexOf", 2, ANY2, MoldReturnKind::Int),
    MoldSpec::exact("LastIndexOf", 2, ANY2, MoldReturnKind::Int),
    MoldSpec::exact("Sort", 1, LIST1, MoldReturnKind::List)
        .with_options(SORT_OPTIONS)
        .enforce_checker(),
    MoldSpec::exact("Unique", 1, LIST1, MoldReturnKind::List)
        .with_options(UNIQUE_OPTIONS)
        .enforce_checker(),
    MoldSpec::exact("Flatten", 1, LIST1, MoldReturnKind::List).enforce_checker(),
    MoldSpec::exact("Reverse", 1, ANY1, MoldReturnKind::Dynamic).enforce_checker(),
    MoldSpec::exact("Take", 2, LIST_OR_STREAM_ANY, MoldReturnKind::List).enforce_checker(),
    MoldSpec::exact(
        "TakeWhile",
        2,
        LIST_OR_STREAM_PREDICATE,
        MoldReturnKind::List,
    )
    .enforce_checker(),
    MoldSpec::exact("Drop", 2, LIST_OR_STREAM_ANY, MoldReturnKind::List).enforce_checker(),
    MoldSpec::exact(
        "DropWhile",
        2,
        LIST_OR_STREAM_PREDICATE,
        MoldReturnKind::List,
    )
    .enforce_checker(),
    MoldSpec::exact("Append", 2, LIST_ANY, MoldReturnKind::List).enforce_checker(),
    MoldSpec::exact("Prepend", 2, LIST_ANY, MoldReturnKind::List).enforce_checker(),
    MoldSpec::exact("Zip", 2, ANY2, MoldReturnKind::List).enforce_checker(),
    MoldSpec::exact("Enumerate", 1, LIST1, MoldReturnKind::List).enforce_checker(),
    MoldSpec::exact("Map", 2, LIST_UNARY_FUNCTION, MoldReturnKind::Dynamic).enforce_checker(),
    MoldSpec::exact("Filter", 2, LIST_UNARY_PREDICATE, MoldReturnKind::Dynamic).enforce_checker(),
    MoldSpec::exact("Fold", 3, LIST_ANY_BINARY_FUNCTION, MoldReturnKind::Dynamic).enforce_checker(),
    MoldSpec::exact(
        "Foldr",
        3,
        LIST_ANY_BINARY_FUNCTION,
        MoldReturnKind::Dynamic,
    )
    .enforce_checker(),
    MoldSpec::exact(
        "Reduce",
        3,
        LIST_ANY_BINARY_FUNCTION,
        MoldReturnKind::Dynamic,
    )
    .enforce_checker(),
    MoldSpec::exact("Sum", 1, LIST1, MoldReturnKind::Dynamic).enforce_checker(),
    MoldSpec::exact("Min", 1, LIST1, MoldReturnKind::Dynamic),
    MoldSpec::exact("Max", 1, LIST1, MoldReturnKind::Dynamic),
    MoldSpec::exact("If", 3, ANY3, MoldReturnKind::Dynamic).enforce_checker(),
    // Async combinators.
    MoldSpec::exact("Cancel", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync),
    MoldSpec::exact("All", 1, LIST1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync),
    MoldSpec::exact("Par", 1, LIST1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync)
        .enforce_checker(),
    MoldSpec::exact("ParMap", 2, LIST_UNARY_FUNCTION, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync)
        .enforce_checker(),
    MoldSpec::exact("Race", 1, LIST1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync),
    MoldSpec::exact("Timeout", 2, ASYNC_NUM, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::NestedAsync),
    // OS package mold constructors.
    MoldSpec::exact("Read", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::Effectful),
    MoldSpec::exact("ListDir", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::Effectful),
    MoldSpec::exact("Stat", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::Effectful),
    MoldSpec::exact("EnvVar", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::Effectful),
    MoldSpec::exact("ReadAsync", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::Effectful),
    MoldSpec::exact("HttpGet", 1, ANY1, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::Effectful),
    MoldSpec::exact("HttpPost", 2, ANY2, MoldReturnKind::Pack)
        .with_worker_boundary(WorkerMoldBoundary::Effectful),
    MoldSpec::exact("HttpRequest", 2, ANY2, MoldReturnKind::Pack)
        .with_options(HTTP_REQUEST_OPTIONS)
        .with_worker_boundary(WorkerMoldBoundary::Effectful),
];

/// Look up a builtin mold specification by name.
pub fn lookup_mold_spec(name: &str) -> Option<&'static MoldSpec> {
    MOLD_SPECS.iter().find(|spec| spec.name == name)
}

/// Look up the return-type kind of a builtin mold by name.
///
/// Returns `None` for user-defined molds (they always return `Pack` in
/// codegen — callers should treat `None` as "unknown builtin" and apply
/// their usual fallback).
pub fn lookup_mold_return_kind(name: &str) -> Option<MoldReturnKind> {
    lookup_mold_spec(name).map(|spec| spec.return_kind)
}

/// Return the runtime tag for a builtin mold, if the return kind is
/// statically known (not Dynamic, not user-defined).
///
/// Used by `src/codegen/lower.rs::expr_type_tag()` to decide whether a
/// `MoldInst` can be dispatched through `taida_io_stdout_with_tag`
/// directly (no `convert_to_string` fallback needed).
pub fn mold_return_tag(name: &str) -> Option<i64> {
    lookup_mold_return_kind(name).and_then(|k| k.tag())
}

pub fn lookup_worker_safety(name: &str) -> WorkerSafety {
    lookup_mold_spec(name)
        .map(|spec| spec.worker_safety)
        .unwrap_or(WorkerSafety::Unsafe)
}

pub fn lookup_worker_mold_boundary(name: &str) -> WorkerMoldBoundary {
    lookup_mold_spec(name)
        .map(|spec| spec.worker_boundary)
        .unwrap_or(WorkerMoldBoundary::Structural)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_returning_molds_map_to_str_tag() {
        // `Str[x]()` returns Lax[Str], not a bare Str.
        for name in [
            "Upper",
            "Lower",
            "Trim",
            "Replace",
            "ReplaceAll",
            "Repeat",
            "Pad",
            "PadLeft",
            "PadRight",
            "Join",
            "ToFixed",
            // Byte-level slice + single-alloc repeat/join.
            "ByteSlice",
            "StringRepeatJoin",
            "TypeName",
        ] {
            assert_eq!(mold_return_tag(name), Some(3), "{name} should be Str (3)");
        }
    }

    #[test]
    fn bool_returning_molds_map_to_bool_tag() {
        // `Bool[x]()` returns Lax[Bool], not a bare Bool.
        for name in ["TypeIs", "TypeExtends", "Contains"] {
            assert_eq!(mold_return_tag(name), Some(2), "{name} should be Bool (2)");
        }
        assert_eq!(
            mold_return_tag("Exists"),
            Some(4),
            "Exists should be Result[Bool] envelope (Pack tag 4)"
        );
    }

    #[test]
    fn span_aware_molds_map_to_expected_tags() {
        // Span-aware comparison molds return Bool; `SpanSlice` returns a
        // Pack sub-span `@(start, len)`.
        for name in ["SpanEquals", "SpanStartsWith", "SpanContains"] {
            assert_eq!(mold_return_tag(name), Some(2), "{name} should be Bool (2)");
        }
        assert_eq!(
            mold_return_tag("SpanSlice"),
            Some(4),
            "SpanSlice should be Pack (4) — returns @(start, len) sub-span"
        );
        // `StrOf[span, raw]()` materializes a span pack into an owned Str.
        assert_eq!(
            mold_return_tag("StrOf"),
            Some(3),
            "StrOf should be Str (3) — returns owned materialized string"
        );
    }

    #[test]
    fn int_returning_molds_map_to_int_tag() {
        // `Int[x]()` returns Lax[Int], not a bare Int.
        for name in [
            "Length",
            "Count",
            "IndexOf",
            "LastIndexOf",
            "FindIndex",
            "Floor",
            "Ceil",
            "Round",
            "Truncate",
            "BitAnd",
            "BitOr",
            "BitXor",
            "BitNot",
            // `ByteLength` returns bare Int (bytes in UTF-8 encoding).
            "ByteLength",
        ] {
            assert_eq!(mold_return_tag(name), Some(0), "{name} should be Int (0)");
        }
    }

    #[test]
    fn primitive_conversion_molds_map_to_pack_tag() {
        // Primitive conversion molds return Lax (= Pack), not their
        // primitive output type directly.
        for name in ["Int", "Float", "Bool", "Str"] {
            assert_eq!(
                mold_return_tag(name),
                Some(4),
                "{name}[x]() returns Lax (= Pack tag 4) — not the primitive output type"
            );
        }
    }

    #[test]
    fn list_returning_molds_map_to_list_tag() {
        for name in [
            "Chars",
            "Split",
            "Sort",
            "Unique",
            "Flatten",
            "Take",
            "TakeWhile",
            "Drop",
            "DropWhile",
            "Append",
            "Prepend",
            "Zip",
            "Enumerate",
            "BytesToList",
        ] {
            assert_eq!(mold_return_tag(name), Some(5), "{name} should be List (5)");
        }
    }

    #[test]
    fn dynamic_molds_return_none_for_tag() {
        for name in [
            "Map", "Filter", "Reverse", "Concat", "Slice", "Abs", "Clamp", "Sum", "Min", "Max",
            "If", "Fold", "Foldr", "Reduce",
        ] {
            assert_eq!(mold_return_tag(name), None, "{name} is argument-dependent");
        }
    }

    #[test]
    fn user_defined_molds_return_none() {
        assert_eq!(mold_return_tag("MyMold"), None);
        assert_eq!(mold_return_tag("UserPack"), None);
    }

    #[test]
    fn registry_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in MOLD_SPECS {
            assert!(
                seen.insert(spec.name),
                "duplicate mold spec for {}",
                spec.name
            );
        }
    }

    #[test]
    fn enforced_specs_have_kind_for_each_positional_arg() {
        for spec in MOLD_SPECS.iter().filter(|spec| spec.checker_enforced) {
            let max_arity = spec.arity_max.unwrap_or(spec.arity_min);
            assert!(
                spec.arg_kinds.len() >= max_arity,
                "{} enforces {} positional args but only has {} kind entries",
                spec.name,
                max_arity,
                spec.arg_kinds.len()
            );
        }
    }

    #[test]
    fn registry_option_names_are_unique_per_mold() {
        for spec in MOLD_SPECS {
            let mut seen = std::collections::HashSet::new();
            for option in spec.options {
                assert!(
                    seen.insert(option.name),
                    "duplicate option {} for {}",
                    option.name,
                    spec.name
                );
            }
        }
    }

    #[test]
    fn pack_returning_molds_map_to_pack_tag() {
        for name in [
            "Lax",
            "Result",
            "Async",
            "AsyncTask",
            "Par",
            "ParMap",
            "Gorillax",
            "Stream",
            "StreamFrom",
            "Molten",
            "Cage",
            "Find",
            "CharAt",
            "ToRadix",
            "ShiftL",
            "ShiftR",
            "ByteSet",
            // `ByteAt` returns Lax[Int] (Pack at runtime).
            "ByteAt",
        ] {
            assert_eq!(mold_return_tag(name), Some(4), "{name} should be Pack (4)");
        }
    }

    #[test]
    fn worker_safety_defaults_to_unsafe() {
        assert_eq!(lookup_worker_safety("HttpGet"), WorkerSafety::Unsafe);
        assert_eq!(
            lookup_worker_safety("DefinitelyUserDefined"),
            WorkerSafety::Unsafe
        );
    }

    #[test]
    fn worker_safety_marks_transparent_wrappers() {
        for name in ["Lax", "Result", "Gorillax", "RelaxedGorillax"] {
            assert_eq!(
                lookup_worker_safety(name),
                WorkerSafety::Transparent,
                "{name} should be transparent"
            );
        }
    }

    #[test]
    fn worker_safety_keeps_async_family_unsafe() {
        for name in [
            "Async",
            "AsyncReject",
            "AsyncTask",
            "Par",
            "ParMap",
            "All",
            "Race",
            "Timeout",
            "Cancel",
        ] {
            assert_eq!(
                lookup_worker_safety(name),
                WorkerSafety::Unsafe,
                "{name} should be unsafe"
            );
        }
    }

    #[test]
    fn worker_boundaries_classify_disallowed_worker_molds() {
        for name in ["Read", "ListDir", "Stat", "EnvVar", "ReadAsync", "HttpGet"] {
            assert_eq!(
                lookup_worker_mold_boundary(name),
                WorkerMoldBoundary::Effectful,
                "{name} should be effectful"
            );
        }

        for name in [
            "Molten",
            "Cage",
            "JSGet",
            "JSCall",
            "JSCallAsync",
            "JSNew",
            "JSSet",
        ] {
            assert_eq!(
                lookup_worker_mold_boundary(name),
                WorkerMoldBoundary::HostBoundary,
                "{name} should be a host boundary"
            );
        }

        for name in [
            "Async",
            "AsyncTask",
            "Par",
            "ParMap",
            "All",
            "Race",
            "Stream",
        ] {
            assert_eq!(
                lookup_worker_mold_boundary(name),
                WorkerMoldBoundary::NestedAsync,
                "{name} should be nested async"
            );
        }
    }
}

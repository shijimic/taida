use super::types::{Type, TypeRegistry};
use crate::lexer::Span;
use crate::parser::*;
/// Type checker for Taida Lang.
///
/// Performs type inference and type checking on the AST.
/// Key principles:
/// - No null/undefined (all types have default values)
/// - No implicit type conversion
/// - Structural subtyping (width subtyping)
/// - Scope-aware type inference
///
/// ## Type inference convention
///
/// `Type::Unknown` is a checker-local sentinel for in-flight inference,
/// recovery after an already emitted error, or an explicitly opaque
/// boundary that has not yet been modeled as a concrete Taida type. It is
/// not a subtype wildcard, and user-authored function, lambda, method, or
/// lowering boundaries must resolve to concrete types or report a
/// diagnostic.
use std::collections::{HashMap, HashSet};

/// bypass closure (2026-04-15, root fix): field names reserved
/// for compiler-internal use. A user-authored `Expr::BuchiPack` /
/// `Expr::TypeInst` literal that assigns any of these is rejected at
/// type-check time with `[E1617]`.
///
/// Rationale: compiler-generated packs set `__type`, `__value`,
/// `__default`, `__error`, `__tag`, `__items`, `__transforms`,
/// `__status` as *internal* tags to carry nominal-type identity and
/// invariants (e.g., `Regex` packs carry a validated `pattern` /
/// `flags` pair, `Lax` packs carry `has_value` + default, `Async` packs
/// carry a state tag). Allowing user code to set these fields lets
/// callers fabricate fake nominal packs that bypass the official
/// constructors' validation. The earlier narrower fix (literal
/// `__type <= "Regex"` only) was bypassed by variable binding
/// (`tag <= "Regex"; @(__type <= tag,...)`) and by expression
/// composition (`"Re" + "gex"`, `if(c, "Regex", "X")`). The root
/// remedy is to reject **all** user assignments to `__`-prefixed
/// field names, regardless of the value expression.
///
/// This is consistent with the Taida naming convention: `__`-prefix
/// denotes compiler-internal symbols. Compiler-generated packs are
/// built via Rust-level `Value::BuchiPack(...)` construction (in
/// `src/interpreter/*`, `src/js/runtime/*`, `src/codegen/lower/*`)
/// and IR ops — never through the AST `Expr::BuchiPack` /
/// `Expr::TypeInst` paths this check guards.
///
/// Field **reads** (`value.__type`, `lax.__value`, etc.) are rejected too.
/// Compiler-generated packs may still carry these internal fields, but
/// user-facing access must go through unmolding / public methods.
const RESERVED_INTERNAL_FIELD_PREFIX: &str = "__";
const MAX_CALL_ARGUMENTS: usize = 256;

/// Build-driver descriptor constructor names (`taida-lang/build`).
///
/// These five names denote build-driver descriptors consumed by
/// `taida build --unit / --plan / --all-units`, **not** ordinary runtime
/// values. The descriptor build path parses the entry module and matches
/// these `Expr::TypeInst` names directly (see `run_descriptor_build_driver`
/// in `src/main.rs`), bypassing the type checker entirely. When a program
/// is instead run / checked / single-target-built, the checker must reject
/// any attempt to use a descriptor value in a runtime position (`[E1532]`).
///
/// The names are reserved by the build driver regardless of whether
/// `taida-lang/build` is imported, so an importless `BuildUnit(...)` and an
/// imported one are detected identically. A user-declared type that shadows
/// one of these names (a class-like / mold definition in the same program)
/// is *not* treated as a descriptor — see `is_descriptor_type_name`.
const BUILD_DESCRIPTOR_NAMES: [&str; 5] = [
    "BuildUnit",
    "BuildPlan",
    "AssetBundle",
    "RouteAsset",
    "BuildHook",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CageBranch {
    Js,
    Build,
    File,
    Host,
}

impl CageBranch {
    fn label(self) -> &'static str {
        match self {
            Self::Js => "JS",
            Self::Build => "Build",
            Self::File => "File",
            Self::Host => "Host",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "JS" => Some(Self::Js),
            "Build" => Some(Self::Build),
            "File" => Some(Self::File),
            "Host" => Some(Self::Host),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct CageRunnerType {
    branch: CageBranch,
    output: Type,
    async_boundary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchInfo {
    None,
    Molten(CageBranch),
    GorillaxValue(CageBranch),
}

#[derive(Debug, Clone)]
struct MoldHeaderSpec {
    header_args: Vec<MoldHeaderArg>,
}

struct MoldBindingDef<'a> {
    kind: &'a str,
    name: &'a str,
    span: &'a Span,
}

/// Type checking error.
///
/// ## Error code convention (N-68)
///
/// Error codes follow the pattern `[EXXXX]` where:
/// - `E1301` -- arity errors (too many/few arguments)
/// - `E1302` -- default parameter reference errors
/// - `E1303` -- default parameter type mismatch
/// - `E1501` -- same-scope redefinition
/// - `E1502` -- undefined variable / deprecated syntax
/// - `E1503` -- unsupported partial application
/// - `E1504` -- placeholder outside pipeline
/// - `E1505` -- partial application slot count mismatch
/// - `E1506` -- argument type mismatch
/// - `E1507` -- builtin arity mismatch
/// - `E1508` -- method argument error
/// - `E1509` -- unknown method / generic constraint violation
/// - `E1510` -- non-callable invocation
/// - `E1601` -- return type mismatch
/// - `E1605` -- comparison type mismatch
/// - `E1606` -- logical operator type mismatch
/// - `E1607` -- unary operator type mismatch
/// - `E1608` -- unknown enum variant
/// - `E1618` -- enum variant order mismatch across module boundary/// - `E1611` -- reserved backend capability rejection
/// - `E1612` -- WASM backend capability rejection
/// - `E1613` -- TypeExtends does not accept enum variant literals
/// - `E1617` -- Regex invariant rejection. Two emitters share this code (both ):
/// (1) WASM backend Regex rejection (`emit_wasm_c::validate_regex_api_for_wasm`) —
/// `Regex(...)` ctor / `.match(re)` / `.search(re)` are unsupported on wasm;
/// (2) Manual `__type <= "Regex"` BuchiPack construction rejection
/// (`checker::check_mold_errors_in_expr`) — nominal `:Regex` must be produced
/// by its official constructor to enforce eager pattern validation.
///
/// Some internal diagnostic messages (e.g., inheritance validation, mold binding
/// checks) do not yet carry error codes. These are emitted during registration
/// and are not user-facing in the same way as expression-level diagnostics.
#[derive(Debug, Clone)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Type error at line {}, column {}: {}",
            self.span.line, self.span.column, self.message
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FunctionHintDiagnostic {
    FunctionArg,
    MethodArg,
}

impl FunctionHintDiagnostic {
    fn code(self) -> &'static str {
        match self {
            FunctionHintDiagnostic::FunctionArg => "E1506",
            FunctionHintDiagnostic::MethodArg => "E1508",
        }
    }
}

/// Argument-shape category of a `taida-lang/crypto` export. Drives the
/// per-symbol `[E1506]` argument-type checks and the registered function
/// signature (return type + arity).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CryptoSym {
    /// 1 arg `Str | Bytes` -> `Str` (lowercase hex digest).
    /// sha256 / sha512 / sha384 / sha224.
    Hash,
    /// 2 args `Str | Bytes` (key, data) -> `Str` (hex). hmacSha256.
    Hmac,
    /// 2 args `Str | Bytes` -> `Bool`. constantTimeEquals.
    Equals,
    /// 1 arg `Str | Bytes` -> `Str`. hexEncode / base64Encode.
    Encode,
    /// 1 arg `Str` -> `Lax[Bytes]`. hexDecode / base64Decode.
    Decode,
    /// 1 arg `Int` -> `Bytes`. randomBytes.
    Random,
}

impl CryptoSym {
    /// Map an export name to its argument-shape category. Returns `None`
    /// for names that are not part of the crypto surface (so a typo'd
    /// import still routes through the uniform unknown-symbol diagnostic).
    fn from_export(name: &str) -> Option<Self> {
        Some(match name {
            "sha256" | "sha512" | "sha384" | "sha224" => CryptoSym::Hash,
            "hmacSha256" => CryptoSym::Hmac,
            "constantTimeEquals" => CryptoSym::Equals,
            "hexEncode" | "base64Encode" => CryptoSym::Encode,
            "hexDecode" | "base64Decode" => CryptoSym::Decode,
            "randomBytes" => CryptoSym::Random,
            _ => return None,
        })
    }

    /// Registered return type of the symbol.
    fn return_type(self) -> Type {
        match self {
            CryptoSym::Hash | CryptoSym::Hmac | CryptoSym::Encode => Type::Str,
            CryptoSym::Equals => Type::Bool,
            CryptoSym::Decode => Type::Generic("Lax".to_string(), vec![Type::Bytes]),
            CryptoSym::Random => Type::Bytes,
        }
    }

    /// Maximum arity (parameter count upper bound).
    fn max_arity(self) -> usize {
        match self {
            CryptoSym::Hmac | CryptoSym::Equals => 2,
            _ => 1,
        }
    }
}

/// Position context for the build-descriptor runtime-use pass ([E1532]).
///
/// `Allowed` marks the three positions where a build descriptor may appear
/// (a top-level export value, a descriptor field, a top-level binding RHS);
/// `Runtime` marks every other position, where a descriptor value is a
/// misuse and is rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DescriptorUseCtx {
    Allowed,
    Runtime,
}

/// Type checker state.
pub struct TypeChecker {
    pub registry: TypeRegistry,
    pub errors: Vec<TypeError>,
    /// Scope stack for variable type tracking.
    /// Each scope maps variable names to their inferred types.
    scope_stack: Vec<HashMap<String, Type>>,
    /// Function return types (name -> return type).
    func_types: HashMap<String, Type>,
    /// Function parameter counts (name -> arity upper bound).
    func_param_counts: HashMap<String, usize>,
    /// Function parameter types (name -> param types). Used for partial application type inference.
    func_param_types: HashMap<String, Vec<Type>>,
    /// Imported local names for `taida-lang/crypto::sha256`.
    crypto_sha256_funcs: HashSet<String>,
    /// Imported local names for every `taida-lang/crypto` symbol, mapped to
    /// the per-symbol argument-shape validator (hash / hmac / encode / decode
    /// / random / equals). Drives the generalized `[E1506]` argument checks.
    crypto_funcs: HashMap<String, CryptoSym>,
    /// Function definitions retained for expected-type body inference.
    func_defs: HashMap<String, FuncDef>,
    /// Scope depth where a function name was bound as the function value.
    /// Used to distinguish the function binding from an inner variable shadow.
    func_def_scope_depths: HashMap<String, usize>,
    /// Generic function definitions keyed by function name.
    generic_func_defs: HashMap<String, FuncDef>,
    /// Function definitions rejected during registration.
    invalid_func_defs: HashSet<String>,
    /// Function names already seen during first-pass registration.
    seen_func_defs: HashSet<String>,
    /// Concrete type-like names declared anywhere in the current program.
    declared_concrete_type_names: HashSet<String>,
    /// Custom mold field definitions (name -> raw AST fields).
    /// Used for `[]` / `()` binding validation.
    mold_field_defs: HashMap<String, Vec<FieldDef>>,
    /// Custom mold header declarations (name -> formal header args).
    mold_header_specs: HashMap<String, MoldHeaderSpec>,
    /// Declared formal header arity for named types/molds.
    declared_header_arities: HashMap<String, usize>,
    /// Whether we are currently inside a pipeline expression.
    /// Used to allow `_` (Placeholder) in pipeline context while rejecting it elsewhere.
    in_pipeline: bool,
    /// True while the comparison-diagnostic walker is speculatively inferring
    /// a subtree. Main inference paths use this to avoid recursively
    /// re-starting the same E1605-only walk from nested containers.
    in_comparison_error_walk: bool,
    /// Spans of arguments seen at `setOf(...)` call sites. A heterogeneous
    /// literal fed straight into `setOf` is a legitimate value-tagged Set
    /// (per-element kind array), so the `[E0401]` the literal itself reports
    /// must not reject the whole program; `check_program` excuses exactly
    /// these spans at the end. Direct bindings (`xs <= @[1, "x"]`) and
    /// unrelated diagnostics stay reported.
    setof_arg_spans: Vec<Span>,
    /// Source file path — used for resolving import paths to validate export symbols.
    source_file: Option<std::path::PathBuf>,
    /// Compile target for backend-aware diagnostics.
    compile_target: CompileTarget,
    /// Local names that resolve to taida-lang/net's `httpServe`.
    net_http_serve_symbols: HashSet<String>,
    /// Local enum names that resolve to taida-lang/net's `HttpProtocol`.
    net_http_protocol_type_names: HashSet<String>,
    /// Local names that resolve to APIs with externally visible effects.
    worker_effect_symbols: HashSet<String>,
    /// Local names that resolve to external addon / host boundaries.
    worker_addon_symbols: HashSet<String>,
    /// Local addon function imports whose package/function identity is known.
    worker_addon_bindings: HashMap<String, WorkerAddonBinding>,
    /// Scope-aligned metadata for branch-carrying values. `Type::Molten`
    /// remains the public type; this side table records the branch only
    /// when the checker can prove it.
    branch_scope_stack: Vec<HashMap<String, BranchInfo>>,
    /// Scope-aligned compile-time string constants. `None` marks a local
    /// shadow that is known not to be a compile-time string constant.
    string_const_scope_stack: Vec<HashMap<String, Option<String>>>,
    /// Optional host capability manifest injected by a build adapter or test
    /// fixture. When present, every statically resolvable HostCapability pair
    /// must be declared here.
    host_capability_manifest: Option<HashSet<(String, String)>>,
    /// stack of type parameter declarations for the
    /// enclosing generic functions. Pushed on `Statement::FuncDef` body
    /// entry, popped on exit. Used to resolve constrained type variables
    /// inside the body (e.g. arithmetic on `T <=:Num`, calling `F <=:T =>:T`).
    current_func_type_params: Vec<Vec<TypeParam>>,
    /// Re-entrancy guard for expected-type named function body inference.
    hinted_func_stack: Vec<String>,
    /// Top-level variable names whose bound value is a build-driver
    /// descriptor (`BuildUnit` / `BuildPlan` / `AssetBundle` / `RouteAsset`
    /// / `BuildHook`). Populated during the descriptor-usage pass so that a
    /// descriptor reached through a `name <= BuildUnit(...)` binding is still
    /// recognised when `name` is later used in a runtime position. Bindings
    /// are the only allow-listed indirection (they let a descriptor reach a
    /// top-level export); any *other* use of such a name is rejected with
    /// `[E1532]`.
    descriptor_binding_names: HashSet<String>,
    /// Names user-declared as class-like / mold types in the current program
    /// that collide with a reserved descriptor name. Such a name resolves to
    /// the user's own type, not a build descriptor, so it is excluded from
    /// `[E1532]` detection.
    descriptor_shadow_names: HashSet<String>,
    /// Names currently shadowed by a function parameter / lambda parameter /
    /// local binding while the `[E1532]` descriptor-use pass walks a nested
    /// scope. A local `unit: Str` argument must not be mistaken for a
    /// same-named top-level `unit <= BuildUnit(...)` binding. Saved and
    /// restored at every scope boundary (function body, lambda, error
    /// ceiling, branch arm).
    descriptor_scope_shadows: HashSet<String>,
    /// While `infer_expr_type` descends into a pipeline stage that
    /// contains a `_` placeholder (`data => f(_, a)`), this holds the
    /// *previous stage's* result type — the value the runtime injects at
    /// the placeholder (F62B-025 rule 1). The `Expr::Placeholder` arm
    /// reads it so `_` carries the piped value's type into argument and
    /// operand validation at any nesting depth. `None` outside pipeline
    /// stages.
    pipeline_placeholder_type: Option<Type>,
    /// Typed HIR / expression type table. `infer_expr_type` records
    /// every observed `Expr` here so codegen lowering can answer
    /// "is this expression Bool?" by looking up the recorded type.
    pub typed_expr_table: super::typed_hir::TypedExprTable,
    /// F62B-021: generic functions whose body passes a type parameter into
    /// a host-call Out slot (`Uncage[b, m, T]` / `HostCall[steps, T]`).
    /// Maps function name → the type parameters (declared order) whose
    /// schemas must be supplied by call sites. Such functions require
    /// explicit type arguments at every call site.
    pub schema_passing_generic_funcs: HashMap<String, Vec<String>>,
    /// Bidirectional hints for empty list literals, keyed by AST node id.
    /// Seeded by `seed_empty_literal_hints` when an expected type reaches a
    /// literal tree (annotated bindings, annotated call arguments, type
    /// constructors); consulted by the plain `ListLit` inference arm so an
    /// empty `@[]` in a hinted position types as the expected list instead
    /// of `@[?]`.
    pub(super) empty_literal_hints: HashMap<usize, Type>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileTarget {
    Neutral,
    Interpreter,
    Js,
    Native,
    WasmMin,
    WasmWasi,
    WasmEdge,
    WasmFull,
}

impl CompileTarget {
    /// Native and wasm targets that lower through the
    /// C / wasm-C runtime use regular call instructions for mutual
    /// recursion (no trampoline). Deep mutual cycles therefore overflow
    /// the OS stack at runtime instead of falling back to bounded
    /// iteration. The checker uses this predicate to gate the
    /// `[E0700]` mutual-recursion reject so Interpreter / JS programs
    /// continue to compile while Native and wasm-* programs hard-fail
    /// before they reach the segfault path.
    pub(crate) fn is_native_lowering(self) -> bool {
        matches!(
            self,
            Self::Native | Self::WasmMin | Self::WasmWasi | Self::WasmEdge | Self::WasmFull
        )
    }

    fn label(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Interpreter => "interpreter",
            Self::Js => "js",
            Self::Native => "native",
            Self::WasmMin => "wasm-min",
            Self::WasmWasi => "wasm-wasi",
            Self::WasmEdge => "wasm-edge",
            Self::WasmFull => "wasm-full",
        }
    }
}

#[derive(Debug, Clone)]
struct WorkerAddonBinding {
    package_id: String,
    function_name: String,
    decision: WorkerAddonDecision,
}

#[derive(Debug, Clone)]
enum WorkerAddonDecision {
    Allow,
    Deny {
        code: &'static str,
        reason: String,
        active_policy: String,
        effective_claim: String,
    },
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut checker = Self {
            registry: TypeRegistry::new(),
            errors: Vec::new(),
            scope_stack: vec![HashMap::new()], // global scope
            func_types: HashMap::new(),
            func_param_counts: HashMap::new(),
            func_param_types: HashMap::new(),
            crypto_sha256_funcs: HashSet::new(),
            crypto_funcs: HashMap::new(),
            func_defs: HashMap::new(),
            func_def_scope_depths: HashMap::new(),
            generic_func_defs: HashMap::new(),
            invalid_func_defs: HashSet::new(),
            seen_func_defs: HashSet::new(),
            declared_concrete_type_names: HashSet::new(),
            mold_field_defs: HashMap::new(),
            mold_header_specs: HashMap::new(),
            declared_header_arities: HashMap::new(),
            in_pipeline: false,
            in_comparison_error_walk: false,
            setof_arg_spans: Vec::new(),
            source_file: None,
            compile_target: CompileTarget::Neutral,
            net_http_serve_symbols: HashSet::new(),
            net_http_protocol_type_names: HashSet::new(),
            worker_effect_symbols: HashSet::new(),
            worker_addon_symbols: HashSet::new(),
            worker_addon_bindings: HashMap::new(),
            branch_scope_stack: vec![HashMap::new()],
            string_const_scope_stack: vec![HashMap::new()],
            host_capability_manifest: None,
            current_func_type_params: Vec::new(),
            hinted_func_stack: Vec::new(),
            descriptor_binding_names: HashSet::new(),
            descriptor_shadow_names: HashSet::new(),
            descriptor_scope_shadows: HashSet::new(),
            pipeline_placeholder_type: None,
            typed_expr_table: super::typed_hir::TypedExprTable::new(),
            schema_passing_generic_funcs: HashMap::new(),
            empty_literal_hints: HashMap::new(),
        };
        // C19B-002 (import-less): the C19 interactive variants are core-bundled
        // in `src/codegen/lower/core.rs` (import-less parity with interpreter/
        // JS), so their typed signatures must be pinned whether or not the
        // user writes `>>> taida-lang/os => @(runInteractive)`. Installing
        // them unconditionally at checker construction guarantees that bare
        // calls (`runInteractive(...).__value.stdout`) are caught at
        // `taida check` time, matching the imported path.
        checker.install_core_bundled_os_pins();
        checker
    }

    fn is_core_builtin_name(name: &str) -> bool {
        Self::core_builtin_arity(name).is_some()
    }

    fn core_builtin_allows_unknown_return(name: &str) -> bool {
        matches!(
            name,
            "dnsResolve"
                | "tcpConnect"
                | "tcpListen"
                | "tcpAccept"
                | "socketSend"
                | "socketSendAll"
                | "socketSendBytes"
                | "socketRecv"
                | "socketRecvBytes"
                | "socketRecvExact"
                | "udpBind"
                | "udpSendTo"
                | "udpRecvFrom"
                | "socketClose"
                | "listenerClose"
                | "udpClose"
                | "poolCreate"
                | "poolAcquire"
                | "poolRelease"
                | "poolClose"
        )
    }

    pub fn set_source_file(&mut self, path: &std::path::Path) {
        self.source_file = Some(path.to_path_buf());
    }

    pub fn set_compile_target(&mut self, target: CompileTarget) {
        self.compile_target = target;
    }

    pub fn set_host_capability_manifest<I, N, K>(&mut self, capabilities: I)
    where
        I: IntoIterator<Item = (N, K)>,
        N: Into<String>,
        K: Into<String>,
    {
        self.host_capability_manifest = Some(
            capabilities
                .into_iter()
                .map(|(name, kind)| (name.into(), kind.into()))
                .collect(),
        );
    }

    /// Check if a type contains Unknown anywhere in its structure.
    pub(super) fn contains_unknown(ty: &Type) -> bool {
        match ty {
            Type::Unknown => true,
            Type::List(inner) => Self::contains_unknown(inner),
            Type::Generic(_, args) => args.iter().any(Self::contains_unknown),
            Type::Function(params, ret) => {
                params.iter().any(Self::contains_unknown) || Self::contains_unknown(ret)
            }
            _ => false,
        }
    }

    /// [E1520]: Is this type a "value-absence" type that must not
    /// appear on Taida surface as a return / parameter / type argument?
    ///
    /// Detects (shallow):
    /// - `Type::Unit` (resolved from `:Unit` / `:Void` named types)
    /// - `Type::BuchiPack` with no fields (resolved from `:@()`)
    /// - `Type::Named("Unit" | "Void")` (un-resolved alias form)
    ///
    /// PHILOSOPHY.md I の系「値の不在は値の不在」と CLAUDE.md「Taida 実装側
    /// の絶対ルール」を整合的に実装するための判定 helper。
    pub(super) fn is_unit_like_type(ty: &Type) -> bool {
        match ty {
            Type::Unit => true,
            Type::BuchiPack(fields) if fields.is_empty() => true,
            Type::Named(name) if name == "Unit" || name == "Void" => true,
            _ => false,
        }
    }

    /// [E1520]: Recursive check that detects value-absence types
    /// nested inside `Async[Unit]`, `Result[Unit, _]`, `Optional[Unit]`,
    /// `List[Unit]`, `Function([Unit], Unit)`, **BuchiPack fields**, etc.
    ///
    /// The shallow `is_unit_like_type` is preserved for direct comparisons
    /// (e.g. checking whether the immediate return type is `:Unit`). This
    /// recursive variant is intended for callers that need to reject
    /// `Async[Unit]` annotations, `Optional[Void]` annotations, and other
    /// nested forms — every Type::Unit / empty BuchiPack hidden in the
    /// composite is reachable from Taida surface.
    ///
    /// **Round-4 補強**: `Type::BuchiPack(fields)` の非空 fields 内に
    /// `:Unit` / `:Void` / `:@()` を書く抜け道 (`:@(payload: @())` 等) を
    /// 塞ぐため、非空 BuchiPack の各 field type を再帰的にチェック。
    pub(super) fn contains_unit_like_type(ty: &Type) -> bool {
        if Self::is_unit_like_type(ty) {
            return true;
        }
        match ty {
            Type::List(inner) => Self::contains_unit_like_type(inner),
            Type::Generic(_, args) => args.iter().any(Self::contains_unit_like_type),
            Type::Function(params, ret) => {
                params.iter().any(Self::contains_unit_like_type)
                    || Self::contains_unit_like_type(ret)
            }
            // F42 sweep (R4): BuchiPack 非空 fields 内の Unit 抜け道を塞ぐ。
            Type::BuchiPack(fields) => fields
                .iter()
                .any(|(_, field_ty)| Self::contains_unit_like_type(field_ty)),
            _ => false,
        }
    }

    /// `Num` is a generic-constraint marker (`[T <= :Num]`), not a wire
    /// value type — the constraints reference says so explicitly. A
    /// value-position annotation (`=> :Num`, `x: Num`, nested forms)
    /// must therefore be rejected: the lowering would have to guess a
    /// concrete representation and silently picked Int, rendering Float
    /// returns as raw bit patterns.
    pub(super) fn contains_constraint_marker_type(ty: &Type) -> bool {
        if matches!(ty, Type::Num) {
            return true;
        }
        match ty {
            Type::List(inner) => Self::contains_constraint_marker_type(inner),
            Type::Generic(_, args) => args.iter().any(Self::contains_constraint_marker_type),
            Type::Function(params, ret) => {
                params.iter().any(Self::contains_constraint_marker_type)
                    || Self::contains_constraint_marker_type(ret)
            }
            Type::BuchiPack(fields) => fields
                .iter()
                .any(|(_, field_ty)| Self::contains_constraint_marker_type(field_ty)),
            _ => false,
        }
    }

    fn push_wired_constraint_error(&mut self, subject: &str, actual: &Type, span: &Span) {
        self.errors.push(TypeError {
            message: format!(
                "[E3601] {} must satisfy Wired[T], got {}. \
                 Hint: use Str / Int / Float / Bool / Bytes, a non-empty buchi pack whose fields are wired, a wired list, WebRequest, WebResponse, or HostCapability.",
                subject, actual
            ),
            span: span.clone(),
        });
    }

    fn contains_unresolved_type_var(&self, ty: &Type) -> bool {
        match ty {
            Type::Named(name) => self.registry.get_type_fields(name).is_none(),
            Type::List(inner) => self.contains_unresolved_type_var(inner),
            Type::Generic(_, args) => args.iter().any(|a| self.contains_unresolved_type_var(a)),
            Type::BuchiPack(fields) => fields
                .iter()
                .any(|(_, t)| self.contains_unresolved_type_var(t)),
            Type::Function(params, ret) => {
                params.iter().any(|p| self.contains_unresolved_type_var(p))
                    || self.contains_unresolved_type_var(ret)
            }
            _ => false,
        }
    }

    /// Check whether a type is a mold-defined Named type.
    ///
    /// Custom mold instantiations (e.g. `AlwaysFail[x]()`) return
    /// `Type::Named("AlwaysFail")` from `infer_expr_type`, but the
    /// checker cannot predict what the mold's `solidify` function
    /// actually produces at runtime. We suppress E1601 in this case.
    fn is_mold_defined_named(&self, ty: &Type) -> bool {
        matches!(ty, Type::Named(name) if self.registry.mold_defs.contains_key(name))
    }

    /// [E1523]: detect built-in type names mistakenly written
    /// as Mold header type variables. `Mold[Int]` parses as a type
    /// variable named `Int`, which collides with the built-in `Int` type
    /// and is almost always a misuse for `Mold[:Int]` (concrete type
    /// argument) or `Mold[T <=:Int]` (constrained type variable).
    ///
    /// Built-in type names that trigger this diagnostic:
    /// - Primitive / scalar: `Int`, `Float`, `Num`, `Number`, `Str`,
    /// `String`, `Bytes`, `Bool`, `Boolean`
    /// - Special / forbidden surface types: `Unit`, `Void`, `JSON`, `Molten`
    /// - Built-in type constraints / molds: `Wired`, `HostCall`, `HostStep`,
    /// `HostCapability`, `Lax`, `Result`, `Async`,
    /// `Optional`, `Stream`, `Mold`, `TODO`, `Log`, `Slice`, `Concat`
    /// The primitive value-type names protected by [E1538]: a user
    /// redefinition of these can never be referenced (annotation
    /// resolution always picks the built-in), so the definition site
    /// rejects the shadowing. Container/mold names (Lax / Result /
    /// Async / ...) are NOT in this set — the type guide legitimately
    /// re-states their definitions as documentation examples and the
    /// generic-inheritance tests build on user molds of those names;
    /// their consistency is the mold-spec layer's concern.
    pub(super) fn is_primitive_value_type_name(name: &str) -> bool {
        matches!(
            name,
            "Int"
                | "Integer"
                | "Float"
                | "Num"
                | "Str"
                | "String"
                | "Bytes"
                | "Bool"
                | "Boolean"
                | "Unit"
                | "Void"
        )
    }

    pub(super) fn is_builtin_type_name(name: &str) -> bool {
        matches!(
            name,
            "Int"
                | "Integer"
                | "Float"
                | "Num"
                | "Str"
                | "String"
                | "Bytes"
                | "Bool"
                | "Boolean"
                | "Unit"
                | "Void"
                | "JSON"
                | "Molten"
                | "Wired"
                | "HostCall"
                | "HostStep"
                | "HostCapability"
                | "InCage"
                | "Uncage"
                | "Lax"
                | "Result"
                | "Async"
                | "Optional"
                | "Stream"
                | "Mold"
                | "TODO"
                | "Log"
                | "Slice"
                | "Concat"
                | "Gorillax"
                | "RelaxedGorillax"
        )
    }

    /// Push a new scope (e.g., entering a function body).
    fn push_scope(&mut self) {
        self.scope_stack.push(HashMap::new());
        self.branch_scope_stack.push(HashMap::new());
        self.string_const_scope_stack.push(HashMap::new());
    }

    /// Pop a scope (e.g., leaving a function body).
    fn pop_scope(&mut self) {
        self.scope_stack.pop();
        self.branch_scope_stack.pop();
        self.string_const_scope_stack.pop();
    }

    fn define_branch_info(&mut self, name: &str, info: BranchInfo) {
        if let Some(scope) = self.branch_scope_stack.last_mut() {
            scope.insert(name.to_string(), info);
        }
    }

    fn define_string_const(&mut self, name: &str, value: Option<String>) {
        if let Some(scope) = self.string_const_scope_stack.last_mut() {
            scope.insert(name.to_string(), value);
        }
    }

    fn define_string_const_from_expr(&mut self, name: &str, expr: &Expr) {
        let value = self.string_const_expr(expr);
        self.define_string_const(name, value);
    }

    fn string_const_expr(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::StringLit(value, _) => Some(value.clone()),
            Expr::Ident(name, _) => self.lookup_string_const(name),
            _ => None,
        }
    }

    fn http_serve_tls_pack_shape(&self, expr: &Expr) -> Option<(Span, bool)> {
        match expr {
            Expr::BuchiPack(fields, span) => Some((span.clone(), !fields.is_empty())),
            Expr::Ident(name, span) => match self.lookup_var(name) {
                Some(Type::BuchiPack(fields)) => Some((span.clone(), !fields.is_empty())),
                _ => None,
            },
            _ => None,
        }
    }

    /// Find project root by walking up from the given directory.
    /// `.taida/` is state/config storage, not a project-root marker; otherwise
    /// `~/.taida` can make `$HOME` look like the active project root.
    fn find_project_root(start_dir: &std::path::Path) -> std::path::PathBuf {
        crate::project_root::find_project_root(start_dir)
    }

    fn define_var(&mut self, name: &str, ty: Type) {
        self.define_var_with_span(name, ty, None);
    }

    fn define_var_silent(&mut self, name: &str, ty: Type) {
        if let Some(scope) = self.scope_stack.last_mut() {
            scope.insert(name.to_string(), ty);
        }
        self.define_branch_info(name, BranchInfo::None);
        self.define_string_const(name, None);
    }

    /// Define a variable with a span for duplicate detection.
    fn define_var_with_span(&mut self, name: &str, ty: Type, span: Option<&Span>) -> bool {
        if let Some(scope) = self.scope_stack.last_mut() {
            if let Some(span) = span
                && scope.contains_key(name)
            {
                self.errors.push(TypeError {
                        message: format!(
                            "[E1501] Name '{}' is already defined in this scope. \
                             Redefinition in the same scope is not allowed. \
                             Hint: Use a different name, or define it in an inner scope (shadowing is allowed).",
                            name
                        ),
                        span: span.clone(),
                    });
                return false;
            }
            scope.insert(name.to_string(), ty);
        }
        self.define_branch_info(name, BranchInfo::None);
        self.define_string_const(name, None);
        true
    }

    /// True if `name` in an intermediate pipeline
    /// step should be treated as a function-like reference (classic
    /// pipeline semantics: call it with the current value). False means
    /// bind-and-forward: the current step's value is bound to `name` and
    /// passed through unchanged.
    ///
    /// A name is considered callable if:
    /// - the variable is declared with a `Function` type in scope, or
    /// - the name is registered as a user-defined (possibly generic)
    /// function / type / mold, or
    /// - it is a known builtin identifier.
    fn is_pipeline_callable_ident(&self, name: &str) -> bool {
        if let Some(ty) = self.lookup_var(name)
            && matches!(ty, Type::Function(_, _))
        {
            return true;
        }
        if self.func_types.contains_key(name)
            || self.generic_func_defs.contains_key(name)
            || self.declared_concrete_type_names.contains(name)
            || self.registry.mold_defs.contains_key(name)
        {
            return true;
        }
        Self::is_core_builtin_name(name)
    }

    /// the statically-known Function type of a
    /// callable pipeline stage name — locals via `lookup_var`, user and
    /// prelude functions via `func_types`. Returns None for molds and
    /// constructors whose pipeline call form is not a plain function value,
    /// so their first stages keep inferring without a hint.
    fn pipeline_stage_function_type(&self, name: &str) -> Option<Type> {
        if let Some(ty) = self.lookup_var(name)
            && matches!(ty, Type::Function(_, _))
        {
            return Some(ty);
        }
        self.func_types.get(name).cloned()
    }

    /// Get all variable names and types visible in the current scope (for LSP completion).
    pub fn all_visible_vars(&self) -> Vec<(String, Type)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Walk from innermost to outermost, skip duplicates
        for scope in self.scope_stack.iter().rev() {
            for (name, ty) in scope {
                if seen.insert(name.clone()) {
                    result.push((name.clone(), ty.clone()));
                }
            }
        }
        result
    }

    /// Get all registered function names and their return types (for LSP completion).
    pub fn all_functions(&self) -> Vec<(String, Type)> {
        self.func_types
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// F62B-040: schema-passing metadata for codegen lowering —
    /// `name -> (declared type params, schema params)` for every callable
    /// this checker knows to be schema-passing, local and imported alike
    /// (imported ones under their local aliases). The driver hands this
    /// to `Lowering::set_schema_passing_metadata` next to the typed-HIR
    /// table so the checker's call-form enforcement and the lowering's
    /// hidden schema params never disagree across module boundaries.
    pub fn schema_passing_metadata(
        &self,
    ) -> std::collections::HashMap<String, (Vec<String>, Vec<String>)> {
        self.schema_passing_generic_funcs
            .iter()
            .filter_map(|(name, schema)| {
                self.generic_func_defs.get(name).map(|fd| {
                    (
                        name.clone(),
                        (
                            fd.type_params
                                .iter()
                                .map(|tp| tp.name.clone())
                                .collect::<Vec<String>>(),
                            schema.clone(),
                        ),
                    )
                })
            })
            .collect()
    }

    /// Check an entire program. Collects type definitions first,
    /// then checks all statements.
    pub fn check_program(&mut self, program: &Program) {
        self.seen_func_defs.clear();
        self.func_def_scope_depths.clear();
        self.declared_concrete_type_names.clear();
        self.worker_effect_symbols.clear();
        self.worker_addon_symbols.clear();
        self.worker_addon_bindings.clear();
        // Phase-2 review Low-2: AST node ids restart at 1 per parse — a
        // reused checker must not leak per-program node-id-keyed state
        // into the next program.
        self.empty_literal_hints.clear();
        self.schema_passing_generic_funcs.clear();
        for stmt in &program.statements {
            match stmt {
                Statement::EnumDef(ed) => {
                    // [E1538]: a user definition under a built-in type name
                    // would be registered but never referenced — annotation
                    // resolution always picks the built-in, so the value is
                    // definable yet unusable (and the resulting E1601 reads
                    // as nonsense: "returns Num, expected Num"). Reject the
                    // shadowing at the definition site instead.
                    if Self::is_primitive_value_type_name(&ed.name) {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1538] Enum '{}' shadows a built-in type name. Built-in type names cannot be redefined — annotation positions always resolve to the built-in, so the definition would be unusable. Choose a different name. See docs/reference/diagnostic_codes.md [E1538].",
                                ed.name
                            ),
                            span: ed.span.clone(),
                        });
                    }
                    self.declared_concrete_type_names.insert(ed.name.clone());
                }
                // (E30 Sub-step 2.1) ClassLikeDef 単一 variant + kind dispatch (旧 TypeDef/MoldDef/InheritanceDef を統合)
                Statement::ClassLikeDef(cl) => {
                    // [E1538]: same shadowing guard for BuchiPack / Mold /
                    // Inheritance definitions.
                    if Self::is_primitive_value_type_name(&cl.name) {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1538] Type definition '{}' shadows a built-in type name. Built-in type names cannot be redefined — annotation positions always resolve to the built-in, so the definition would be unusable. Choose a different name. See docs/reference/diagnostic_codes.md [E1538].",
                                cl.name
                            ),
                            span: cl.span.clone(),
                        });
                    }
                    // BuchiPack / Mold / Inheritance いずれも子型名を登録
                    self.declared_concrete_type_names.insert(cl.name.clone());
                }
                // N-64: Intentional catch-all — the first pass only collects ClassLikeDef
                // and EnumDef names for forward-reference resolution.
                // All other statement kinds (Assignment, FuncDef, Expr, etc.) are
                // processed in the second pass by check_statement().
                _ => {}
            }
        }

        // Predeclare header metadata so generic inheritance validation is not source-order dependent.
        self.predeclare_header_metadata(&program.statements);

        // First pass: register base type definitions and function signatures before inheritances.
        // (E30 Sub-step 2.1) ClassLikeDef + kind discriminator
        for stmt in &program.statements {
            let is_inheritance = matches!(
                stmt,
                Statement::ClassLikeDef(cl) if cl.is_inheritance()
            );
            if !is_inheritance {
                self.register_types(stmt);
            }
        }

        // F62B-038 #11 / F62B-040: schema-passing is transitive over
        // explicit generic calls (`outer[T] = inner[T](..)`), so the
        // per-function sets are a whole-program fixpoint, not a
        // per-definition scan. Built AFTER the register_types pass so the
        // imported schema-passing generics registered there (under their
        // local aliases) seed the closure — a local forwarding generic
        // whose callee is imported is detected like a local one. Shared
        // closure with codegen lowering: the checker's call-form
        // enforcement and the lowering's hidden schema params must never
        // disagree.
        {
            let seed: std::collections::HashMap<String, (Vec<String>, Vec<String>)> = self
                .schema_passing_generic_funcs
                .iter()
                .filter_map(|(name, schema)| {
                    self.generic_func_defs.get(name).map(|fd| {
                        (
                            name.clone(),
                            (
                                fd.type_params
                                    .iter()
                                    .map(|tp| tp.name.clone())
                                    .collect::<Vec<String>>(),
                                schema.clone(),
                            ),
                        )
                    })
                })
                .collect();
            let fd_refs: Vec<&FuncDef> = program
                .statements
                .iter()
                .filter_map(|s| match s {
                    Statement::FuncDef(fd) => Some(fd),
                    _ => None,
                })
                .collect();
            self.schema_passing_generic_funcs =
                crate::parser::close_schema_passing_type_params(&fd_refs, &seed);
        }

        // Phase-2 review M-4: alias targets resolve at registration, so a
        // forward reference (`Grid = @[Row]` before `Row = @[Int]`) left an
        // unexpanded `Named` inside the stored type. Re-resolve every alias
        // from its source TypeExpr until the table is stable — each round
        // can pick up names registered later in the file; the round count
        // is bounded by the alias count (a cycle simply stops expanding).
        let alias_defs: Vec<(String, crate::parser::TypeExpr, Span)> = program
            .statements
            .iter()
            .filter_map(|stmt| match stmt {
                Statement::ClassLikeDef(cl) => match &cl.kind {
                    crate::parser::ClassLikeKind::Alias { target } => {
                        Some((cl.name.clone(), target.clone(), cl.span.clone()))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect();
        // report alias expansion cycles with a dedicated
        // diagnostic. The fixpoint loop below simply stops expanding on a
        // cycle, which used to leave a half-expanded table whose only
        // symptom was a baffling nested-List expectation on unrelated
        // assignments (`expected @[@[@[B]]], got @[@[Int]]`).
        let alias_names: HashSet<String> =
            alias_defs.iter().map(|(name, _, _)| name.clone()).collect();
        let mut deps: HashMap<&str, Vec<String>> = HashMap::new();
        for (name, target, _) in &alias_defs {
            deps.insert(name.as_str(), collect_alias_refs(target, &alias_names));
        }
        let spans: HashMap<&str, Span> = alias_defs
            .iter()
            .map(|(n, _, s)| (n.as_str(), s.clone()))
            .collect();
        // DFS with gray/black marking: a back edge to a gray node closes a
        // cycle; the chain from that node around is reported once. Roots are
        // visited in name order so the reported chain is deterministic.
        // Missing color key means unvisited (white).
        const WHITE: u8 = 0;
        let mut color: HashMap<&str, u8> = HashMap::new();
        let mut stack: Vec<&str> = Vec::new();
        let mut roots: Vec<&str> = deps.keys().copied().collect();
        roots.sort_unstable();
        for root in roots {
            if color.get(root).copied().unwrap_or(WHITE) == WHITE {
                stack.clear();
                if let Err(chain) = dfs_alias_cycle(root, &deps, &mut color, &mut stack) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1632] Type alias expansion cycle detected: {}. \
                             Hint: break the cycle so every alias expands to a finite type.",
                            chain
                        ),
                        span: spans
                            .get(root)
                            .cloned()
                            .unwrap_or_else(|| Span::new(0, 0, 0, 0)),
                    });
                    // The DFS bailed out mid-walk, leaving its nodes GRAY.
                    // Blacken them so the next root starts from a clean
                    // marking state instead of seeing phantom back edges.
                    blacken_stale_gray_alias_nodes(&mut color);
                }
            }
        }
        for _ in 0..alias_defs.len().saturating_sub(1) {
            let mut changed = false;
            for (name, target, _) in &alias_defs {
                let resolved = self.registry.resolve_type(target);
                if self.registry.type_aliases.get(name) != Some(&resolved) {
                    self.registry.register_type_alias(name, resolved);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Register inheritances only after their mold-like parents have field metadata available.
        let mut pending_inheritances: Vec<&Statement> = program
            .statements
            .iter()
            .filter(|stmt| {
                matches!(
                    stmt,
                    Statement::ClassLikeDef(cl) if cl.is_inheritance()
                )
            })
            .collect();
        while !pending_inheritances.is_empty() {
            let mut next_round = Vec::new();
            let mut made_progress = false;
            for stmt in pending_inheritances {
                let Statement::ClassLikeDef(inh) = stmt else {
                    continue;
                };
                if !inh.is_inheritance() {
                    continue;
                }
                let inh_parent = inh.parent().expect("inheritance kind has parent");
                let parent_is_mold_like = self.mold_header_specs.contains_key(inh_parent);
                if !parent_is_mold_like || self.mold_field_defs.contains_key(inh_parent) {
                    self.register_types(stmt);
                    made_progress = true;
                } else {
                    next_round.push(stmt);
                }
            }

            if !made_progress {
                for stmt in next_round {
                    self.register_types(stmt);
                }
                break;
            }
            pending_inheritances = next_round;
        }

        // Second pass: type-check statements
        for stmt in &program.statements {
            self.check_statement(stmt);
        }

        // Third pass: check mold-specific errors (e.g., E1613) that need
        // to fire regardless of expression context. This separate pass
        // ensures errors are caught even inside builtin function args where
        // infer_expr_type may not recurse.
        for stmt in &program.statements {
            self.check_mold_errors_in_stmt(stmt);
        }

        // Build-descriptor runtime-use pass ([E1532]): reject build-driver
        // descriptors (`BuildUnit` / `BuildPlan` / `AssetBundle` /
        // `RouteAsset` / `BuildHook`) used as ordinary runtime values. The
        // descriptor build path (`taida build --unit / --plan / --all-units`)
        // parses + matches the AST directly without invoking the checker, so
        // this pass only ever runs when a descriptor module is `run` /
        // `way check`'d / single-target built — i.e. exactly the cases where a
        // descriptor is being treated as a runtime value. Allow-listed
        // positions (top-level export value, descriptor field, binding RHS)
        // are threaded through `DescriptorUseCtx`.
        self.check_descriptor_runtime_use(program);

        // C12-3 / FB-8: promote non-tail mutual recursion to a
        // compile-time error so programs that would overflow the stack at
        // runtime (`Maximum call depth exceeded`) are rejected up front.
        // Tail-only mutual recursion is left to pass — the Interpreter / JS
        // backends handle it via the mutual-TCO trampoline and the Native
        // backend treats it as a regular call (see
        // docs/reference/tail_recursion.md).
        self.check_mutual_recursion_errors(program);

        // [E1539]: top-level executable code must not reference a
        // function defined later in the file. The interpreter executes
        // statements in order (definition order is the language
        // semantics), while the compiled backends hoist definitions —
        // without this check the same program succeeds or fails
        // depending on the backend. Function/lambda bodies defer
        // resolution to call time and stay legal (mutual recursion).
        self.check_toplevel_forward_function_references(program);

        // [E1540]: the six value-absence words are reserved — reads
        // already fail as undefined variables, this closes bindings.
        self.check_reserved_absence_words(program);

        if self.typed_expr_table.has_residual_unknown() {
            let residuals = self
                .typed_expr_table
                .residual_unknown_types()
                .into_iter()
                .take(5)
                .map(|ty| ty.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            self.errors.push(TypeError {
                message: format!(
                    "[E1529] Type inference left unresolved type(s): {}. Add explicit type annotations.",
                    residuals
                ),
                span: Span::new(0, 0, 1, 1),
            });
        }

        // Drop exact duplicates (same span, same message) while keeping the
        // first occurrence's position. Overlapping validation passes can
        // legitimately reach the same node twice (e.g. the call-argument
        // walk and the expression-arm walk both seeing a misplaced `_`);
        // reporting the identical diagnostic twice adds no information.
        let mut seen = std::collections::HashSet::new();
        self.errors.retain(|err| {
            seen.insert((
                err.span.line,
                err.span.column,
                err.span.start,
                err.span.end,
                err.message.clone(),
            ))
        });

        // A heterogeneous list literal fed straight into `setOf` is a
        // legitimate value-tagged Set (the runtime keeps per-element kinds),
        // so the literal's own `[E0401]` must not reject the program.
        // Exactly the recorded argument spans are excused: direct bindings
        // (`xs <= @[1, "x"]`) keep their diagnostic, and errors from other
        // statements are untouched.
        if !self.setof_arg_spans.is_empty() {
            let excused = &self.setof_arg_spans;
            self.errors
                .retain(|err| !(err.message.contains("[E0401]") && excused.contains(&err.span)));
        }
    }

    fn type_expr_to_string(ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Named(name) => name.clone(),
            TypeExpr::BuchiPack(fields) => {
                let rendered_fields: Vec<String> = fields
                    .iter()
                    .map(|field| match &field.type_annotation {
                        Some(field_ty) => {
                            format!("{}: {}", field.name, Self::type_expr_to_string(field_ty))
                        }
                        None => field.name.clone(),
                    })
                    .collect();
                format!("@({})", rendered_fields.join(", "))
            }
            TypeExpr::List(inner) => format!("@[{}]", Self::type_expr_to_string(inner)),
            TypeExpr::Generic(name, args) => {
                let rendered_args: Vec<String> =
                    args.iter().map(Self::type_expr_to_string).collect();
                format!("{}[{}]", name, rendered_args.join(", "))
            }
            TypeExpr::Function(params, ret) => {
                let rendered_params: Vec<String> =
                    params.iter().map(Self::type_expr_to_string).collect();
                match rendered_params.as_slice() {
                    [single] => format!("{} => :{}", single, Self::type_expr_to_string(ret)),
                    _ => format!(
                        "({}) => :{}",
                        rendered_params.join(", "),
                        Self::type_expr_to_string(ret)
                    ),
                }
            }
        }
    }

    // ── B11B-016: Mold-specific error pass (third pass) ──────────────
    // Recursively walks expressions to find mold patterns that need
    // rejection regardless of expression context. Separated from
    // infer_expr_type to avoid triggering unrelated type errors (e.g.,
    // E1510 on closure return types) in builtin function arguments.

    // ── Build-descriptor runtime-use pass ([E1532]) ──────────────────
    //
    // `BuildUnit` / `BuildPlan` / `AssetBundle` / `RouteAsset` / `BuildHook`
    // are build-driver descriptors, not runtime values. They are valid only
    // in a handful of positions; everywhere else they are rejected so a
    // descriptor cannot leak into a runtime computation (where the backends
    // would treat its `__type`-tagged pack as an ordinary pack — the
    // behaviour the docs previously only discouraged in prose).
    //
    // Allow-listed positions (`DescriptorUseCtx::Allowed`):
    //   - a top-level `<<<` export value (a descriptor *is* the artefact the
    //     build driver consumes),
    //   - a field value of an enclosing descriptor (`BuildUnit.assets` holding
    //     `RouteAsset(...)`, `BuildPlan.units` holding `BuildUnit` references,
    //     etc. — the nested-descriptor shape the driver walks),
    //   - the right-hand side of a top-level binding (`name <= BuildUnit(...)`),
    //     which exists purely so the value can reach an export.
    // Every other position (`DescriptorUseCtx::Runtime`) is rejected:
    //   builtin args (`stdout(unit)`), user-function args, conversion / mold
    //   args, operator operands, field / method access, list elements outside
    //   a descriptor field, etc.

    fn push_e1605_once(&mut self, span: &Span, message: String) {
        if self
            .errors
            .iter()
            .any(|err| err.span == *span && err.message.contains("[E1605]"))
        {
            return;
        }
        self.errors.push(TypeError {
            message,
            span: span.clone(),
        });
    }

    const MAX_BIDI_TYPE_HINT_DEPTH: usize = 32;

    fn visible_binding_shadows_function(&self, name: &str) -> bool {
        let Some(function_scope_depth) = self.func_def_scope_depths.get(name).copied() else {
            return self.lookup_var(name).is_some();
        };
        for (idx, scope) in self.scope_stack.iter().enumerate().rev() {
            if scope.contains_key(name) {
                return idx != function_scope_depth;
            }
        }
        false
    }

    fn is_narrow_body_inference_expr(expr: &Expr, params: &[Param]) -> bool {
        let param_names: HashSet<&str> = params.iter().map(|param| param.name.as_str()).collect();
        Self::is_narrow_body_expr_inner(expr, &param_names)
    }

    fn is_narrow_body_expr_inner(expr: &Expr, param_names: &HashSet<&str>) -> bool {
        match expr {
            Expr::Ident(name, _) => param_names.contains(name.as_str()),
            Expr::IntLit(_, _)
            | Expr::FloatLit(_, _)
            | Expr::StringLit(_, _)
            | Expr::BoolLit(_, _) => true,
            Expr::FieldAccess(base, _, _) => Self::is_narrow_body_expr_inner(base, param_names),
            // Keep the allow list to local, side-effect-free shapes that
            // propagate types from hinted params. Branches and free calls are
            // left to annotated functions or the normal checker path; method
            // calls stay allowed only when receiver and args are narrow too.
            Expr::MethodCall(receiver, method, args, _) if Self::is_narrow_body_method(method) => {
                Self::is_narrow_body_expr_inner(receiver, param_names)
                    && args
                        .iter()
                        .all(|arg| Self::is_narrow_body_expr_inner(arg, param_names))
            }
            _ => false,
        }
    }

    fn is_narrow_body_method(method: &str) -> bool {
        matches!(
            method,
            "toString" | "length" | "isEmpty" | "hasValue" | "typename"
        )
    }
}

mod arity;
mod cage;
mod check;
mod checker_methods;
mod descriptor;
mod imports;
mod infer;
pub(crate) mod method_spec;
mod mold_header;
mod resolve;
mod validate;
mod worker;

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

// ────────────────────────────────────────────────────────────────────────
// E30 Phase 6 / E30B-004: defaultFn 生成可能性判定 API (Lock-D verdict)
// ────────────────────────────────────────────────────────────────────────
//
// `default_fn_generatable` returns whether a synthetic default function
// (defaultFn) can be generated for the given `TypeExpr`.
//
// Lock-D verdict (E30 Phase 0, 2026-04-28):
//   - primitive types (Int, Num, Float, Str, Bytes, Bool, Unit, JSON, Molten): true
//   - List[T] / Lax[T] / Async[T]: true iff inner T is generatable
//   - BuchiPack inline: true iff all fields are generatable
//   - Named type: true iff registered in TypeRegistry (TypeDef / Mold /
//     Error / Enum). Recursive cycles are allowed via `visiting` cycle
//     guard. Unknown alias (opaque type) → false.
//   - Function type: true iff return type is generatable (recursive)
//
// Lock-C verdict (E30 Phase 0, 2026-04-28): Phase 5 will fire `[E1410]`
// when this API returns false for a declare-only function field's type
// annotation.
//
// `visiting` is the cycle guard used by `default_for_type_expr` (interpreter)
// and `lower_default_for_type_expr` (codegen) so that the judgement remains
// consistent with actual default-value materialisation.

/// Returns true iff a defaultFn can be synthesised for the given function /
/// value type per verdict.
///
/// `visiting` carries the names already in the recursion stack so that
/// self-referential / mutually-recursive types are treated as generatable
/// (the existing class-like `default_for_type_expr` cycle guard returns a
/// minimal `__type` pack at the cycle point — we mirror that semantics).
pub fn default_fn_generatable(
    type_expr: &TypeExpr,
    registry: &TypeRegistry,
    visiting: &mut HashSet<String>,
) -> bool {
    match type_expr {
        TypeExpr::Named(name) => match name.as_str() {
            // Built-in primitives — Lock-D "primitive types: true".
            "Int" | "Num" | "Float" | "Str" | "Bytes" | "Bool" | "Unit" | "JSON" | "Molten" => true,
            // T (single uppercase) — type parameters that may or may not be
            // bound at the use site. Treat as generatable (the eventual
            // binding determines the concrete default; cycle guard handles
            // the recursive case).
            _ if name.len() == 1
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_uppercase())
                    .unwrap_or(false) =>
            {
                true
            }
            _ => {
                if visiting.contains(name) {
                    // Cycle: mirror interpreter's `default_for_type_expr`
                    // which returns a minimal `__type` pack at the cycle
                    // point. That counts as a valid default.
                    return true;
                }
                // Registered class-like types (TypeDef / Mold / Error /
                // Enum) all have well-defined defaults.
                if registry.type_defs.contains_key(name)
                    || registry.mold_defs.contains_key(name)
                    || registry.error_types.contains_key(name)
                    || registry.enum_defs.contains_key(name)
                {
                    return true;
                }
                // Unknown / opaque alias — defaultFn cannot be generated.
                false
            }
        },
        TypeExpr::List(inner) => {
            // List default is empty list; we still recurse so that the
            // inner type is generatable for downstream introspection.
            default_fn_generatable(inner, registry, visiting)
        }
        TypeExpr::Generic(name, args) => match name.as_str() {
            "Lax" | "Async" => args
                .first()
                .map(|inner| default_fn_generatable(inner, registry, visiting))
                .unwrap_or(true),
            // Other generic bases are intentionally not accepted here yet:
            // interpreter / JS / native default materializers only share
            // concrete support for Lax and Async. Accepting arbitrary
            // registered generics would let the checker approve a defaultFn
            // whose return value diverges across backends.
            _ => false,
        },
        TypeExpr::BuchiPack(fields) => fields.iter().filter(|f| !f.is_method).all(|f| {
            f.type_annotation
                .as_ref()
                .map(|ty| default_fn_generatable(ty, registry, visiting))
                .unwrap_or(true) // missing annotation defaults to Unit
        }),
        TypeExpr::Function(_, ret) => {
            // defaultFn is generatable iff the return type's default value
            // can be constructed. Argument types do not affect generability.
            default_fn_generatable(ret, registry, visiting)
        }
    }
}

/// alias names referenced anywhere inside a target type
/// expression (nested lists, generics, function types included).
fn collect_alias_refs(ty: &crate::parser::TypeExpr, alias_names: &HashSet<String>) -> Vec<String> {
    use crate::parser::TypeExpr;
    let mut refs: Vec<String> = Vec::new();
    match ty {
        TypeExpr::Named(name) => {
            if alias_names.contains(name) {
                refs.push(name.clone());
            }
        }
        TypeExpr::List(inner) => refs.extend(collect_alias_refs(inner, alias_names)),
        TypeExpr::Generic(_, args) => {
            for arg in args {
                refs.extend(collect_alias_refs(arg, alias_names));
            }
        }
        TypeExpr::Function(params, ret) => {
            for p in params {
                refs.extend(collect_alias_refs(p, alias_names));
            }
            refs.extend(collect_alias_refs(ret, alias_names));
        }
        TypeExpr::BuchiPack(fields) => {
            for field in fields {
                if let Some(annotation) = &field.type_annotation {
                    refs.extend(collect_alias_refs(annotation, alias_names));
                }
            }
        }
    }
    refs
}

/// DFS returning Err(cycle chain) when a back edge closes a
/// cycle. Gray nodes are on the current path; black nodes are done.
fn dfs_alias_cycle<'a>(
    node: &'a str,
    deps: &'a HashMap<&'a str, Vec<String>>,
    color: &mut HashMap<&'a str, u8>,
    stack: &mut Vec<&'a str>,
) -> Result<(), String> {
    const WHITE: u8 = 0;
    const GRAY: u8 = 1;
    const BLACK: u8 = 2;
    color.insert(node, GRAY);
    stack.push(node);
    for next in deps.get(node).into_iter().flatten() {
        match color.get(next.as_str()).copied().unwrap_or(WHITE) {
            GRAY => {
                // Cycle: from `next` (first occurrence on the stack) around.
                let start = stack.iter().position(|n| *n == next.as_str()).unwrap_or(0);
                let mut chain: Vec<String> = stack[start..].iter().map(|n| n.to_string()).collect();
                chain.push(next.clone());
                return Err(chain.join(" -> "));
            }
            BLACK => {}
            _ => dfs_alias_cycle(next, deps, color, stack)?,
        }
    }
    stack.pop();
    color.insert(node, BLACK);
    Ok(())
}

/// F64 review-fix: an early `Err` return from `dfs_alias_cycle` leaves the
/// nodes it walked marked GRAY. A later root must not mistake one of those
/// stale grays for a back edge (`D=@[A] A=@[B] B=@[C] C=@[B]` reported a
/// phantom `D -> A` cycle after the real `B -> C -> B`). After each cycle
/// report, promote every remaining GRAY node to BLACK — the cycle they sit
/// on has already been reported once.
fn blacken_stale_gray_alias_nodes(color: &mut HashMap<&str, u8>) {
    const GRAY: u8 = 1;
    const BLACK: u8 = 2;
    let gray_keys: Vec<&str> = color
        .iter()
        .filter(|(_, c)| **c == GRAY)
        .map(|(n, _)| *n)
        .collect();
    for key in gray_keys {
        color.insert(key, BLACK);
    }
}

#[cfg(test)]
mod tests;

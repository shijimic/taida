use super::types::{Type, TypeRegistry};
use crate::lexer::Span;
use crate::net_surface::NET_HTTP_PROTOCOL_VARIANTS;
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
    /// While `infer_expr_type` descends into a `FuncCall` that is itself
    /// a pipeline stage (`data => f(...)`), this holds the *previous
    /// stage's* result type — the value the runtime injects as the
    /// implicit first argument when the stage call carries no
    /// placeholder. Consumed (taken) by the FuncCall arm so calls nested
    /// inside the stage's arguments do not inherit it; arity *and* type
    /// validation must count / check that injected argument. `None`
    /// outside pipeline stages.
    pipeline_stage_injected_type: Option<Type>,
    /// Typed HIR / expression type table. `infer_expr_type` records
    /// every observed `Expr` here so codegen lowering can answer
    /// "is this expression Bool?" by looking up the recorded type.
    pub typed_expr_table: super::typed_hir::TypedExprTable,
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
            pipeline_stage_injected_type: None,
            typed_expr_table: super::typed_hir::TypedExprTable::new(),
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

    /// install pinned signatures for the interactive os
    /// variants. Idempotent — `register_os_import_symbol` delegates here
    /// for the same symbol names, so the import path remains a no-op
    /// overwrite with the identical `Gorillax[@(code: Int)]` shape.
    ///
    /// Captured `run` / `execShell` are intentionally left out: pinning
    /// them would change the non-interfering contract documented in
    /// `register_os_import_symbol` and tightening on the core-bundled
    /// path would silently affect every existing program that never
    /// imports `taida-lang/os`.
    fn install_core_bundled_os_pins(&mut self) {
        self.pin_run_interactive_signature("runInteractive");
        self.pin_exec_shell_interactive_signature("execShellInteractive");
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

    fn pin_run_interactive_signature(&mut self, local_name: &str) {
        // runInteractive(program: Str, args: @[Str]) → Gorillax[@(code: Int)]
        let inner = Type::BuchiPack(vec![("code".to_string(), Type::Int)]);
        let ret = Type::Generic("Gorillax".to_string(), vec![inner]);
        self.func_types.insert(local_name.to_string(), ret);
        self.func_param_counts.insert(local_name.to_string(), 2);
        self.func_param_types.insert(
            local_name.to_string(),
            vec![Type::Str, Type::List(Box::new(Type::Str))],
        );
    }

    fn pin_exec_shell_interactive_signature(&mut self, local_name: &str) {
        // execShellInteractive(command: Str) → Gorillax[@(code: Int)]
        let inner = Type::BuchiPack(vec![("code".to_string(), Type::Int)]);
        let ret = Type::Generic("Gorillax".to_string(), vec![inner]);
        self.func_types.insert(local_name.to_string(), ret);
        self.func_param_counts.insert(local_name.to_string(), 1);
        self.func_param_types
            .insert(local_name.to_string(), vec![Type::Str]);
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

    fn register_net_import_symbol(&mut self, symbol_name: &str, local_name: &str) {
        match symbol_name {
            "httpServe" => {
                self.net_http_serve_symbols.insert(local_name.to_string());
            }
            "HttpProtocol" => {
                self.registry.register_enum(
                    local_name,
                    NET_HTTP_PROTOCOL_VARIANTS
                        .iter()
                        .map(|variant| (*variant).to_string())
                        .collect(),
                );
                self.declared_header_arities
                    .insert(local_name.to_string(), 0);
                self.net_http_protocol_type_names
                    .insert(local_name.to_string());
            }
            _ => {}
        }
    }

    /// register typed signatures for `taida-lang/os` symbols that
    /// need compile-time Gorillax inner-shape pinning.
    ///
    /// Currently only the interactive variants are pinned, because
    /// their inner shape `@(code: Int)` is strictly narrower than the
    /// captured `run` / `execShell` form `@(stdout, stderr, code)` — and
    /// callers who reach for `.__value.stdout` on an interactive result
    /// must get a compile error rather than silent Unknown.
    ///
    /// The captured variants are intentionally left Unknown so we stay
    /// non-interfering with pre-existing callers (`run(...).__value.stdout`
    /// etc. must keep working). If/when we want to pin those too, add
    /// matches for "run" / "execShell" below.
    fn register_os_import_symbol(&mut self, symbol_name: &str, local_name: &str) {
        match symbol_name {
            "runInteractive" => {
                // Delegates to the same helper used by the import-less path
                // (`install_core_bundled_os_pins`), so the pinned shape is
                // identical whether or not the user wrote
                // `>>> taida-lang/os => @(runInteractive)`. When the import
                // uses an alias (`runInteractive as foo`), this path also
                // installs the alias under the same pin.
                self.pin_run_interactive_signature(local_name);
            }
            "execShellInteractive" => {
                self.pin_exec_shell_interactive_signature(local_name);
            }
            _ => {
                // Other os symbols stay unregistered so the checker treats
                // them as Type::Unknown (pre-C19 behaviour, non-interfering).
            }
        }
    }

    fn abi_request_fields() -> Vec<(String, Type)> {
        let pair_list = Self::abi_name_value_pair_list_type();
        vec![
            ("method".to_string(), Type::Str),
            ("path".to_string(), Type::Str),
            ("rawQuery".to_string(), Type::Str),
            ("query".to_string(), pair_list.clone()),
            ("headers".to_string(), pair_list),
            ("body".to_string(), Type::Bytes),
        ]
    }

    fn abi_response_fields() -> Vec<(String, Type)> {
        let pair_list = Self::abi_name_value_pair_list_type();
        vec![
            ("status".to_string(), Type::Int),
            ("headers".to_string(), pair_list),
            ("body".to_string(), Type::Bytes),
        ]
    }

    fn register_abi_type_symbol(&mut self, symbol_name: &str, local_name: &str) {
        match symbol_name {
            "WebRequest" => {
                self.registry
                    .register_type(local_name, Self::abi_request_fields());
                self.declared_concrete_type_names
                    .insert(local_name.to_string());
                self.declared_header_arities
                    .insert(local_name.to_string(), 0);
            }
            "WebResponse" => {
                self.registry
                    .register_type(local_name, Self::abi_response_fields());
                self.declared_concrete_type_names
                    .insert(local_name.to_string());
                self.declared_header_arities
                    .insert(local_name.to_string(), 0);
            }
            _ => {}
        }
    }

    fn register_abi_imports(&mut self, symbols: &[crate::parser::ImportSymbol]) {
        let request_name = symbols
            .iter()
            .find(|sym| sym.name == "WebRequest")
            .map(|sym| sym.alias.as_deref().unwrap_or(sym.name.as_str()))
            .unwrap_or("WebRequest");
        let response_name = symbols
            .iter()
            .find(|sym| sym.name == "WebResponse")
            .map(|sym| sym.alias.as_deref().unwrap_or(sym.name.as_str()))
            .unwrap_or("WebResponse");

        for sym in symbols {
            let local_name = sym.alias.as_deref().unwrap_or(sym.name.as_str());
            self.register_abi_type_symbol(&sym.name, local_name);
        }

        let response_ty = Type::Named(response_name.to_string());
        for sym in symbols {
            let local_name = sym.alias.as_deref().unwrap_or(sym.name.as_str());
            match sym.name.as_str() {
                "text" => {
                    self.func_types
                        .insert(local_name.to_string(), response_ty.clone());
                    self.func_param_counts.insert(local_name.to_string(), 1);
                    self.func_param_types
                        .insert(local_name.to_string(), vec![Type::Str]);
                }
                "json" => {
                    self.func_types
                        .insert(local_name.to_string(), response_ty.clone());
                    self.func_param_counts.insert(local_name.to_string(), 1);
                    self.func_param_types
                        .insert(local_name.to_string(), vec![Type::Unknown]);
                }
                "bytes" => {
                    self.func_types
                        .insert(local_name.to_string(), response_ty.clone());
                    self.func_param_counts.insert(local_name.to_string(), 1);
                    self.func_param_types
                        .insert(local_name.to_string(), vec![Type::Bytes]);
                }
                "status" => {
                    self.func_types
                        .insert(local_name.to_string(), response_ty.clone());
                    self.func_param_counts.insert(local_name.to_string(), 2);
                    self.func_param_types
                        .insert(local_name.to_string(), vec![Type::Int, response_ty.clone()]);
                }
                "header" => {
                    self.func_types
                        .insert(local_name.to_string(), response_ty.clone());
                    self.func_param_counts.insert(local_name.to_string(), 3);
                    self.func_param_types.insert(
                        local_name.to_string(),
                        vec![Type::Str, Type::Str, response_ty.clone()],
                    );
                }
                "WebRequest" | "WebResponse" => {
                    let _ = request_name;
                }
                _ => {}
            }
        }
    }

    fn binding_diag(code: &str, message: String, hint: &str) -> String {
        format!("[{}] {} Hint: {}", code, message, hint)
    }

    fn type_expr_mentions_type_param(ty: &TypeExpr, name: &str) -> bool {
        match ty {
            TypeExpr::Named(type_name) => type_name == name,
            TypeExpr::BuchiPack(fields) => fields.iter().any(|field| {
                field
                    .type_annotation
                    .as_ref()
                    .is_some_and(|field_ty| Self::type_expr_mentions_type_param(field_ty, name))
            }),
            TypeExpr::List(inner) => Self::type_expr_mentions_type_param(inner, name),
            TypeExpr::Generic(type_name, args) => {
                type_name == name
                    || args
                        .iter()
                        .any(|arg| Self::type_expr_mentions_type_param(arg, name))
            }
            TypeExpr::Function(params, ret) => {
                params
                    .iter()
                    .any(|param| Self::type_expr_mentions_type_param(param, name))
                    || Self::type_expr_mentions_type_param(ret, name)
            }
        }
    }

    fn type_param_name_is_reserved(&self, name: &str) -> bool {
        self.declared_concrete_type_names.contains(name)
            || self.registry.type_defs.contains_key(name)
            || self.registry.enum_defs.contains_key(name)
            || self.registry.mold_defs.contains_key(name)
            || !matches!(
                self.registry.resolve_type(&TypeExpr::Named(name.to_string())),
                Type::Named(ref resolved) if resolved == name
            )
    }

    fn effective_mold_header_args(md: &ClassLikeDef) -> Vec<MoldHeaderArg> {
        // (E30 Sub-step 2.1) Mold kind の ClassLikeDef のみ呼び出される想定。
        let mold_args = md.mold_args().cloned().unwrap_or_default();
        md.name_args.as_ref().cloned().unwrap_or(mold_args)
    }

    fn merge_field_defs(parent: &[FieldDef], child: &[FieldDef]) -> Vec<FieldDef> {
        let mut merged = parent.to_vec();
        for child_field in child {
            if let Some(existing) = merged
                .iter_mut()
                .find(|field| field.name == child_field.name)
            {
                *existing = child_field.clone();
            } else {
                merged.push(child_field.clone());
            }
        }
        merged
    }

    fn header_arg_label(arg: &MoldHeaderArg) -> String {
        match arg {
            MoldHeaderArg::TypeParam(tp) => match &tp.constraint {
                Some(constraint) => {
                    format!("{} <= :{}", tp.name, Self::type_expr_to_string(constraint))
                }
                None => tp.name.clone(),
            },
            MoldHeaderArg::Concrete(ty) => format!(":{}", Self::type_expr_to_string(ty)),
        }
    }

    fn collect_mold_type_param_names(args: &[MoldHeaderArg]) -> Vec<String> {
        args.iter()
            .filter_map(|arg| match arg {
                MoldHeaderArg::TypeParam(tp) => Some(tp.name.clone()),
                MoldHeaderArg::Concrete(_) => None,
            })
            .collect()
    }

    fn inheritance_uses_headers(inh: &ClassLikeDef) -> bool {
        // (E30 Sub-step 2.1) Inheritance kind の ClassLikeDef のみ呼び出される想定。
        inh.parent_args().is_some() || inh.name_args.is_some()
    }

    fn predeclare_header_metadata(&mut self, statements: &[Statement]) {
        // (E30 Sub-step 2.1) ClassLikeDef + kind discriminator dispatch
        self.mold_header_specs.clear();
        self.declared_header_arities.clear();

        for stmt in statements {
            if let Statement::ClassLikeDef(cl) = stmt {
                match &cl.kind {
                    ClassLikeKind::BuchiPack => {
                        self.declared_header_arities.insert(cl.name.clone(), 0);
                    }
                    ClassLikeKind::Mold { .. } => {
                        let header_args = Self::effective_mold_header_args(cl);
                        self.mold_header_specs.insert(
                            cl.name.clone(),
                            MoldHeaderSpec {
                                header_args: header_args.clone(),
                            },
                        );
                        self.declared_header_arities
                            .insert(cl.name.clone(), header_args.len());
                    }
                    ClassLikeKind::Inheritance { .. } => {}
                }
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            for stmt in statements {
                let Statement::ClassLikeDef(inh) = stmt else {
                    continue;
                };
                if !inh.is_inheritance() {
                    continue;
                }
                let inh_parent = inh.parent().expect("inheritance kind has parent");
                let inh_child = &inh.name;

                let parent_header = self
                    .mold_header_specs
                    .get(inh_parent)
                    .map(|spec| spec.header_args.clone());
                let parent_arity = parent_header
                    .as_ref()
                    .map(Vec::len)
                    .or_else(|| self.declared_header_arities.get(inh_parent).copied());

                if let Some(parent_header) = parent_header {
                    let child_header = inh
                        .name_args
                        .clone()
                        .or_else(|| inh.parent_args().cloned())
                        .unwrap_or_else(|| parent_header.clone());
                    if self
                        .mold_header_specs
                        .get(inh_child)
                        .map(|spec| spec.header_args.as_slice())
                        != Some(child_header.as_slice())
                    {
                        self.mold_header_specs.insert(
                            inh_child.clone(),
                            MoldHeaderSpec {
                                header_args: child_header.clone(),
                            },
                        );
                        changed = true;
                    }

                    let child_arity = child_header.len();
                    if self.declared_header_arities.get(inh_child) != Some(&child_arity) {
                        self.declared_header_arities
                            .insert(inh_child.clone(), child_arity);
                        changed = true;
                    }
                } else if !Self::inheritance_uses_headers(inh)
                    && let Some(parent_arity) = parent_arity
                    && self.declared_header_arities.get(inh_child) != Some(&parent_arity)
                {
                    self.declared_header_arities
                        .insert(inh_child.clone(), parent_arity);
                    changed = true;
                }
            }
        }
    }

    fn find_forbidden_default_ref(expr: &Expr, forbidden: &HashSet<String>) -> Option<String> {
        match expr {
            Expr::Ident(name, _) => {
                if forbidden.contains(name) {
                    Some(name.clone())
                } else {
                    None
                }
            }
            Expr::IntLit(_, _)
            | Expr::FloatLit(_, _)
            | Expr::StringLit(_, _)
            | Expr::TemplateLit(_, _)
            | Expr::BoolLit(_, _)
            | Expr::Gorilla(_)
            | Expr::Placeholder(_)
            | Expr::EnumVariant(_, _, _)
            | Expr::TypeLiteral(_, _, _)
            | Expr::Hole(_) => None,
            Expr::BuchiPack(fields, _) => fields
                .iter()
                .find_map(|field| Self::find_forbidden_default_ref(&field.value, forbidden)),
            Expr::ListLit(items, _) => items
                .iter()
                .find_map(|item| Self::find_forbidden_default_ref(item, forbidden)),
            Expr::BinaryOp(left, _, right, _) => Self::find_forbidden_default_ref(left, forbidden)
                .or_else(|| Self::find_forbidden_default_ref(right, forbidden)),
            Expr::UnaryOp(_, inner, _) => Self::find_forbidden_default_ref(inner, forbidden),
            Expr::FuncCall(callee, args, _) => Self::find_forbidden_default_ref(callee, forbidden)
                .or_else(|| {
                    args.iter()
                        .find_map(|arg| Self::find_forbidden_default_ref(arg, forbidden))
                }),
            Expr::MethodCall(obj, _, args, _) => Self::find_forbidden_default_ref(obj, forbidden)
                .or_else(|| {
                    args.iter()
                        .find_map(|arg| Self::find_forbidden_default_ref(arg, forbidden))
                }),
            Expr::FieldAccess(obj, _, _) => Self::find_forbidden_default_ref(obj, forbidden),
            Expr::CondBranch(arms, _) => arms.iter().find_map(|arm| {
                arm.condition
                    .as_ref()
                    .and_then(|cond| Self::find_forbidden_default_ref(cond, forbidden))
                    .or_else(|| {
                        arm.body.iter().find_map(|stmt| {
                            if let Statement::Expr(e) = stmt {
                                Self::find_forbidden_default_ref(e, forbidden)
                            } else {
                                None
                            }
                        })
                    })
            }),
            Expr::Pipeline(exprs, _) => exprs
                .iter()
                .find_map(|node| Self::find_forbidden_default_ref(node, forbidden)),
            Expr::MoldInst(_, type_args, fields, _) => type_args
                .iter()
                .find_map(|arg| Self::find_forbidden_default_ref(arg, forbidden))
                .or_else(|| {
                    fields
                        .iter()
                        .find_map(|field| Self::find_forbidden_default_ref(&field.value, forbidden))
                }),
            Expr::Unmold(inner, _) => Self::find_forbidden_default_ref(inner, forbidden),
            Expr::Lambda(params, body, _) => {
                let mut nested_forbidden = forbidden.clone();
                for param in params {
                    nested_forbidden.remove(&param.name);
                }
                Self::find_forbidden_default_ref(body, &nested_forbidden)
            }
            Expr::TypeInst(_, fields, _) => fields
                .iter()
                .find_map(|field| Self::find_forbidden_default_ref(&field.value, forbidden)),
            Expr::Throw(inner, _) => Self::find_forbidden_default_ref(inner, forbidden),
        }
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

    /// returns true when `name` is an active generic type parameter
    /// whose declared subtype constraint is a numeric primitive (`Num` / `Int`
    /// `Float`). Such a type variable is treated as numeric for arithmetic
    /// (`+` / `-` / `*`) and ordering operators inside the function body.
    fn type_param_is_numeric(&self, name: &str) -> bool {
        let Some(tp) = self.lookup_active_type_param(name) else {
            return false;
        };
        matches!(
            tp.constraint.as_ref(),
            Some(TypeExpr::Named(n)) if n == "Num" || n == "Int" || n == "Float"
        )
    }

    /// if `name` is an active generic type parameter whose
    /// declared subtype constraint is a function type (e.g. `F <=:T =>:T`),
    /// return the resolved `Type::Function(...)` for that constraint.
    /// Returns `None` for non-function constraints (or unconstrained vars).
    fn type_param_function_constraint(&self, name: &str) -> Option<Type> {
        let tp = self.lookup_active_type_param(name)?;
        let constraint = tp.constraint.as_ref()?;
        if matches!(constraint, TypeExpr::Function(_, _)) {
            Some(self.registry.resolve_type(constraint))
        } else {
            None
        }
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
    pub(super) fn is_builtin_type_name(name: &str) -> bool {
        matches!(
            name,
            "Int"
                | "Float"
                | "Num"
                | "Number"
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

    fn branch_from_type_arg(&self, expr: &Expr) -> Option<CageBranch> {
        match expr {
            Expr::Ident(name, _) | Expr::TypeLiteral(name, None, _) => CageBranch::from_name(name),
            _ => None,
        }
    }

    fn is_js_rilla_constructor(name: &str) -> bool {
        matches!(
            name,
            "JSGet" | "JSCall" | "JSCallAsync" | "JSNew" | "JSSet" | "JSBind" | "JSSpread"
        )
    }

    fn is_cage_runner_constructor(name: &str) -> bool {
        Self::is_js_rilla_constructor(name) || name == "HostCall"
    }

    fn js_rilla_constructor_signature(name: &str) -> Option<(usize, &'static str)> {
        match name {
            "JSGet" => Some((2, "JSGet[path, Out]()")),
            "JSCall" => Some((3, "JSCall[path, args, Out]()")),
            "JSCallAsync" => Some((3, "JSCallAsync[path, args, Out]()")),
            "JSNew" => Some((3, "JSNew[path, args, Out]()")),
            "JSSet" => Some((2, "JSSet[path, value]()")),
            "JSBind" => Some((1, "JSBind[path]()")),
            "JSSpread" => Some((1, "JSSpread[source]()")),
            "HostCall" => Some((2, "HostCall[steps, Out]()")),
            _ => None,
        }
    }

    fn is_cage_rilla_child(name: &str) -> bool {
        matches!(name, "JSRilla" | "FileRilla" | "BuildRilla")
    }

    fn is_hammer_cage_boundary_expr(expr: &Expr) -> bool {
        matches!(expr, Expr::MoldInst(name, _, _, _) if name == "JSON" || name == "JSONRilla")
    }

    fn molten_branch_for_expr(&self, expr: &Expr) -> Option<CageBranch> {
        match expr {
            Expr::Ident(name, _) => self.lookup_molten_branch(name),
            Expr::Unmold(inner, _) => self.gorillax_value_branch_for_expr(inner),
            _ => None,
        }
    }

    fn gorillax_value_branch_for_expr(&self, expr: &Expr) -> Option<CageBranch> {
        match expr {
            Expr::Ident(name, _) => self.lookup_gorillax_value_branch(name),
            Expr::MoldInst(name, type_args, _, _) if name == "Cage" => type_args
                .get(1)
                .and_then(|runner| self.cage_runner_type(runner))
                .and_then(|runner| {
                    if runner.output == Type::Molten {
                        Some(runner.branch)
                    } else {
                        None
                    }
                }),
            _ => None,
        }
    }

    fn branch_info_for_assignment_expr(&self, expr: &Expr, inferred: &Type) -> BranchInfo {
        match inferred {
            Type::Molten => self
                .molten_branch_for_expr(expr)
                .map(BranchInfo::Molten)
                .unwrap_or(BranchInfo::None),
            Type::Generic(name, args)
                if name == "Gorillax" && args.first().is_some_and(|arg| *arg == Type::Molten) =>
            {
                self.gorillax_value_branch_for_expr(expr)
                    .map(BranchInfo::GorillaxValue)
                    .unwrap_or(BranchInfo::None)
            }
            _ => BranchInfo::None,
        }
    }

    fn push_cage_error(&mut self, code: &str, span: &Span, message: String) {
        if self
            .errors
            .iter()
            .any(|err| err.span == *span && err.message.starts_with(code))
        {
            return;
        }
        self.errors.push(TypeError {
            message,
            span: span.clone(),
        });
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

    fn register_worker_addon_imports(&mut self, imp: &crate::parser::ImportStmt) {
        if imp.path.starts_with("npm:")
            || imp.path.starts_with("taida-lang/")
            || imp.path.starts_with("./")
            || imp.path.starts_with("../")
            || imp.path.starts_with('/')
        {
            return;
        }

        let Some(source_file) = self.source_file.clone() else {
            return;
        };
        let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
        let project_root = Self::find_project_root(source_dir);
        let resolution = if let Some(ref version) = imp.version {
            crate::pkg::resolver::resolve_package_module_versioned(
                &project_root,
                &imp.path,
                version,
            )
        } else {
            crate::pkg::resolver::resolve_package_module(&project_root, &imp.path)
        };
        let Some(resolution) = resolution else {
            return;
        };
        if resolution.submodule.is_some() {
            return;
        }
        let manifest_path = resolution.pkg_dir.join("native").join("addon.toml");
        if !manifest_path.exists() {
            return;
        }

        let manifest = match crate::addon::manifest::parse_addon_manifest(&manifest_path) {
            Ok(manifest) => manifest,
            Err(err) => {
                for sym in &imp.symbols {
                    let local = sym.alias.as_ref().unwrap_or(&sym.name);
                    self.worker_addon_bindings.insert(
                        local.to_string(),
                        WorkerAddonBinding {
                            package_id: imp.path.clone(),
                            function_name: sym.name.clone(),
                            decision: WorkerAddonDecision::Deny {
                                code: "[E1631]",
                                reason: err.to_string(),
                                active_policy: "unresolved".to_string(),
                                effective_claim: "invalid".to_string(),
                            },
                        },
                    );
                }
                return;
            }
        };

        let policy = crate::pkg::addon_purity_policy::load_addon_purity_policy(&project_root);

        for sym in &imp.symbols {
            let local = sym.alias.as_ref().unwrap_or(&sym.name);
            let decision = match &policy {
                Ok(policy) => self.decide_worker_addon_import(policy, &manifest, &sym.name),
                Err(err) => WorkerAddonDecision::Deny {
                    code: "[E1630]",
                    reason: err.clone(),
                    active_policy: "invalid".to_string(),
                    effective_claim: "unresolved".to_string(),
                },
            };
            self.worker_addon_bindings.insert(
                local.to_string(),
                WorkerAddonBinding {
                    package_id: manifest.package.clone(),
                    function_name: sym.name.clone(),
                    decision,
                },
            );
        }
    }

    fn decide_worker_addon_import(
        &self,
        policy: &crate::pkg::addon_purity_policy::AddonPurityPolicy,
        manifest: &crate::addon::manifest::AddonManifest,
        function_name: &str,
    ) -> WorkerAddonDecision {
        let active_policy = policy.mode.as_str().to_string();
        if !manifest.functions.contains_key(function_name) {
            return WorkerAddonDecision::Deny {
                code: "[E1631]",
                reason: format!(
                    "addon manifest for '{}' does not declare function '{}'",
                    manifest.package, function_name
                ),
                active_policy,
                effective_claim: "invalid".to_string(),
            };
        }
        if policy.is_override_trusted(&manifest.package, function_name) {
            return WorkerAddonDecision::Allow;
        }

        let purity = manifest.function_purity_for(function_name);
        match purity.claim {
            crate::addon::manifest::AddonPurityClaim::Unspecified => WorkerAddonDecision::Deny {
                code: "[E1627]",
                reason: "function has no `declared` purity claim".to_string(),
                active_policy,
                effective_claim: "unspecified".to_string(),
            },
            crate::addon::manifest::AddonPurityClaim::Declared => {
                if purity.audit.is_some() {
                    return WorkerAddonDecision::Deny {
                        code: "[E1629]",
                        reason: "audit metadata is present but no F48 audit verifier is available"
                            .to_string(),
                        active_policy,
                        effective_claim: "invalid".to_string(),
                    };
                }
                if policy.allows_declared() {
                    WorkerAddonDecision::Allow
                } else {
                    WorkerAddonDecision::Deny {
                        code: "[E1628]",
                        reason: "`declared` purity is below the active policy".to_string(),
                        active_policy,
                        effective_claim: "declared".to_string(),
                    }
                }
            }
        }
    }

    fn register_imported_function_signature(
        &mut self,
        fd: &crate::parser::FuncDef,
        local_name: &str,
        type_aliases: &std::collections::HashMap<&str, &str>,
    ) {
        let ret_ty = fd
            .return_type
            .as_ref()
            .map(|ty| self.resolve_imported_type_expr(ty, type_aliases))
            .unwrap_or(Type::Unknown);
        let param_types: Vec<Type> = fd
            .params
            .iter()
            .map(|param| {
                param
                    .type_annotation
                    .as_ref()
                    .map(|ty| self.resolve_imported_type_expr(ty, type_aliases))
                    .unwrap_or(Type::Unknown)
            })
            .collect();

        self.func_types.insert(local_name.to_string(), ret_ty);
        self.func_param_counts
            .insert(local_name.to_string(), fd.params.len());
        self.func_param_types
            .insert(local_name.to_string(), param_types);

        if !fd.type_params.is_empty() {
            let aliased = Self::alias_imported_func_def(fd, local_name, type_aliases);
            self.generic_func_defs
                .insert(local_name.to_string(), aliased);
        }
    }

    fn alias_imported_func_def(
        fd: &crate::parser::FuncDef,
        local_name: &str,
        type_aliases: &std::collections::HashMap<&str, &str>,
    ) -> crate::parser::FuncDef {
        let mut aliased = fd.clone();
        aliased.name = local_name.to_string();
        for type_param in &mut aliased.type_params {
            if let Some(constraint) = &type_param.constraint {
                type_param.constraint =
                    Some(Self::alias_imported_type_expr(constraint, type_aliases));
            }
        }
        for param in &mut aliased.params {
            if let Some(type_annotation) = &param.type_annotation {
                param.type_annotation = Some(Self::alias_imported_type_expr(
                    type_annotation,
                    type_aliases,
                ));
            }
        }
        if let Some(return_type) = &aliased.return_type {
            aliased.return_type = Some(Self::alias_imported_type_expr(return_type, type_aliases));
        }
        aliased
    }

    fn alias_imported_type_expr(
        ty: &crate::parser::TypeExpr,
        type_aliases: &std::collections::HashMap<&str, &str>,
    ) -> crate::parser::TypeExpr {
        use crate::parser::TypeExpr;

        match ty {
            TypeExpr::Named(name) => TypeExpr::Named(
                type_aliases
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(name.as_str())
                    .to_string(),
            ),
            TypeExpr::BuchiPack(fields) => TypeExpr::BuchiPack(
                fields
                    .iter()
                    .map(|field| {
                        let mut field = field.clone();
                        if let Some(type_annotation) = &field.type_annotation {
                            field.type_annotation = Some(Self::alias_imported_type_expr(
                                type_annotation,
                                type_aliases,
                            ));
                        }
                        field
                    })
                    .collect(),
            ),
            TypeExpr::List(inner) => TypeExpr::List(Box::new(Self::alias_imported_type_expr(
                inner,
                type_aliases,
            ))),
            TypeExpr::Generic(name, args) => TypeExpr::Generic(
                type_aliases
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(name.as_str())
                    .to_string(),
                args.iter()
                    .map(|arg| Self::alias_imported_type_expr(arg, type_aliases))
                    .collect(),
            ),
            TypeExpr::Function(params, ret) => TypeExpr::Function(
                params
                    .iter()
                    .map(|param| Self::alias_imported_type_expr(param, type_aliases))
                    .collect(),
                Box::new(Self::alias_imported_type_expr(ret, type_aliases)),
            ),
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

    /// Check an entire program. Collects type definitions first,
    /// then checks all statements.
    pub fn check_program(&mut self, program: &Program) {
        self.seen_func_defs.clear();
        self.func_def_scope_depths.clear();
        self.declared_concrete_type_names.clear();
        self.worker_effect_symbols.clear();
        self.worker_addon_symbols.clear();
        self.worker_addon_bindings.clear();
        for stmt in &program.statements {
            match stmt {
                Statement::EnumDef(ed) => {
                    self.declared_concrete_type_names.insert(ed.name.clone());
                }
                // (E30 Sub-step 2.1) ClassLikeDef 単一 variant + kind dispatch (旧 TypeDef/MoldDef/InheritanceDef を統合)
                Statement::ClassLikeDef(cl) => {
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
    }

    /// Run the `mutual-recursion` verify check and surface any findings as
    /// [`TypeError`]s attached to the checker. See
    /// `src/graph/verify.rs::check_mutual_recursion` for the detection
    /// semantics.
    fn check_mutual_recursion_errors(&mut self, program: &Program) {
        // Locate function definitions by name so we can attach an accurate
        // span to each finding (verify returns only a line number).
        let mut func_spans: std::collections::HashMap<String, Span> =
            std::collections::HashMap::new();
        for stmt in &program.statements {
            if let Statement::FuncDef(fd) = stmt {
                func_spans
                    .entry(fd.name.clone())
                    .or_insert_with(|| fd.span.clone());
            }
        }

        // The file path is informational for the verify layer; type errors
        // carry their own spans so we pass a neutral marker here.
        let file = self
            .source_file
            .as_deref()
            .and_then(|p| p.to_str())
            .unwrap_or("<program>");

        // Always run the cross-backend non-tail mutual recursion check.
        // E32B-023 (Lock-N): when the active compile target lowers through
        // the C / wasm-C runtime (Native or wasm-*), additionally reject
        // *any* mutual cycle (tail or non-tail) with `[E0700]` because
        // those backends lack the trampoline that Interpreter / JS use.
        let mut findings = crate::graph::verify::run_check("mutual-recursion", program, file);
        if self.compile_target.is_native_lowering() {
            findings.extend(crate::graph::verify::run_check(
                "mutual-recursion-native",
                program,
                file,
            ));
        }

        for f in findings {
            if !matches!(f.severity, crate::graph::verify::Severity::Error) {
                continue;
            }
            // Best-effort: pick the first function name in the message
            // (formatted as "A -> B -> ... -> A") to anchor the span.
            let span = f
                .line
                .map(|line| Span {
                    line,
                    column: 1,
                    node_id: 0,
                    start: 0,
                    end: 0,
                })
                .or_else(|| {
                    // fall back: first function name mentioned in the msg
                    f.message.split_whitespace().find_map(|tok| {
                        let name = tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                        func_spans.get(name).cloned()
                    })
                })
                .unwrap_or(Span {
                    line: 1,
                    column: 1,
                    node_id: 0,
                    start: 0,
                    end: 0,
                });
            self.errors.push(TypeError {
                message: f.message,
                span,
            });
        }
    }

    fn mold_header_type_compatible(&self, actual: &Type, expected: &Type) -> bool {
        match (actual, expected) {
            (Type::Unknown, Type::Unknown) => true,
            (Type::Unknown, _) | (_, Type::Unknown) => false,
            (
                Type::Function(actual_params, actual_ret),
                Type::Function(expected_params, expected_ret),
            ) => {
                actual_params.len() == expected_params.len()
                    && actual_params.iter().zip(expected_params.iter()).all(
                        |(actual_param, expected_param)| {
                            self.mold_header_type_compatible(actual_param, expected_param)
                                && self.mold_header_type_compatible(expected_param, actual_param)
                        },
                    )
                    && self.mold_header_type_compatible(actual_ret, expected_ret)
            }
            _ => self.registry.is_subtype_of(actual, expected),
        }
    }

    fn builtin_mold_kind_matches(
        &self,
        actual: &Type,
        kind: crate::types::mold_specs::MoldArgKind,
    ) -> bool {
        use crate::types::mold_specs::MoldArgKind;

        if matches!(actual, Type::Unknown | Type::Any) {
            return true;
        }
        match kind {
            MoldArgKind::Any => true,
            MoldArgKind::Bool => actual == &Type::Bool,
            MoldArgKind::Function => matches!(actual, Type::Function(_, _)),
            MoldArgKind::Int => actual == &Type::Int,
            MoldArgKind::Str => actual == &Type::Str,
            MoldArgKind::NullaryFunction => {
                matches!(actual, Type::Function(params, _) if params.is_empty())
            }
            MoldArgKind::UnaryFunction => {
                matches!(actual, Type::Function(params, _) if params.len() == 1)
            }
            MoldArgKind::UnaryPredicate => match actual {
                Type::Function(params, ret) if params.len() == 1 => {
                    matches!(ret.as_ref(), Type::Bool | Type::Unknown | Type::Any)
                }
                _ => false,
            },
            MoldArgKind::BinaryFunction => {
                matches!(actual, Type::Function(params, _) if params.len() == 2)
            }
            MoldArgKind::List => matches!(actual, Type::List(_)),
            MoldArgKind::ListOrStream => {
                matches!(actual, Type::List(_))
                    || matches!(actual, Type::Generic(name, _) if name == "Stream")
            }
            MoldArgKind::Numeric => actual.is_numeric(),
        }
    }

    fn builtin_mold_kind_label(kind: crate::types::mold_specs::MoldArgKind) -> &'static str {
        use crate::types::mold_specs::MoldArgKind;

        match kind {
            MoldArgKind::Any => "any value",
            MoldArgKind::Bool => "Bool",
            MoldArgKind::Function => "function",
            MoldArgKind::Int => "Int",
            MoldArgKind::Str => "Str",
            MoldArgKind::NullaryFunction => "zero-argument function",
            MoldArgKind::UnaryFunction => "1-argument function",
            MoldArgKind::UnaryPredicate => "1-argument Bool predicate",
            MoldArgKind::BinaryFunction => "2-argument function",
            MoldArgKind::List => "List",
            MoldArgKind::ListOrStream => "List or Stream",
            MoldArgKind::Numeric => "numeric",
        }
    }

    fn bind_mold_header_arg(
        &self,
        arg: &MoldHeaderArg,
        actual: &Type,
        bound_types: &mut HashMap<String, Type>,
    ) {
        if let MoldHeaderArg::TypeParam(tp) = arg {
            bound_types.insert(tp.name.clone(), actual.clone());
        }
    }

    fn bind_generic_type_pattern(
        &self,
        pattern: &Type,
        actual: &Type,
        generic_names: &HashSet<String>,
        bindings: &mut HashMap<String, Type>,
    ) -> bool {
        match pattern {
            Type::Named(name) if generic_names.contains(name) => {
                if actual == &Type::Unknown {
                    return true;
                }
                if let Some(bound) = bindings.get(name) {
                    self.mold_header_type_compatible(actual, bound)
                        && self.mold_header_type_compatible(bound, actual)
                } else {
                    bindings.insert(name.clone(), actual.clone());
                    true
                }
            }
            Type::List(pattern_inner) => match actual {
                Type::List(actual_inner) => self.bind_generic_type_pattern(
                    pattern_inner,
                    actual_inner,
                    generic_names,
                    bindings,
                ),
                _ => false,
            },
            Type::Generic(pattern_name, pattern_args) => match actual {
                Type::Generic(actual_name, actual_args)
                    if pattern_name == actual_name && pattern_args.len() == actual_args.len() =>
                {
                    pattern_args
                        .iter()
                        .zip(actual_args.iter())
                        .all(|(pattern_arg, actual_arg)| {
                            self.bind_generic_type_pattern(
                                pattern_arg,
                                actual_arg,
                                generic_names,
                                bindings,
                            )
                        })
                }
                _ => false,
            },
            Type::BuchiPack(pattern_fields) => match actual {
                Type::BuchiPack(actual_fields) => {
                    pattern_fields.iter().all(|(pattern_name, pattern_ty)| {
                        actual_fields
                            .iter()
                            .find(|(actual_name, _)| actual_name == pattern_name)
                            .is_some_and(|(_, actual_ty)| {
                                self.bind_generic_type_pattern(
                                    pattern_ty,
                                    actual_ty,
                                    generic_names,
                                    bindings,
                                )
                            })
                    })
                }
                _ => false,
            },
            Type::Function(pattern_params, pattern_ret) => match actual {
                Type::Function(actual_params, actual_ret)
                    if pattern_params.len() == actual_params.len() =>
                {
                    pattern_params.iter().zip(actual_params.iter()).all(
                        |(pattern_param, actual_param)| {
                            self.bind_generic_type_pattern(
                                pattern_param,
                                actual_param,
                                generic_names,
                                bindings,
                            )
                        },
                    ) && self.bind_generic_type_pattern(
                        pattern_ret,
                        actual_ret,
                        generic_names,
                        bindings,
                    )
                }
                _ => false,
            },
            _ => self.registry.is_subtype_of(actual, pattern),
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

    fn finalize_named_function_signature(&mut self, fd: &FuncDef) -> Option<(Vec<Type>, Type)> {
        let Some(return_type) = &fd.return_type else {
            self.errors.push(TypeError {
                message: format!(
                    "[E1526] Function '{}' must declare a return type with `=> :Type`.",
                    fd.name
                ),
                span: fd.span.clone(),
            });
            return None;
        };

        let ret_ty = self.registry.resolve_type(return_type);
        let mut param_types: Vec<Type> = fd
            .params
            .iter()
            .map(|p| {
                p.type_annotation
                    .as_ref()
                    .map(|t| self.registry.resolve_type(t))
                    .unwrap_or(Type::Unknown)
            })
            .collect();

        if let Some(tail_expr) = fd.body.last().and_then(Statement::yielded_expr) {
            self.current_func_type_params.push(fd.type_params.clone());
            self.collect_named_function_param_constraints(fd, tail_expr, &ret_ty, &mut param_types);
            self.current_func_type_params.pop();
        }

        let mut ok = true;
        for (idx, param) in fd.params.iter().enumerate() {
            let ty = param_types.get(idx).cloned().unwrap_or(Type::Unknown);
            if Self::contains_unknown(&ty) {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1525] Cannot infer type of parameter '{}' in function '{}'. Add a type annotation.",
                        param.name, fd.name
                    ),
                    span: param.span.clone(),
                });
                ok = false;
            }
        }

        ok.then_some((param_types, ret_ty))
    }

    fn collect_named_function_param_constraints(
        &mut self,
        fd: &FuncDef,
        expr: &Expr,
        expected: &Type,
        param_types: &mut [Type],
    ) {
        match expr {
            Expr::Ident(name, span) => {
                self.constrain_named_function_param(fd, name, expected, param_types, span);
            }
            Expr::BinaryOp(left, op, right, span) => {
                if let Some(operand_ty) = self.binary_operand_constraint_from_expected(op, expected)
                {
                    self.collect_named_function_param_constraints(
                        fd,
                        left,
                        &operand_ty,
                        param_types,
                    );
                    self.collect_named_function_param_constraints(
                        fd,
                        right,
                        &operand_ty,
                        param_types,
                    );
                } else if matches!(op, BinOp::Add) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1525] Cannot resolve overloaded '+' in function '{}'. Add parameter annotations or use a concrete return type.",
                            fd.name
                        ),
                        span: span.clone(),
                    });
                }
            }
            Expr::UnaryOp(_, inner, _) => {
                self.collect_named_function_param_constraints(fd, inner, expected, param_types);
            }
            Expr::Unmold(base, _) | Expr::Throw(base, _) => {
                self.collect_named_function_param_constraints(fd, base, expected, param_types);
            }
            Expr::FieldAccess(_, _, _) => {}
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(arm_expr) = arm.last_expr() {
                        self.collect_named_function_param_constraints(
                            fd,
                            arm_expr,
                            expected,
                            param_types,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn binary_operand_constraint_from_expected(&self, op: &BinOp, expected: &Type) -> Option<Type> {
        match op {
            BinOp::Add => match expected {
                Type::Int | Type::Float | Type::Num | Type::Str => Some(expected.clone()),
                Type::Named(name) if self.type_param_is_numeric(name) => Some(expected.clone()),
                _ => None,
            },
            BinOp::Sub | BinOp::Mul => match expected {
                Type::Int | Type::Float | Type::Num => Some(expected.clone()),
                Type::Named(name) if self.type_param_is_numeric(name) => Some(expected.clone()),
                _ => None,
            },
            BinOp::Lt | BinOp::Gt | BinOp::GtEq => None,
            BinOp::Eq | BinOp::NotEq | BinOp::And | BinOp::Or | BinOp::Concat => None,
        }
    }

    fn constrain_named_function_param(
        &mut self,
        fd: &FuncDef,
        name: &str,
        expected: &Type,
        param_types: &mut [Type],
        span: &Span,
    ) {
        if matches!(expected, Type::Unknown) || Self::contains_unknown(expected) {
            return;
        }
        let Some(idx) = fd.params.iter().position(|param| param.name == name) else {
            return;
        };
        let current = param_types.get(idx).cloned().unwrap_or(Type::Unknown);
        if current == Type::Unknown {
            param_types[idx] = expected.clone();
            return;
        }
        if current != *expected
            && !self.registry.is_subtype_of(&current, expected)
            && !self.registry.is_subtype_of(expected, &current)
        {
            self.errors.push(TypeError {
                message: format!(
                    "[E1525] Conflicting inferred type for parameter '{}' in function '{}': {} vs {}.",
                    name, fd.name, current, expected
                ),
                span: span.clone(),
            });
        }
    }

    // ── B11B-016: Mold-specific error pass (third pass) ──────────────
    // Recursively walks expressions to find mold patterns that need
    // rejection regardless of expression context. Separated from
    // infer_expr_type to avoid triggering unrelated type errors (e.g.,
    // E1510 on closure return types) in builtin function arguments.

    fn check_mold_errors_in_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assignment(a) => self.check_mold_errors_in_expr(&a.value),
            Statement::Expr(e) => self.check_mold_errors_in_expr(e),
            Statement::FuncDef(fd) => {
                for s in &fd.body {
                    self.check_mold_errors_in_stmt(s);
                }
            }
            Statement::ErrorCeiling(ec) => {
                for s in &ec.handler_body {
                    self.check_mold_errors_in_stmt(s);
                }
            }
            _ => {}
        }
    }

    fn check_mold_errors_in_expr(&mut self, expr: &Expr) {
        self.check_mold_errors_in_expr_ctx(expr, false);
    }

    fn check_mold_errors_in_expr_ctx(&mut self, expr: &Expr, in_cage_runner: bool) {
        match expr {
            // B11B-016: TypeExtends does not accept enum variant literals
            Expr::MoldInst(name, type_args, fields, _) => {
                if Self::is_cage_runner_constructor(name) && !in_cage_runner {
                    self.push_cage_error(
                        "[E1515]",
                        expr.span(),
                        format!(
                            "[E1515] `{}` is a Cage runner descriptor and cannot be executed directly. \
                             Hint: pass it as the second argument of `Cage[subject, {}[...]()]()`.",
                            name, name
                        ),
                    );
                }
                if Self::is_cage_rilla_child(name) && type_args.len() != 1 {
                    self.push_cage_error(
                        "[E1516]",
                        expr.span(),
                        format!(
                            "[E1516] {} takes exactly one `[]` output type argument. \
                             Hint: write `{}[Out]()`; the branch is implied by the child family.",
                            name, name
                        ),
                    );
                }
                if name == "TypeExtends" {
                    for arg in type_args {
                        if let Expr::TypeLiteral(enum_name, Some(variant_name), lit_span) = arg {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1613] TypeExtends does not accept enum variants (`{}:{}`). \
                                     Hint: Use TypeIs for variant checks (e.g., `TypeIs[value, {}:{}]()`).",
                                    enum_name, variant_name, enum_name, variant_name
                                ),
                                span: lit_span.clone(),
                            });
                        }
                    }
                }
                for (idx, arg) in type_args.iter().enumerate() {
                    let child_in_cage_runner = name == "Cage" && idx == 1;
                    self.check_mold_errors_in_expr_ctx(arg, child_in_cage_runner);
                }
                for f in fields {
                    self.check_mold_errors_in_expr_ctx(&f.value, false);
                }
            }
            Expr::FuncCall(callee, args, _) => {
                self.check_call_argument_limit("function call", args.len(), expr.span().clone());
                self.check_mold_errors_in_expr_ctx(callee, false);
                for arg in args {
                    self.check_mold_errors_in_expr_ctx(arg, false);
                }
            }
            Expr::MethodCall(obj, _, args, _) => {
                self.check_call_argument_limit("method call", args.len(), expr.span().clone());
                self.check_mold_errors_in_expr_ctx(obj, false);
                for arg in args {
                    self.check_mold_errors_in_expr_ctx(arg, false);
                }
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    self.check_mold_errors_in_expr_ctx(e, false);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.check_mold_errors_in_expr_ctx(cond, false);
                    }
                    for s in &arm.body {
                        self.check_mold_errors_in_stmt(s);
                    }
                }
            }
            Expr::BuchiPack(fields, span) | Expr::TypeInst(_, fields, span) => {
                // C12B-023 bypass closure root fix (2026-04-15 v2): reject
                // any user-authored BuchiPack / TypeInst literal that
                // assigns a `__`-prefixed field name, regardless of the
                // value expression. `__`-prefix field names are reserved
                // for compiler-internal tags (e.g., `__type`, `__value`,
                // `__default`, `__error`). Hand-rolled packs that set
                // these tags fabricate nominal-type identity without the
                // invariants that the official constructors guarantee
                // (e.g., `Regex(pattern, flags?)` validates the pattern;
                // `Lax` / `Async` / `Result` wrap values with specific
                // state discipline).
                //
                // Prior narrower fix (literal `__type <= "Regex"` only)
                // was bypassed via variable binding
                // (`tag <= "Regex"; @(__type <= tag, ...)`) and
                // expression composition. Rejecting at the field-name
                // level closes every indirect route (variable, arg,
                // if-expr, string concatenation) because the value
                // expression is no longer consulted. `[E1617]` is shared
                // with `emit_wasm_c::validate_regex_api_for_wasm` as the
                // runtime-side backstop.
                for f in fields {
                    if f.name.starts_with(RESERVED_INTERNAL_FIELD_PREFIX) {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1617] Field name `{}` is reserved for compiler-internal use \
                                 and may not be assigned in a user-authored pack. \
                                 The `__`-prefix marks tags that nominal-type constructors \
                                 (e.g., `Regex(pattern, flags?)`, `Lax(...)`, `Async(...)`) \
                                 populate to carry validated invariants. Hand-rolled packs \
                                 that set these fields fabricate fake nominal values, \
                                 bypass backend invariants (wasm: no regex runtime; \
                                 Interpreter/JS/Native: unvalidated payload), and produce \
                                 silent undefined behaviour (PHILOSOPHY I). \
                                 Hint: Use the official constructor (e.g., `Regex(pat, flags?)`) \
                                 or pick a non-`__`-prefixed field name for your own tag.",
                                f.name
                            ),
                            span: f.span.clone(),
                        });
                    }
                }
                let _ = span;
                for f in fields {
                    self.check_mold_errors_in_expr_ctx(&f.value, false);
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    self.check_mold_errors_in_expr_ctx(item, false);
                }
            }
            Expr::UnaryOp(_, inner, _) => self.check_mold_errors_in_expr_ctx(inner, false),
            Expr::BinaryOp(l, _, r, _) => {
                self.check_mold_errors_in_expr_ctx(l, false);
                self.check_mold_errors_in_expr_ctx(r, false);
            }
            Expr::Throw(inner, _) => self.check_mold_errors_in_expr_ctx(inner, false),
            Expr::FieldAccess(obj, _, _) => self.check_mold_errors_in_expr_ctx(obj, false),
            Expr::Lambda(_, body, _) => self.check_mold_errors_in_expr_ctx(body, false),
            // Leaf expressions — no recursion needed
            _ => {}
        }
    }

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

    fn check_call_argument_limit(&mut self, kind: &str, arg_count: usize, span: Span) {
        if arg_count <= MAX_CALL_ARGUMENTS {
            return;
        }
        self.errors.push(TypeError {
            message: format!(
                "[E1301] {} takes at most {} argument(s), got {}. Hint: Split the call or reduce arity; native/WASM tag propagation is capped at {} arguments.",
                kind, MAX_CALL_ARGUMENTS, arg_count, MAX_CALL_ARGUMENTS
            ),
            span,
        });
    }

    /// narrow walker that triggers full type inference only on
    /// FieldAccess nodes inside builtin call arguments (e.g.
    /// `stdout(r.__value.stdout)`). This lets us surface pinned-Gorillax
    /// field-access rejections without retroactively tightening other
    /// builtin arg subtrees (BinaryOp / MethodCall / etc.) that earlier
    /// callers were silently relying on.
    ///
    /// The returned type is intentionally discarded; we only care about
    /// errors pushed into `self.errors` during traversal.
    fn check_pinned_field_access_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::FieldAccess(_, _, _) => {
                let _ = self.infer_expr_type(expr);
            }
            Expr::MethodCall(obj, _, args, _) => {
                self.check_pinned_field_access_in_expr(obj);
                for arg in args {
                    self.check_pinned_field_access_in_expr(arg);
                }
            }
            Expr::FuncCall(callee, args, _) => {
                self.check_pinned_field_access_in_expr(callee);
                for arg in args {
                    self.check_pinned_field_access_in_expr(arg);
                }
            }
            Expr::BinaryOp(l, _, r, _) => {
                self.check_pinned_field_access_in_expr(l);
                self.check_pinned_field_access_in_expr(r);
            }
            Expr::UnaryOp(_, inner, _) => self.check_pinned_field_access_in_expr(inner),
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    self.check_pinned_field_access_in_expr(e);
                }
            }
            _ => {}
        }
    }

    fn check_str_plus_known_non_str_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::BinaryOp(lhs, BinOp::Add, rhs, _) => {
                let lhs_type = Self::static_add_operand_type(lhs);
                let rhs_type = Self::static_add_operand_type(rhs);

                let lhs_bad = matches!(lhs_type, Some(Type::Str))
                    && !matches!(rhs_type, Some(Type::Str) | None);
                let rhs_bad = matches!(rhs_type, Some(Type::Str))
                    && !matches!(lhs_type, Some(Type::Str) | None);
                if lhs_bad || rhs_bad {
                    let _ = self.infer_expr_type(expr);
                } else {
                    self.check_str_plus_known_non_str_in_expr(lhs);
                    self.check_str_plus_known_non_str_in_expr(rhs);
                }
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                self.check_str_plus_known_non_str_in_expr(lhs);
                self.check_str_plus_known_non_str_in_expr(rhs);
            }
            Expr::MethodCall(obj, _, args, _) => {
                self.check_str_plus_known_non_str_in_expr(obj);
                for arg in args {
                    self.check_str_plus_known_non_str_in_expr(arg);
                }
            }
            Expr::FuncCall(callee, args, _) => {
                self.check_str_plus_known_non_str_in_expr(callee);
                for arg in args {
                    self.check_str_plus_known_non_str_in_expr(arg);
                }
            }
            Expr::UnaryOp(_, inner, _) | Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
                self.check_str_plus_known_non_str_in_expr(inner);
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    self.check_str_plus_known_non_str_in_expr(e);
                }
            }
            Expr::ListLit(items, _) => {
                for e in items {
                    self.check_str_plus_known_non_str_in_expr(e);
                }
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for field in fields {
                    self.check_str_plus_known_non_str_in_expr(&field.value);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.check_str_plus_known_non_str_in_expr(cond);
                    }
                    for stmt in &arm.body {
                        if let Statement::Expr(e) = stmt {
                            self.check_str_plus_known_non_str_in_expr(e);
                        }
                    }
                }
            }
            Expr::Lambda(_, body, _) => self.check_str_plus_known_non_str_in_expr(body),
            Expr::FieldAccess(obj, _, _) => self.check_str_plus_known_non_str_in_expr(obj),
            _ => {}
        }
    }

    // ── Comparison diagnostics in skipped expression contexts ──
    //
    // Some containers know their own type without fully inferring children
    // (for example builtin function args, method args with `Unknown`
    // parameters, lambdas passed as values, and TemplateLit raw strings).
    // The old implementation ran a whole-program fourth pass with its own
    // scope reconstruction.  That both re-inferred nested expressions and
    // could drift from the main pass.  This walker is started from main
    // inference paths that may skip child expressions or treat their argument
    // signature as Unknown, and records only `[E1605]` diagnostics from those
    // speculative walks.
    fn run_comparison_error_walk(&mut self, expr: &Expr) {
        if self.in_comparison_error_walk {
            return;
        }
        self.in_comparison_error_walk = true;
        self.check_comparison_errors_in_expr(expr);
        self.in_comparison_error_walk = false;
    }

    fn check_comparison_errors_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::BinaryOp(_, _, _, _) => {
                let _ = self.infer_expr_type_recording_only_e1605(expr);
            }
            Expr::UnaryOp(_, inner, _) | Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
                self.check_comparison_errors_in_expr(inner);
            }
            Expr::FuncCall(callee, args, _) => {
                self.check_comparison_errors_in_expr(callee);
                for arg in args {
                    self.check_comparison_errors_in_expr(arg);
                }
            }
            Expr::MethodCall(obj, _, args, _) => {
                self.check_comparison_errors_in_expr(obj);
                for arg in args {
                    self.check_comparison_errors_in_expr(arg);
                }
            }
            Expr::FieldAccess(obj, _, _) => self.check_comparison_errors_in_expr(obj),
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for field in fields {
                    self.check_comparison_errors_in_expr(&field.value);
                }
            }
            Expr::ListLit(items, _) | Expr::Pipeline(items, _) => {
                for item in items {
                    self.check_comparison_errors_in_expr(item);
                }
            }
            Expr::MoldInst(_, type_args, fields, _) => {
                for arg in type_args {
                    self.check_comparison_errors_in_expr(arg);
                }
                for field in fields {
                    self.check_comparison_errors_in_expr(&field.value);
                }
            }
            Expr::CondBranch(_, _) => {
                let _ = self.infer_expr_type_recording_only_e1605(expr);
            }
            Expr::Lambda(params, body, _) => {
                self.push_scope();
                for param in params {
                    if let Some(default_value) = &param.default_value {
                        self.check_comparison_errors_in_expr(default_value);
                    }
                    let ty = param
                        .type_annotation
                        .as_ref()
                        .map(|ty| self.registry.resolve_type(ty))
                        .unwrap_or(Type::Unknown);
                    self.define_var_silent(&param.name, ty);
                }
                self.check_comparison_errors_in_expr(body);
                self.pop_scope();
            }
            Expr::TemplateLit(template, span) => {
                self.check_comparison_errors_in_template(template, span)
            }
            Expr::IntLit(_, _)
            | Expr::FloatLit(_, _)
            | Expr::StringLit(_, _)
            | Expr::BoolLit(_, _)
            | Expr::Gorilla(_)
            | Expr::Ident(_, _)
            | Expr::Placeholder(_)
            | Expr::Hole(_)
            | Expr::EnumVariant(_, _, _)
            | Expr::TypeLiteral(_, _, _) => {}
        }
    }

    fn check_comparison_errors_in_template(&mut self, template: &str, span: &Span) {
        let chars: Vec<char> = template.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                i += 2;
                let start = i;
                let mut depth = 1;
                while i < chars.len() && depth > 0 {
                    if chars[i] == '{' {
                        depth += 1;
                    }
                    if chars[i] == '}' {
                        depth -= 1;
                    }
                    if depth > 0 {
                        i += 1;
                    }
                }
                let expr_str: String = chars[start..i].iter().collect();
                let trimmed = expr_str.trim();
                if let Some(parsed_expr) = Self::parse_template_interpolation_expr(trimmed) {
                    let error_count = self.errors.len();
                    self.check_comparison_errors_in_expr(&parsed_expr);
                    for err in &mut self.errors[error_count..] {
                        if err.message.contains("[E1605]") {
                            err.span = span.clone();
                        }
                    }
                }
                if i < chars.len() {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
    }

    // E32B-045: When the interpolation source has trailing syntax errors
    // (e.g. `foo == "x" |> bar` — `|>` is not valid in expression context),
    // the parser still produces a partial AST for the prefix that *did*
    // parse cleanly (`foo == "x"`). Earlier code dropped the partial AST
    // whenever `parse_errors` was non-empty, which silently hid `[E1605]`
    // detection on any comparison sitting inside such an interpolation.
    // We now accept the partial AST and let `check_comparison_errors_in_expr`
    // walk it as a best-effort diagnosis: comparison prefixes that *did*
    // parse get diagnosed, and downstream `Type::Unknown` guards keep
    // false positives away on the missing pieces. This is a diagnostic
    // policy rather than a soundness proof — the goal is to refuse to
    // miss `[E1605]` just because a tail of the interpolation failed to
    // tokenize, not to claim soundness in the presence of arbitrary
    // partial trees.
    fn parse_template_interpolation_expr(source: &str) -> Option<Expr> {
        fn parse_expr(source: &str) -> Option<Expr> {
            let (program, _parse_errors) = crate::parser::parse(source);
            if let Some(Statement::Expr(parsed_expr)) = program.statements.first() {
                return Some(parsed_expr.clone());
            }
            None
        }

        parse_expr(source).or_else(|| parse_expr(&format!("({source})")))
    }

    fn func_call_args_need_comparison_walk(&self, func: &Expr, args: &[Expr]) -> bool {
        fn args_with_unknown_expected_need_walk(args: &[Expr], params: &[Type]) -> bool {
            args.iter().enumerate().any(|(i, arg)| {
                if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                    return false;
                }
                params
                    .get(i)
                    .is_none_or(|expected| matches!(expected, Type::Unknown))
            })
        }

        let Expr::Ident(name, _) = func else {
            return true;
        };

        if self.generic_func_defs.contains_key(name) {
            // Generic function dispatch infers every provided argument while
            // binding type parameters, so an additional E1605 walk would only
            // duplicate that work.
            return false;
        }
        if let Some(param_types) = self.func_param_types.get(name) {
            return args_with_unknown_expected_need_walk(args, param_types);
        }
        if self.func_types.contains_key(name) {
            return true;
        }
        if let Some(Type::Function(params, _)) = self.lookup_var(name) {
            return args_with_unknown_expected_need_walk(args, &params);
        }
        if let Some(Type::Named(var_name)) = self.lookup_var(name)
            && let Some(Type::Function(params, _)) = self.type_param_function_constraint(&var_name)
        {
            return args_with_unknown_expected_need_walk(args, &params);
        }
        true
    }

    // The two complex `if` guards under each `BinOp` arm cover several
    // distinct fall-through cases; collapsing them into match-arm guards
    // pushes long boolean expressions next to the pattern and hurts
    // readability without changing semantics.
    #[allow(clippy::collapsible_match)]
    fn emit_comparison_mismatch_if_needed(
        &mut self,
        left_type: &Type,
        op: &BinOp,
        right_type: &Type,
        span: &Span,
    ) {
        let left_is_numeric_var =
            matches!(left_type, Type::Named(n) if self.type_param_is_numeric(n));
        let right_is_numeric_var =
            matches!(right_type, Type::Named(n) if self.type_param_is_numeric(n));
        let left_is_numeric_ext = left_type.is_numeric() || left_is_numeric_var;
        let right_is_numeric_ext = right_type.is_numeric() || right_is_numeric_var;

        match op {
            BinOp::Eq | BinOp::NotEq => {
                if left_type != &Type::Unknown
                    && right_type != &Type::Unknown
                    && !Self::contains_unknown(left_type)
                    && !Self::contains_unknown(right_type)
                    && left_type != right_type
                    && !(left_type.is_numeric() && right_type.is_numeric())
                    && !(left_is_numeric_ext && right_is_numeric_ext)
                    && !self.registry.is_subtype_of(left_type, right_type)
                    && !self.registry.is_subtype_of(right_type, left_type)
                {
                    self.push_e1605_once(
                        span,
                        format!(
                            "[E1605] Cannot compare {} with {} using {:?}. \
                             Hint: Both operands should be of compatible types.",
                            left_type, right_type, op
                        ),
                    );
                }
            }
            BinOp::Lt | BinOp::Gt | BinOp::GtEq => {
                if left_type != &Type::Unknown
                    && right_type != &Type::Unknown
                    && !Self::contains_unknown(left_type)
                    && !Self::contains_unknown(right_type)
                {
                    let both_numeric = left_type.is_numeric() && right_type.is_numeric();
                    let both_str =
                        matches!(left_type, Type::Str) && matches!(right_type, Type::Str);
                    let same_enum = match (left_type, right_type) {
                        (Type::Named(a), Type::Named(b)) => a == b && self.registry.is_enum_type(a),
                        _ => false,
                    };
                    let both_numeric_ext = left_is_numeric_ext && right_is_numeric_ext;
                    let valid = both_numeric || both_numeric_ext || both_str || same_enum;
                    if !valid {
                        self.push_e1605_once(
                            span,
                            format!(
                                "[E1605] Cannot compare {} with {} using {:?}. \
                                 Hint: Ordering comparison requires numeric, string, or same-Enum operands. \
                                 For Enum↔Int comparisons use `Ordinal[<enum>]()` to obtain the Int first.",
                                left_type, right_type, op
                            ),
                        );
                    }
                }
            }
            _ => {}
        }
    }

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

    /// Type-check a statement (second pass).
    fn check_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::EnumDef(_) => {}
            Statement::Assignment(assign) => {
                let is_addon_binding = assign.as_rust_addon_binding().is_some();
                let expected_annotation = assign
                    .type_annotation
                    .as_ref()
                    .map(|type_ann| self.registry.resolve_type(type_ann));
                let inferred = if let Some(expected) = &expected_annotation {
                    self.infer_expr_type_with_expected(&assign.value, expected)
                } else {
                    self.infer_expr_type(&assign.value)
                };

                // If there's a type annotation, check compatibility
                if let Some(expected) = expected_annotation {
                    if !self.registry.is_subtype_of(&inferred, &expected)
                        && inferred != Type::Unknown
                    {
                        self.errors.push(TypeError {
                            message: format!(
                                "Type mismatch in assignment to '{}': expected {}, got {}",
                                assign.target, expected, inferred
                            ),
                            span: assign.span.clone(),
                        });
                    }
                    // Register with the annotated type
                    if self.define_var_with_span(&assign.target, expected, Some(&assign.span)) {
                        self.define_string_const_from_expr(&assign.target, &assign.value);
                        self.define_branch_info(
                            &assign.target,
                            self.branch_info_for_assignment_expr(&assign.value, &inferred),
                        );
                    }
                } else {
                    // @[] without type annotation is ambiguous — element type is unknown
                    if matches!(&inferred, Type::List(inner) if matches!(inner.as_ref(), Type::Unknown))
                        && matches!(&assign.value, Expr::ListLit(items, _) if items.is_empty())
                    {
                        self.errors.push(TypeError {
                                message: format!(
                                    "Empty list literal `@[]` requires a type annotation (e.g., `{}: @[Int] <= @[]`). Element type cannot be inferred.",
                                    assign.target
                                ),
                                span: assign.span.clone(),
                            });
                    }
                    // Register with the inferred type
                    let branch_info =
                        self.branch_info_for_assignment_expr(&assign.value, &inferred);
                    if self.define_var_with_span(&assign.target, inferred, Some(&assign.span)) {
                        self.define_string_const_from_expr(&assign.target, &assign.value);
                        self.define_branch_info(&assign.target, branch_info);
                    }
                }
                if is_addon_binding {
                    self.worker_addon_symbols.insert(assign.target.clone());
                }
            }
            Statement::FuncDef(fd) => {
                let ret_ty = self
                    .func_types
                    .get(&fd.name)
                    .cloned()
                    .or_else(|| {
                        fd.return_type
                            .as_ref()
                            .map(|t| self.registry.resolve_type(t))
                    })
                    .unwrap_or(Type::Unknown);
                let param_types: Vec<Type> = self
                    .func_param_types
                    .get(&fd.name)
                    .cloned()
                    .unwrap_or_else(|| {
                        fd.params
                            .iter()
                            .map(|p| {
                                p.type_annotation
                                    .as_ref()
                                    .map(|t| self.registry.resolve_type(t))
                                    .unwrap_or(Type::Unknown)
                            })
                            .collect()
                    });

                // F42 sweep [E1520] R1: reject `:@()` / `:Unit` / `:Void` as
                // return type annotation on Taida-surface function definitions.
                // PHILOSOPHY I の系「値の不在は値の不在」: 「情報なしを意味する型」を関数戻り型に書くこと自体を禁止する。
                // 再帰的に Async[Unit] / Result[Unit, _] / Optional[Unit] / List[Unit] /
                // Function([Unit], Unit) 等のネストした unit-like 型も検出する。
                if fd.return_type.is_some() && Self::contains_unit_like_type(&ret_ty) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1520] Function '{}' declares return type {} ('value-absence' type, possibly nested). \
                             Taida forbids `:@()` / `:Unit` / `:Void` (including nested forms like `:Async[Unit]`, \
                             `:Result[Unit, _]`, `:List[Unit]`, `:Function([Unit], Unit)`) as function return type \
                             annotations. Return a meaningful value instead (e.g., `:Int` for byte count, `:Bool` \
                             for status, a structured BuchiPack, or a common Enum variant such as `:OpStatus`). \
                             See PHILOSOPHY.md I and docs/reference/diagnostic_codes.md [E1520].",
                            fd.name, ret_ty
                        ),
                        span: fd.span.clone(),
                    });
                }

                // F42 sweep [E1520] R1 対称版: reject `:@()` / `:Unit` / `:Void` as
                // parameter type annotation on Taida-surface function definitions
                // (再帰検出も含む).
                for (idx, param) in fd.params.iter().enumerate() {
                    if param.type_annotation.is_some()
                        && let Some(pty) = param_types.get(idx)
                        && Self::contains_unit_like_type(pty)
                    {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1520] Function '{}' parameter '{}' has type annotation {} ('value-absence' type, possibly nested). \
                                 Taida forbids `:@()` / `:Unit` / `:Void` (including nested forms like `:Async[Unit]`, \
                                 `:Result[Unit, _]`) as parameter type annotations. Use a meaningful concrete type instead. \
                                 See PHILOSOPHY.md I and docs/reference/diagnostic_codes.md [E1520].",
                                fd.name, param.name, pty
                            ),
                            span: fd.span.clone(),
                        });
                    }
                }

                // Register the name in scope so duplicate detection still works.
                // Invalid generic functions stay non-callable by using `Unknown`.
                let function_value_ty = if self.invalid_func_defs.contains(&fd.name) {
                    Type::Unknown
                } else {
                    Type::Function(param_types.clone(), Box::new(ret_ty.clone()))
                };
                self.define_var_with_span(&fd.name, function_value_ty, Some(&fd.span));
                if !self.invalid_func_defs.contains(&fd.name) {
                    self.func_def_scope_depths
                        .insert(fd.name.clone(), self.scope_stack.len().saturating_sub(1));
                }

                // Push new scope for function body
                self.push_scope();

                // D28B-023 / D28B-024: make this function's generic type
                // parameters visible to the body so that constrained type
                // variables can resolve operator dispatch (`+` on `T <= :Num`)
                // and call dispatch (`fn(x)` where `fn: F <= :T => :T`).
                self.current_func_type_params.push(fd.type_params.clone());

                // Validate defaults left-to-right and register params in scope order.
                self.validate_function_param_defaults(fd, &param_types);

                // Check function body.
                // FL-1 / Fix 6: When a return type annotation exists, avoid
                // double-inferring the last expression (once via check_statement,
                // once for the return-type check).  We check all statements
                // except the last one first, then handle the last one with the
                // return-type comparison so that infer_expr_type is called
                // exactly once and errors are never duplicated.
                let body_len = fd.body.len();
                let has_return_check = ret_ty != Type::Unknown && body_len > 0;
                let check_up_to = if has_return_check {
                    body_len - 1
                } else {
                    body_len
                };
                for body_stmt in fd.body.iter().take(check_up_to) {
                    self.check_statement(body_stmt);
                }

                // FL-1 + C13-1: Enforce return type annotation against body's tail value.
                // The tail value is:
                //   - `Statement::Expr(e)` → the value of `e` (classic form)
                //   - `Statement::Assignment(a)` → the bound value of `a.value`
                //     (C13-1 tail binding `name <= expr` / `expr => name`)
                //   - `Statement::UnmoldForward(u)` / `UnmoldBackward(u)` →
                //     the unmolded value (C13-1 tail unmold)
                let mut inferred_body_ret = None;
                if has_return_check {
                    let last_stmt = &fd.body[body_len - 1];
                    let body_ty_opt = match last_stmt {
                        Statement::Expr(last_expr) => {
                            Some(self.infer_expr_type_with_expected(last_expr, &ret_ty))
                        }
                        Statement::Assignment(_)
                        | Statement::UnmoldForward(_)
                        | Statement::UnmoldBackward(_) => {
                            // Run check_statement so the target binding is
                            // registered (errors in RHS are surfaced here).
                            // Then look up the bound variable's registered
                            // type to avoid double-inference of the RHS.
                            self.check_statement(last_stmt);
                            let bound_name = match last_stmt {
                                Statement::Assignment(a) => &a.target,
                                Statement::UnmoldForward(u) => &u.target,
                                Statement::UnmoldBackward(u) => &u.target,
                                _ => unreachable!(),
                            };
                            Some(self.lookup_var(bound_name).unwrap_or(Type::Unknown))
                        }
                        _ => None,
                    };

                    if let Some(body_ty) = body_ty_opt {
                        if !(body_ty == Type::Unknown
                            || Self::contains_unknown(&body_ty)
                            || self.registry.is_subtype_of(&body_ty, &ret_ty)
                            // Allow numeric narrowing: Num body is compatible with Int/Float/Num return
                            || body_ty.is_numeric() && ret_ty.is_numeric()
                            // RCB-50: Named/List/BuchiPack are now properly checked
                            // via is_subtype_of. The previous blanket skip hid genuine
                            // return-type mismatches.
                            || ret_ty == Type::Unknown
                            || self.contains_unresolved_type_var(&body_ty)
                            || self.contains_unresolved_type_var(&ret_ty)
                            || self.is_mold_defined_named(&body_ty))
                        {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1601] Function '{}' declares return type {}, but body returns {}. \
                                     Hint: Ensure the last expression in the function body matches the declared return type.",
                                    fd.name, ret_ty, body_ty
                                ),
                                span: fd.span.clone(),
                            });
                        }
                    } else {
                        // Last statement does not yield a value.
                        self.check_statement(last_stmt);
                        let is_unit_ret = ret_ty == Type::Unit
                            || matches!(&ret_ty, Type::Named(n) if n == "Unit");
                        if !is_unit_ret {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1601] Function '{}' declares return type {}, but the last statement is not an expression. \
                                     Hint: The function body's last statement must be an expression or a tail binding (`name <= expr`, `expr => name`, `expr >=> name`, `name <=< expr`) that produces a value.",
                                    fd.name, ret_ty
                                ),
                                span: fd.span.clone(),
                            });
                        }
                    }
                } else if body_len > 0 && !self.invalid_func_defs.contains(&fd.name) {
                    let last_stmt = &fd.body[body_len - 1];
                    let body_ty = match last_stmt {
                        Statement::Expr(last_expr) => self
                            .typed_expr_table
                            .lookup(last_expr)
                            .cloned()
                            .unwrap_or(Type::Unknown),
                        Statement::Assignment(a) => {
                            self.lookup_var(&a.target).unwrap_or(Type::Unknown)
                        }
                        Statement::UnmoldForward(u) => {
                            self.lookup_var(&u.target).unwrap_or(Type::Unknown)
                        }
                        Statement::UnmoldBackward(u) => {
                            self.lookup_var(&u.target).unwrap_or(Type::Unknown)
                        }
                        _ => Type::Unknown,
                    };

                    // F42 sweep [E1520] R2 / R2 拡張: reject functions whose
                    // inferred return type is a "value-absence" type when no
                    // return annotation is provided. This closes the
                    // intermediate-variable bypass `x <= @() => x` and the
                    // direct tail `... => @()` form simultaneously.
                    if fd.type_params.is_empty()
                        && body_ty != Type::Unknown
                        && !Self::contains_unknown(&body_ty)
                        && Self::is_unit_like_type(&body_ty)
                    {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1520] Function '{}' has no return type annotation, but its body's final value resolves to {} \
                                 ('value-absence' type). Taida forbids `:@()` / `:Unit` / `:Void` from leaking as a function's \
                                 inferred return type. Return a meaningful value instead (e.g. `:Int` byte count, `:Bool` status, \
                                 a structured BuchiPack, or a common Enum variant). \
                                 See PHILOSOPHY.md I and docs/reference/diagnostic_codes.md [E1520].",
                                fd.name, body_ty
                            ),
                            span: fd.span.clone(),
                        });
                    }

                    if fd.type_params.is_empty()
                        && body_ty != Type::Unknown
                        && !Self::contains_unknown(&body_ty)
                    {
                        inferred_body_ret = Some(body_ty);
                    }
                }

                // D28B-023 / D28B-024: balance the type-param stack push above.
                self.current_func_type_params.pop();
                self.pop_scope();

                if let Some(body_ret) = inferred_body_ret {
                    self.func_types.insert(fd.name.clone(), body_ret.clone());
                    self.define_var_silent(
                        &fd.name,
                        Type::Function(param_types.clone(), Box::new(body_ret)),
                    );
                }
            }
            Statement::Expr(expr) => {
                self.infer_expr_type(expr);
            }
            Statement::ErrorCeiling(ec) => {
                // Push scope for error handler
                self.push_scope();

                // Register the error parameter
                let err_ty = self.registry.resolve_type(&ec.error_type);

                // F42 sweep [E1520] R1 対称版: reject `:@()` / `:Unit` / `:Void`
                // (recursive) as error-handler parameter type annotation.
                if Self::contains_unit_like_type(&err_ty) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1520] ErrorCeiling parameter '{}' has type annotation {} ('value-absence' type, possibly nested). \
                             Taida forbids `:@()` / `:Unit` / `:Void` (including nested forms) as handler parameter type annotations. \
                             See PHILOSOPHY.md I and docs/reference/diagnostic_codes.md [E1520].",
                            ec.error_param, err_ty
                        ),
                        span: ec.span.clone(),
                    });
                }

                self.define_var(&ec.error_param, err_ty);

                for body_stmt in &ec.handler_body {
                    self.check_statement(body_stmt);
                }

                // RCB-231/232: If the error ceiling declares a return type (`=> :Type`),
                // verify the handler body's last expression type is compatible.
                // Exemptions:
                // - Unit return: checker cannot distinguish Unit from BuchiPack(vec![])
                // - Gorilla (><): process exit, never returns
                // - Named/List/BuchiPack body: mold/fold inference imprecision
                if let Some(ref ret_type_expr) = ec.return_type {
                    let declared_ret = self.registry.resolve_type(ret_type_expr);

                    // F42 sweep [E1520] R1: reject `:@()` / `:Unit` / `:Void`
                    // (recursive) as ErrorCeiling return-type annotation.
                    if Self::contains_unit_like_type(&declared_ret) {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1520] ErrorCeiling declares return type {} ('value-absence' type, possibly nested). \
                                 Taida forbids `:@()` / `:Unit` / `:Void` (including nested forms like `:Async[Unit]`) \
                                 as ErrorCeiling return type annotations. Return a meaningful value instead. \
                                 See PHILOSOPHY.md I and docs/reference/diagnostic_codes.md [E1520].",
                                declared_ret
                            ),
                            span: ec.span.clone(),
                        });
                    }

                    let is_unit_ret = matches!(declared_ret, Type::Unit)
                        || matches!(&declared_ret, Type::Named(n) if n == "Unit");
                    if !matches!(declared_ret, Type::Unknown)
                        && !is_unit_ret
                        && let Some(last_stmt) = ec.handler_body.last()
                    {
                        // C13-1: support tail binding forms in handler body.
                        // Skip if the last expression is Gorilla (><) — never returns.
                        let is_never_returns =
                            matches!(last_stmt, Statement::Expr(Expr::Gorilla(_)));
                        let body_ty_opt = if is_never_returns {
                            None
                        } else {
                            match last_stmt {
                                Statement::Expr(last_expr) => Some(self.infer_expr_type(last_expr)),
                                Statement::Assignment(a) => {
                                    // The binding was already recorded by the loop above.
                                    // Look up the bound variable to avoid double-inference.
                                    Some(self.lookup_var(&a.target).unwrap_or(Type::Unknown))
                                }
                                Statement::UnmoldForward(u) => {
                                    Some(self.lookup_var(&u.target).unwrap_or(Type::Unknown))
                                }
                                Statement::UnmoldBackward(u) => {
                                    Some(self.lookup_var(&u.target).unwrap_or(Type::Unknown))
                                }
                                _ => None,
                            }
                        };

                        if let Some(body_ty) = body_ty_opt {
                            // Also treat empty BuchiPack as Unit
                            let is_unit_body = matches!(body_ty, Type::Unit)
                                || matches!(&body_ty, Type::BuchiPack(f) if f.is_empty());
                            // RCB-241: Aligned with FuncDef return type check (FL-1 / RCB-50)
                            if !(matches!(body_ty, Type::Unknown)
                                || is_unit_body
                                || Self::contains_unknown(&body_ty)
                                || self.registry.is_subtype_of(&body_ty, &declared_ret)
                                || body_ty.is_numeric() && declared_ret.is_numeric()
                                || self.contains_unresolved_type_var(&body_ty)
                                || self.contains_unresolved_type_var(&declared_ret)
                                || self.is_mold_defined_named(&body_ty))
                            {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1601] Error handler declares return type {}, \
                                             but the handler body evaluates to {}. \
                                             Hint: The last expression in the |== handler \
                                             must produce a value compatible with the declared \
                                             return type.",
                                        declared_ret, body_ty
                                    ),
                                    span: ec.span.clone(),
                                });
                            }
                        } else if !is_never_returns {
                            // Non-expression, non-binding last statement.
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1601] Error handler declares return type {}, \
                                         but the last statement is not an expression. \
                                         Hint: The |== handler body's last statement must \
                                         be an expression or a tail binding (`name <= expr`, \
                                         `expr => name`, `expr >=> name`, `name <=< expr`) \
                                         that produces a value.",
                                    declared_ret
                                ),
                                span: ec.span.clone(),
                            });
                        }
                    }
                }

                self.pop_scope();
            }
            Statement::Import(imp) => {
                // RCB-201: Validate imported symbols against module's export list
                self.validate_import_symbols(imp);
                // C18-1: Register Enum types (and future TypeDefs) that cross the
                // module boundary so that `Color:Red()` in the importer resolves
                // without hitting [E1608]. Also detects variant-order mismatch
                // between a local redefinition and the imported module and emits
                // [E1618] when they disagree.
                self.register_imported_types(imp);
                self.register_worker_addon_imports(imp);
                // C19B-002: pin typed signatures for select `taida-lang/os`
                // symbols (runInteractive / execShellInteractive) so that
                // field access through their Gorillax result resolves at
                // compile time. Unpinned os symbols still fall through to
                // `Type::Unknown` below.
                let os_import = imp.path == "taida-lang/os";
                if imp.path == "taida-lang/abi" {
                    self.register_abi_imports(&imp.symbols);
                }
                for sym in &imp.symbols {
                    let name = sym.alias.as_ref().unwrap_or(&sym.name);
                    if imp.path == "taida-lang/net" || os_import {
                        self.worker_effect_symbols.insert(name.to_string());
                    }
                    if imp.path.starts_with("npm:") {
                        self.worker_addon_symbols.insert(name.to_string());
                    }
                    if imp.path == "taida-lang/net" {
                        self.register_net_import_symbol(&sym.name, name);
                    }
                    if os_import {
                        self.register_os_import_symbol(&sym.name, name);
                    }
                    if imp.path.starts_with("npm:") {
                        self.define_var(name, Type::Molten);
                        self.define_branch_info(name, BranchInfo::Molten(CageBranch::Js));
                    } else {
                        let value_ty = self
                            .imported_function_value_type(name)
                            .unwrap_or(Type::Unknown);
                        self.define_var(name, value_ty);
                    }
                }
            }
            Statement::UnmoldForward(uf) => {
                // `expr >=> target` -- target gets the unmolded (inner) value
                let source_ty = self.infer_expr_type(&uf.source);
                let target_ty = self.unmold_type(&source_ty);
                self.define_var_with_span(&uf.target, target_ty.clone(), Some(&uf.span));
                if target_ty == Type::Molten
                    && let Some(branch) = self.gorillax_value_branch_for_expr(&uf.source)
                {
                    self.define_branch_info(&uf.target, BranchInfo::Molten(branch));
                }
            }
            Statement::UnmoldBackward(ub) => {
                // `target <=< expr`
                let source_ty = self.infer_expr_type(&ub.source);
                let target_ty = self.unmold_type(&source_ty);
                self.define_var_with_span(&ub.target, target_ty.clone(), Some(&ub.span));
                if target_ty == Type::Molten
                    && let Some(branch) = self.gorillax_value_branch_for_expr(&ub.source)
                {
                    self.define_branch_info(&ub.target, BranchInfo::Molten(branch));
                }
            }
            Statement::Export(export) => {
                // RCB-102: `<<< @()` (empty export) is almost certainly a mistake.
                // A module that exports nothing is useless to importers, and the
                // current backend handling diverges (Interp: leak, JS: runtime error,
                // Native: linker error).  Reject at check time.
                if export.symbols.is_empty() && export.path.is_none() {
                    self.errors.push(TypeError {
                        message: "Empty export `<<< @()` exports nothing. \
                             If this module is not meant to be imported, remove the export statement. \
                             If you want to export symbols, list them: `<<< @(name1, name2)`."
                            .to_string(),
                        span: export.span.clone(),
                    });
                }
                // RCB-212: Re-export path `<<< ./path` is parsed but not implemented
                // in any backend. Emit an error to avoid silent no-op.
                if export.path.is_some() {
                    self.errors.push(TypeError {
                        message: "Re-export path `<<< ./path` is not yet supported. \
                             Use explicit import and re-export: `>>> ./path.td => @(sym)` then `<<< @(sym)`."
                            .to_string(),
                        span: export.span.clone(),
                    });
                }
            }
            // N-65: Intentional catch-all — TypeDef, MoldDef, and InheritanceDef
            // are registered in the first pass of check_program(). Additional
            // statement kinds (e.g., future AST variants) will need explicit arms
            // added here when introduced.
            _ => {}
        }
    }

    const MAX_BIDI_TYPE_HINT_DEPTH: usize = 32;

    pub(super) fn infer_expr_type_with_expected(&mut self, expr: &Expr, expected: &Type) -> Type {
        self.infer_expr_type_with_expected_inner(expr, expected, FunctionHintDiagnostic::MethodArg)
    }

    fn fill_unknowns_from_expected(inferred: &Type, expected: &Type) -> Type {
        Self::fill_unknowns_from_expected_at_depth(inferred, expected, 0)
    }

    fn fill_unknowns_from_expected_at_depth(
        inferred: &Type,
        expected: &Type,
        depth: usize,
    ) -> Type {
        if depth >= Self::MAX_BIDI_TYPE_HINT_DEPTH {
            return inferred.clone();
        }
        match (inferred, expected) {
            (
                Type::Generic(inferred_name, inferred_args),
                Type::Generic(expected_name, expected_args),
            ) if inferred_name == expected_name && inferred_args.len() == expected_args.len() => {
                Type::Generic(
                    inferred_name.clone(),
                    inferred_args
                        .iter()
                        .zip(expected_args.iter())
                        .map(|(actual, expected)| {
                            if matches!(actual, Type::Unknown) {
                                expected.clone()
                            } else {
                                Self::fill_unknowns_from_expected_at_depth(
                                    actual,
                                    expected,
                                    depth + 1,
                                )
                            }
                        })
                        .collect(),
                )
            }
            (Type::List(inferred_inner), Type::List(expected_inner)) => Type::List(Box::new(
                if matches!(inferred_inner.as_ref(), Type::Unknown) {
                    expected_inner.as_ref().clone()
                } else {
                    Self::fill_unknowns_from_expected_at_depth(
                        inferred_inner,
                        expected_inner,
                        depth + 1,
                    )
                },
            )),
            (Type::BuchiPack(inferred_fields), Type::BuchiPack(expected_fields)) => {
                Type::BuchiPack(
                    inferred_fields
                        .iter()
                        .map(|(field_name, inferred_ty)| {
                            let hinted_ty = expected_fields
                                .iter()
                                .find(|(expected_name, _)| expected_name == field_name)
                                .map(|(_, expected_ty)| {
                                    if matches!(inferred_ty, Type::Unknown) {
                                        expected_ty.clone()
                                    } else {
                                        Self::fill_unknowns_from_expected_at_depth(
                                            inferred_ty,
                                            expected_ty,
                                            depth + 1,
                                        )
                                    }
                                })
                                .unwrap_or_else(|| inferred_ty.clone());
                            (field_name.clone(), hinted_ty)
                        })
                        .collect(),
                )
            }
            (
                Type::Function(inferred_params, inferred_ret),
                Type::Function(expected_params, expected_ret),
            ) if inferred_params.len() == expected_params.len() => Type::Function(
                // This is hint filling, not subtype validation. Function
                // boundary variance is checked later by is_function_arg_subtype_of.
                inferred_params
                    .iter()
                    .zip(expected_params.iter())
                    .map(|(actual, expected)| {
                        if matches!(actual, Type::Unknown) {
                            expected.clone()
                        } else {
                            Self::fill_unknowns_from_expected_at_depth(actual, expected, depth + 1)
                        }
                    })
                    .collect(),
                Box::new(if matches!(inferred_ret.as_ref(), Type::Unknown) {
                    expected_ret.as_ref().clone()
                } else {
                    Self::fill_unknowns_from_expected_at_depth(
                        inferred_ret,
                        expected_ret,
                        depth + 1,
                    )
                }),
            ),
            _ => inferred.clone(),
        }
    }

    fn generic_expected_hint(
        &self,
        pattern: &Type,
        generic_names: &HashSet<String>,
        bindings: &HashMap<String, Type>,
    ) -> Type {
        let substituted = self.substitute_generic_type(pattern, generic_names, bindings);
        Self::erase_unbound_generic_names(&substituted, generic_names)
    }

    fn erase_unbound_generic_names(ty: &Type, generic_names: &HashSet<String>) -> Type {
        Self::erase_unbound_generic_names_at_depth(ty, generic_names, 0)
    }

    fn erase_unbound_generic_names_at_depth(
        ty: &Type,
        generic_names: &HashSet<String>,
        depth: usize,
    ) -> Type {
        if depth >= Self::MAX_BIDI_TYPE_HINT_DEPTH {
            return ty.clone();
        }
        match ty {
            Type::Named(name) if generic_names.contains(name) => Type::Unknown,
            Type::List(inner) => Type::List(Box::new(Self::erase_unbound_generic_names_at_depth(
                inner,
                generic_names,
                depth + 1,
            ))),
            Type::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| {
                        Self::erase_unbound_generic_names_at_depth(arg, generic_names, depth + 1)
                    })
                    .collect(),
            ),
            Type::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|(name, ty)| {
                        (
                            name.clone(),
                            Self::erase_unbound_generic_names_at_depth(
                                ty,
                                generic_names,
                                depth + 1,
                            ),
                        )
                    })
                    .collect(),
            ),
            Type::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| {
                        Self::erase_unbound_generic_names_at_depth(param, generic_names, depth + 1)
                    })
                    .collect(),
                Box::new(Self::erase_unbound_generic_names_at_depth(
                    ret,
                    generic_names,
                    depth + 1,
                )),
            ),
            _ => ty.clone(),
        }
    }

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

    fn push_worker_error(&mut self, code: &str, span: &Span, message: String) {
        if self
            .errors
            .iter()
            .any(|err| err.span == *span && err.message.contains(code))
        {
            return;
        }
        self.errors.push(TypeError {
            message,
            span: span.clone(),
        });
    }

    fn validate_async_task_worker_body(&mut self, task_arg: &Expr) {
        match task_arg {
            Expr::Lambda(params, body, _) => {
                let mut local_names = HashSet::new();
                let mut function_stack = HashSet::new();
                self.push_scope();
                for param in params {
                    if let Some(default_value) = &param.default_value {
                        self.validate_worker_expr(
                            default_value,
                            &mut local_names,
                            &mut function_stack,
                        );
                    }
                    let ty = param
                        .type_annotation
                        .as_ref()
                        .map(|ann| self.registry.resolve_type(ann))
                        .unwrap_or(Type::Unknown);
                    self.define_var_silent(&param.name, ty);
                    local_names.insert(param.name.clone());
                }
                self.validate_worker_expr(body, &mut local_names, &mut function_stack);
                self.pop_scope();
            }
            Expr::Ident(name, span) => {
                let mut function_stack = HashSet::new();
                let local_names = HashSet::new();
                self.validate_worker_call_name(name, span, &local_names, &mut function_stack);
            }
            other => {
                let mut local_names = HashSet::new();
                let mut function_stack = HashSet::new();
                self.validate_worker_expr(other, &mut local_names, &mut function_stack);
                self.push_worker_error(
                    "[E1624]",
                    other.span(),
                    "[E1624] CPU worker body must be a lambda literal or a visible Taida function. \
                     Hint: write `AsyncTask[_ = expr]()` or pass a direct mapper lambda to `ParMap` so the worker body is explicit."
                        .to_string(),
                );
            }
        }
    }

    fn validate_worker_user_function(
        &mut self,
        name: &str,
        span: &Span,
        function_stack: &mut HashSet<String>,
    ) {
        if !function_stack.insert(name.to_string()) {
            return;
        }

        let Some(fd) = self
            .func_defs
            .get(name)
            .or_else(|| self.generic_func_defs.get(name))
            .cloned()
        else {
            self.push_worker_error(
                "[E1624]",
                span,
                format!(
                    "[E1624] CPU worker body cannot call opaque function value '{}'. \
                     Hint: call a Taida function whose body is visible to the checker, or inline a local lambda inside the task.",
                    name
                ),
            );
            function_stack.remove(name);
            return;
        };

        let param_types = self.func_param_types.get(name).cloned().unwrap_or_else(|| {
            fd.params
                .iter()
                .map(|param| {
                    param
                        .type_annotation
                        .as_ref()
                        .map(|ann| self.registry.resolve_type(ann))
                        .unwrap_or(Type::Unknown)
                })
                .collect()
        });

        let mut local_names = HashSet::new();
        self.push_scope();
        for (idx, param) in fd.params.iter().enumerate() {
            if let Some(default_value) = &param.default_value {
                self.validate_worker_expr(default_value, &mut local_names, function_stack);
            }
            self.define_var_silent(
                &param.name,
                param_types.get(idx).cloned().unwrap_or(Type::Unknown),
            );
            local_names.insert(param.name.clone());
        }

        for stmt in &fd.body {
            self.validate_worker_stmt(stmt, &mut local_names, function_stack);
        }
        self.pop_scope();
        function_stack.remove(name);
    }

    fn validate_worker_stmt(
        &mut self,
        stmt: &Statement,
        local_names: &mut HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        match stmt {
            Statement::Assignment(assign) => {
                self.validate_worker_expr(&assign.value, local_names, function_stack);
                let ty = self
                    .typed_expr_table
                    .lookup(&assign.value)
                    .cloned()
                    .unwrap_or(Type::Unknown);
                self.define_var_silent(&assign.target, ty);
                local_names.insert(assign.target.clone());
            }
            Statement::Expr(expr) => self.validate_worker_expr(expr, local_names, function_stack),
            Statement::ErrorCeiling(ec) => {
                let mut handler_locals = local_names.clone();
                self.push_scope();
                let err_ty = self.registry.resolve_type(&ec.error_type);
                self.define_var_silent(&ec.error_param, err_ty);
                handler_locals.insert(ec.error_param.clone());
                for stmt in &ec.handler_body {
                    self.validate_worker_stmt(stmt, &mut handler_locals, function_stack);
                }
                self.pop_scope();
            }
            Statement::UnmoldForward(stmt) => {
                self.validate_worker_expr(&stmt.source, local_names, function_stack);
                let source_ty = self
                    .typed_expr_table
                    .lookup(&stmt.source)
                    .cloned()
                    .unwrap_or(Type::Unknown);
                self.define_var_silent(&stmt.target, self.unmold_type(&source_ty));
                local_names.insert(stmt.target.clone());
            }
            Statement::UnmoldBackward(stmt) => {
                self.validate_worker_expr(&stmt.source, local_names, function_stack);
                let source_ty = self
                    .typed_expr_table
                    .lookup(&stmt.source)
                    .cloned()
                    .unwrap_or(Type::Unknown);
                self.define_var_silent(&stmt.target, self.unmold_type(&source_ty));
                local_names.insert(stmt.target.clone());
            }
            Statement::FuncDef(fd) => {
                self.validate_worker_inline_function_def(fd, local_names, function_stack);
            }
            Statement::ClassLikeDef(_)
            | Statement::EnumDef(_)
            | Statement::Import(_)
            | Statement::Export(_) => {}
        }
    }

    fn validate_worker_expr(
        &mut self,
        expr: &Expr,
        local_names: &mut HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name, span) => self.validate_worker_ident(name, span, local_names),
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for field in fields {
                    self.validate_worker_expr(&field.value, local_names, function_stack);
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    self.validate_worker_expr(item, local_names, function_stack);
                }
            }
            Expr::Pipeline(items, _) => {
                let last_idx = items.len().saturating_sub(1);
                let mut pipeline_locals = local_names.clone();
                self.push_scope();
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0
                        && idx < last_idx
                        && let Expr::Ident(name, _) = item
                        && !self.is_pipeline_callable_ident(name)
                    {
                        pipeline_locals.insert(name.clone());
                        continue;
                    }
                    self.validate_worker_expr(item, &mut pipeline_locals, function_stack);
                }
                self.pop_scope();
            }
            Expr::BinaryOp(left, _, right, _) => {
                self.validate_worker_expr(left, local_names, function_stack);
                self.validate_worker_expr(right, local_names, function_stack);
            }
            Expr::UnaryOp(_, inner, _)
            | Expr::FieldAccess(inner, _, _)
            | Expr::Unmold(inner, _)
            | Expr::Throw(inner, _) => {
                self.validate_worker_expr(inner, local_names, function_stack);
            }
            Expr::FuncCall(callee, args, span) => {
                for arg in args {
                    self.validate_worker_expr(arg, local_names, function_stack);
                }
                match callee.as_ref() {
                    Expr::Ident(name, callee_span) => self.validate_worker_call_name(
                        name,
                        callee_span,
                        local_names,
                        function_stack,
                    ),
                    Expr::Lambda(params, body, _) => {
                        self.validate_worker_lambda(params, body, local_names, function_stack);
                    }
                    other => {
                        self.validate_worker_expr(other, local_names, function_stack);
                        self.push_worker_error(
                            "[E1624]",
                            span,
                            "[E1624] CPU worker body cannot call a computed function value. \
                             Hint: use a direct Taida function call or a lambda literal inside the task."
                                .to_string(),
                        );
                    }
                }
            }
            Expr::MethodCall(receiver, _, args, _) => {
                self.validate_worker_expr(receiver, local_names, function_stack);
                for arg in args {
                    self.validate_worker_expr(arg, local_names, function_stack);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(condition) = &arm.condition {
                        self.validate_worker_expr(condition, local_names, function_stack);
                    }
                    let mut arm_locals = local_names.clone();
                    self.push_scope();
                    for stmt in &arm.body {
                        self.validate_worker_stmt(stmt, &mut arm_locals, function_stack);
                    }
                    self.pop_scope();
                }
            }
            Expr::MoldInst(name, type_args, fields, span) => {
                let value_arg_count = Self::worker_mold_value_arg_count(name, type_args.len());
                for arg in type_args.iter().take(value_arg_count) {
                    self.validate_worker_expr(arg, local_names, function_stack);
                }
                for field in fields {
                    self.validate_worker_expr(&field.value, local_names, function_stack);
                }
                self.validate_worker_mold_name(name, span);
            }
            Expr::Lambda(params, body, _) => {
                self.validate_worker_lambda(params, body, local_names, function_stack);
            }
            Expr::IntLit(_, _)
            | Expr::FloatLit(_, _)
            | Expr::StringLit(_, _)
            | Expr::TemplateLit(_, _)
            | Expr::BoolLit(_, _)
            | Expr::Gorilla(_)
            | Expr::Placeholder(_)
            | Expr::Hole(_)
            | Expr::EnumVariant(_, _, _)
            | Expr::TypeLiteral(_, _, _) => {}
        }
    }

    fn worker_mold_value_arg_count(name: &str, arg_count: usize) -> usize {
        match name {
            "JSGet" if arg_count == 2 => 1,
            "JSCall" | "JSCallAsync" if arg_count == 3 => 2,
            "JSNew" if arg_count == 3 => 2,
            _ => arg_count,
        }
    }

    fn validate_worker_lambda(
        &mut self,
        params: &[Param],
        body: &Expr,
        local_names: &mut HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        let mut nested_locals = local_names.clone();
        self.push_scope();
        for param in params {
            if let Some(default_value) = &param.default_value {
                self.validate_worker_expr(default_value, &mut nested_locals, function_stack);
            }
            let ty = param
                .type_annotation
                .as_ref()
                .map(|ann| self.registry.resolve_type(ann))
                .unwrap_or(Type::Unknown);
            self.define_var_silent(&param.name, ty);
            nested_locals.insert(param.name.clone());
        }
        self.validate_worker_expr(body, &mut nested_locals, function_stack);
        self.pop_scope();
    }

    fn validate_worker_inline_function_def(
        &mut self,
        fd: &FuncDef,
        local_names: &mut HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        let param_types: Vec<Type> = fd
            .params
            .iter()
            .map(|param| {
                param
                    .type_annotation
                    .as_ref()
                    .map(|ann| self.registry.resolve_type(ann))
                    .unwrap_or(Type::Unknown)
            })
            .collect();
        let ret_ty = fd
            .return_type
            .as_ref()
            .map(|ann| self.registry.resolve_type(ann))
            .unwrap_or(Type::Unknown);

        let mut nested_locals = local_names.clone();
        self.push_scope();
        for (idx, param) in fd.params.iter().enumerate() {
            if let Some(default_value) = &param.default_value {
                self.validate_worker_expr(default_value, &mut nested_locals, function_stack);
            }
            self.define_var_silent(
                &param.name,
                param_types.get(idx).cloned().unwrap_or(Type::Unknown),
            );
            nested_locals.insert(param.name.clone());
        }
        for stmt in &fd.body {
            self.validate_worker_stmt(stmt, &mut nested_locals, function_stack);
        }
        self.pop_scope();

        self.define_var_silent(&fd.name, Type::Function(param_types, Box::new(ret_ty)));
        local_names.insert(fd.name.clone());
    }

    fn validate_worker_call_name(
        &mut self,
        name: &str,
        span: &Span,
        local_names: &HashSet<String>,
        function_stack: &mut HashSet<String>,
    ) {
        if local_names.contains(name) {
            return;
        }
        if self.is_worker_effect_symbol(name) {
            self.push_worker_error(
                "[E1620]",
                span,
                format!(
                    "[E1620] CPU worker body cannot call effectful API '{}'. \
                     Hint: perform I/O before creating the task or after `Par[jobs]()` completes.",
                    name
                ),
            );
            return;
        }
        if let Some(binding) = self.worker_addon_bindings.get(name).cloned() {
            match binding.decision {
                WorkerAddonDecision::Allow => {}
                WorkerAddonDecision::Deny {
                    code,
                    reason,
                    active_policy,
                    effective_claim,
                } => {
                    self.push_worker_error(
                        code,
                        span,
                        format!(
                            "{} CPU worker body cannot call addon function '{}::{}'. \
                             Effective claim: {}; active policy: {}. {}. \
                             Hint: add function purity metadata and project policy, or move the addon call outside the worker task.",
                            code,
                            binding.package_id,
                            binding.function_name,
                            effective_claim,
                            active_policy,
                            reason
                        ),
                    );
                }
            }
            return;
        }
        if self.worker_addon_symbols.contains(name) {
            self.push_worker_error(
                "[E1621]",
                span,
                format!(
                    "[E1621] CPU worker body cannot cross addon or host boundary '{}'. \
                     Hint: move addon and host interop calls outside the worker task.",
                    name
                ),
            );
            return;
        }
        if self.func_defs.contains_key(name) || self.generic_func_defs.contains_key(name) {
            self.validate_worker_user_function(name, span, function_stack);
            return;
        }
        if Self::is_core_builtin_name(name) {
            return;
        }
        if matches!(self.lookup_var(name), Some(Type::Function(_, _)))
            || self.func_types.contains_key(name)
        {
            self.push_worker_error(
                "[E1624]",
                span,
                format!(
                    "[E1624] CPU worker body cannot call captured function value '{}'. \
                     Hint: call a visible Taida function directly or inline a local lambda inside the task.",
                    name
                ),
            );
            return;
        }
        if matches!(self.lookup_var(name), Some(Type::Unknown | Type::Any)) {
            self.push_worker_error(
                "[E1626]",
                span,
                format!(
                    "[E1626] CPU worker body calls '{}' before its type is fully known. \
                     Hint: add a concrete annotation or use a visible Taida function.",
                    name
                ),
            );
            return;
        }
        if let Some(ty) = self.lookup_var(name)
            && !self.is_worker_safe_type(&ty)
        {
            self.push_worker_error(
                "[E1623]",
                span,
                format!(
                    "[E1623] CPU worker body cannot call '{}' with non-transferable type {}. \
                     Hint: call visible Taida functions directly and keep host values outside the worker task.",
                    name, ty
                ),
            );
        }
    }

    fn validate_worker_ident(&mut self, name: &str, span: &Span, local_names: &HashSet<String>) {
        if local_names.contains(name) {
            return;
        }
        if self.is_worker_effect_symbol(name) {
            self.push_worker_error(
                "[E1620]",
                span,
                format!(
                    "[E1620] CPU worker body cannot capture effectful API '{}'. \
                     Hint: perform I/O before creating the task or after `Par[jobs]()` completes.",
                    name
                ),
            );
            return;
        }
        if self.worker_addon_bindings.contains_key(name) {
            self.push_worker_error(
                "[E1621]",
                span,
                format!(
                    "[E1621] CPU worker body cannot capture addon or host boundary '{}'. \
                     Hint: call allowed pure addon functions directly inside the worker task; do not capture them as values.",
                    name
                ),
            );
            return;
        }
        if self.worker_addon_symbols.contains(name) {
            self.push_worker_error(
                "[E1621]",
                span,
                format!(
                    "[E1621] CPU worker body cannot capture addon or host boundary '{}'. \
                     Hint: move addon and host interop values outside the worker task.",
                    name
                ),
            );
            return;
        }
        if self.func_defs.contains_key(name)
            || self.generic_func_defs.contains_key(name)
            || self.func_types.contains_key(name)
        {
            self.push_worker_error(
                "[E1624]",
                span,
                format!(
                    "[E1624] CPU worker body cannot capture function value '{}'. \
                     Hint: call a visible Taida function directly or inline a local lambda inside the task.",
                    name
                ),
            );
            return;
        }
        let Some(ty) = self.lookup_var(name) else {
            self.push_worker_error(
                "[E1626]",
                span,
                format!(
                    "[E1626] CPU worker body captures '{}' before its type is known. \
                     Hint: define the value before creating the task and give it a concrete type.",
                    name
                ),
            );
            return;
        };
        if matches!(ty, Type::Unknown | Type::Any) {
            self.push_worker_error(
                "[E1626]",
                span,
                format!(
                    "[E1626] CPU worker body captures '{}' with unresolved type {}. \
                     Hint: add a concrete annotation before creating the task.",
                    name, ty
                ),
            );
            return;
        }
        if matches!(ty, Type::Function(_, _)) {
            self.push_worker_error(
                "[E1624]",
                span,
                format!(
                    "[E1624] CPU worker body cannot capture function value '{}'. \
                     Hint: call a visible Taida function directly or inline a local lambda inside the task.",
                    name
                ),
            );
            return;
        }
        if !self.is_worker_safe_type(&ty) {
            self.push_worker_error(
                "[E1623]",
                span,
                format!(
                    "[E1623] CPU worker body captures '{}' with non-transferable type {}. \
                     Hint: capture primitives, lists, and structurally safe buchi packs only.",
                    name, ty
                ),
            );
        }
    }

    fn validate_worker_mold_name(&mut self, name: &str, span: &Span) {
        if Self::is_worker_effect_mold(name) {
            self.push_worker_error(
                "[E1620]",
                span,
                format!(
                    "[E1620] CPU worker body cannot call effectful mold '{}'. \
                     Hint: perform file, environment, or network access outside the worker task.",
                    name
                ),
            );
        } else if Self::is_worker_host_boundary_mold(name) {
            self.push_worker_error(
                "[E1621]",
                span,
                format!(
                    "[E1621] CPU worker body cannot cross addon or host boundary '{}'. \
                     Hint: move addon and host interop calls outside the worker task.",
                    name
                ),
            );
        } else if Self::is_worker_nested_async_mold(name) {
            self.push_worker_error(
                "[E1622]",
                span,
                format!(
                    "[E1622] CPU worker body cannot create nested async or parallel value '{}'. \
                     Hint: build parallel tasks at the outer level and keep each task body synchronous.",
                    name
                ),
            );
        }
    }

    fn is_worker_effect_symbol(&self, name: &str) -> bool {
        self.worker_effect_symbols.contains(name) || Self::is_worker_effect_builtin(name)
    }

    fn is_worker_effect_builtin(name: &str) -> bool {
        matches!(
            name,
            "debug"
                | "nowMs"
                | "stdout"
                | "stderr"
                | "exit"
                | "stdin"
                | "stdinLine"
                | "argv"
                | "sleep"
                | "readBytes"
                | "readBytesAt"
                | "writeFile"
                | "writeBytes"
                | "appendFile"
                | "remove"
                | "createDir"
                | "rename"
                | "allEnv"
                | "dnsResolve"
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
                | "poolHealth"
                | "run"
                | "execShell"
                | "runInteractive"
                | "execShellInteractive"
        )
    }

    fn is_worker_effect_mold(name: &str) -> bool {
        use crate::types::mold_specs::{WorkerMoldBoundary, lookup_worker_mold_boundary};

        lookup_worker_mold_boundary(name) == WorkerMoldBoundary::Effectful
    }

    fn is_worker_host_boundary_mold(name: &str) -> bool {
        use crate::types::mold_specs::{WorkerMoldBoundary, lookup_worker_mold_boundary};

        name == "RustAddon" || lookup_worker_mold_boundary(name) == WorkerMoldBoundary::HostBoundary
    }

    fn is_worker_nested_async_mold(name: &str) -> bool {
        use crate::types::mold_specs::{WorkerMoldBoundary, lookup_worker_mold_boundary};

        lookup_worker_mold_boundary(name) == WorkerMoldBoundary::NestedAsync
    }

    fn is_worker_safe_type_inner(&self, ty: &Type, seen_named: &mut HashSet<String>) -> bool {
        match ty {
            Type::Int | Type::Float | Type::Num | Type::Str | Type::Bytes | Type::Bool => true,
            Type::BuchiPack(fields) => fields
                .iter()
                .all(|(_, field_ty)| self.is_worker_safe_type_inner(field_ty, seen_named)),
            Type::List(inner) => self.is_worker_safe_type_inner(inner, seen_named),
            Type::Named(name) => {
                if self.registry.is_enum_type(name) {
                    return true;
                }
                if !seen_named.insert(name.clone()) {
                    return true;
                }
                let safe = self.registry.get_type_fields(name).is_some_and(|fields| {
                    fields
                        .iter()
                        .all(|(_, field_ty)| self.is_worker_safe_type_inner(field_ty, seen_named))
                });
                seen_named.remove(name);
                safe
            }
            Type::Generic(name, args) => {
                use crate::types::mold_specs::{WorkerSafety, lookup_worker_safety};
                match lookup_worker_safety(name) {
                    WorkerSafety::Pure => true,
                    WorkerSafety::Transparent => args
                        .iter()
                        .all(|arg| self.is_worker_safe_type_inner(arg, seen_named)),
                    WorkerSafety::Unsafe => self.is_worker_safe_user_mold(name, args, seen_named),
                }
            }
            Type::Error(name) => {
                if !seen_named.insert(name.clone()) {
                    return true;
                }
                let safe = self.registry.get_type_fields(name).is_some_and(|fields| {
                    fields
                        .iter()
                        .all(|(_, field_ty)| self.is_worker_safe_type_inner(field_ty, seen_named))
                });
                seen_named.remove(name);
                safe
            }
            Type::Function(_, _)
            | Type::Unit
            | Type::Unknown
            | Type::Any
            | Type::Json
            | Type::Molten => false,
        }
    }

    fn is_worker_safe_user_mold(
        &self,
        name: &str,
        args: &[Type],
        seen_named: &mut HashSet<String>,
    ) -> bool {
        let Some((type_params, fields)) = self.registry.mold_defs.get(name) else {
            return false;
        };
        let key = format!(
            "{}[{}]",
            name,
            args.iter()
                .map(|arg| arg.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !seen_named.insert(key.clone()) {
            return true;
        }
        let bindings: HashMap<String, Type> = type_params
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        let safe = fields.iter().all(|(_, field_ty)| {
            let resolved = Self::substitute_worker_type_params(field_ty, &bindings);
            self.is_worker_safe_type_inner(&resolved, seen_named)
        });
        seen_named.remove(&key);
        safe
    }

    fn substitute_worker_type_params(ty: &Type, bindings: &HashMap<String, Type>) -> Type {
        match ty {
            Type::Named(name) => bindings.get(name).cloned().unwrap_or_else(|| ty.clone()),
            Type::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|(name, field_ty)| {
                        (
                            name.clone(),
                            Self::substitute_worker_type_params(field_ty, bindings),
                        )
                    })
                    .collect(),
            ),
            Type::List(inner) => Type::List(Box::new(Self::substitute_worker_type_params(
                inner, bindings,
            ))),
            Type::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| Self::substitute_worker_type_params(param, bindings))
                    .collect(),
                Box::new(Self::substitute_worker_type_params(ret, bindings)),
            ),
            Type::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| Self::substitute_worker_type_params(arg, bindings))
                    .collect(),
            ),
            _ => ty.clone(),
        }
    }

    /// Check a condition branch expression (extracted from `infer_expr_type`).
    ///
    /// Validates that:
    /// - All arm conditions are Bool (E1604)
    /// - All arms return compatible types (E1603)
    fn check_cond_branch(&mut self, arms: &[CondArm], span: &Span) -> Type {
        // FL-3: Check all arms' types, not just the first
        if arms.is_empty() {
            return Type::Unknown;
        }

        // F42 sweep [E1524]: a condition branch must have a default arm
        // — either `| _ |>` (condition is `None`) or `| true |>`
        // (literal-true). Otherwise, runtime behavior is undefined when
        // every condition arm fails. PHILOSOPHY IV — strict structure
        // for AI readability.
        let has_default = arms.iter().any(|arm| {
            arm.condition.is_none() || matches!(&arm.condition, Some(Expr::BoolLit(true, _)))
        });
        if !has_default {
            self.errors.push(TypeError {
                message: "[E1524] Condition branch is missing a default arm. \
                          Add `| _ |>` or `| true |>` so the result is defined \
                          for every input (PHILOSOPHY IV — strict structure). \
                          See docs/reference/diagnostic_codes.md [E1524]."
                    .into(),
                span: span.clone(),
            });
        }
        let mut result_ty = Type::Unknown;

        for arm in arms {
            // Check condition type
            if let Some(cond) = &arm.condition {
                let cond_ty = self.infer_expr_type(cond);
                if cond_ty != Type::Bool
                    && cond_ty != Type::Unknown
                    && !Self::contains_unknown(&cond_ty)
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1604] Condition in branch must be Bool, got {}. \
                             Hint: Use a boolean expression as the condition.",
                            cond_ty
                        ),
                        span: arm.span.clone(),
                    });
                }
            }
            // Each arm gets its own scope
            self.push_scope();
            for body_stmt in &arm.body {
                self.check_statement(body_stmt);
            }
            let arm_ty = self.arm_result_type(arm);
            if arm_ty != Type::Unknown && !Self::contains_unknown(&arm_ty) {
                if result_ty == Type::Unknown || Self::contains_unknown(&result_ty) {
                    result_ty = arm_ty;
                } else if !(self.registry.is_subtype_of(&arm_ty, &result_ty)
                    || result_ty.is_numeric() && arm_ty.is_numeric())
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1603] Condition branch type mismatch: first resolved arm returns {}, but this arm returns {}. \
                             Hint: All value-returning arms of a condition branch should return the same type.",
                            result_ty, arm_ty
                        ),
                        span: span.clone(),
                    });
                }
            }
            self.pop_scope();
        }

        result_ty
    }
}

mod arity;
mod checker_methods;
mod descriptor;
mod infer;
mod resolve;
mod validate;

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

#[cfg(test)]
mod tests;

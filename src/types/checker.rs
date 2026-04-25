use super::types::{Type, TypeRegistry};
use crate::lexer::Span;
use crate::net_surface::{NET_HTTP_PROTOCOL_VARIANTS, is_net_export_name, net_export_list};
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
/// ## Type inference convention: `unwrap_or(Type::Unknown)` (N-67)
///
/// Throughout this module, `.unwrap_or(Type::Unknown)` is the standard fallback
/// when type information is unavailable (e.g., unresolved generics, missing
/// function parameter types, cross-module imports). `Type::Unknown` is **not**
/// an error type -- it is a valid propagation signal meaning "the checker cannot
/// determine this type statically." Unknown propagates silently through
/// expressions, and downstream checks skip validation when either operand is
/// Unknown. This prevents cascading false-positive errors while still catching
/// errors where types are fully known.
use std::collections::{HashMap, HashSet};

/// C12B-023 bypass closure (2026-04-15, root fix): field names reserved
/// for compiler-internal use. A user-authored `Expr::BuchiPack` /
/// `Expr::TypeInst` literal that assigns any of these is rejected at
/// type-check time with `[E1617]`.
///
/// Rationale: compiler-generated packs set `__type`, `__value`,
/// `__default`, `__error`, `__tag`, `__items`, `__transforms`,
/// `__status` as *internal* tags to carry nominal-type identity and
/// invariants (e.g., `Regex` packs carry a validated `pattern` /
/// `flags` pair, `Lax` packs carry `hasValue` + default, `Async` packs
/// carry a state tag). Allowing user code to set these fields lets
/// callers fabricate fake nominal packs that bypass the official
/// constructors' validation. The earlier narrower fix (literal
/// `__type <= "Regex"` only) was bypassed by variable binding
/// (`tag <= "Regex"; @(__type <= tag, ...)`) and by expression
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
/// NOTE: field **reads** (`value.__type`) remain allowed for
/// introspection (see `examples/quality/rc6a_error_inheritance.td`);
/// only write-side assignments in pack literals are rejected.
const RESERVED_INTERNAL_FIELD_PREFIX: &str = "__";

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
/// - `E1618` -- enum variant order mismatch across module boundary (C18-1)
/// - `E1611` -- JS backend capability rejection
/// - `E1612` -- WASM backend capability rejection
/// - `E1613` -- TypeExtends does not accept enum variant literals
/// - `E1617` -- Regex invariant rejection. Two emitters share this code (both C12B-023):
///   (1) WASM backend Regex rejection (`emit_wasm_c::validate_regex_api_for_wasm`) —
///   `Regex(...)` ctor / `.match(re)` / `.search(re)` are unsupported on wasm;
///   (2) Manual `__type <= "Regex"` BuchiPack construction rejection
///   (`checker::check_mold_errors_in_expr`) — nominal `:Regex` must be produced
///   by its official constructor to enforce eager pattern validation.
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
    /// Source file path — used for resolving import paths to validate export symbols.
    source_file: Option<std::path::PathBuf>,
    /// Compile target for backend-aware diagnostics.
    compile_target: CompileTarget,
    /// Local names that resolve to taida-lang/net's `httpServe`.
    net_http_serve_symbols: HashSet<String>,
    /// Local enum names that resolve to taida-lang/net's `HttpProtocol`.
    net_http_protocol_type_names: HashSet<String>,
    /// D28B-023 / D28B-024: stack of type parameter declarations for the
    /// enclosing generic functions. Pushed on `Statement::FuncDef` body
    /// entry, popped on exit. Used to resolve constrained type variables
    /// inside the body (e.g. arithmetic on `T <= :Num`, calling `F <= :T => :T`).
    current_func_type_params: Vec<Vec<TypeParam>>,
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
    fn is_js(self) -> bool {
        matches!(self, Self::Js)
    }

    fn is_wasm(self) -> bool {
        matches!(
            self,
            Self::WasmMin | Self::WasmWasi | Self::WasmEdge | Self::WasmFull
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

impl TypeChecker {
    pub fn new() -> Self {
        let mut checker = Self {
            registry: TypeRegistry::new(),
            errors: Vec::new(),
            scope_stack: vec![HashMap::new()], // global scope
            func_types: HashMap::new(),
            func_param_counts: HashMap::new(),
            func_param_types: HashMap::new(),
            generic_func_defs: HashMap::new(),
            invalid_func_defs: HashSet::new(),
            seen_func_defs: HashSet::new(),
            declared_concrete_type_names: HashSet::new(),
            mold_field_defs: HashMap::new(),
            mold_header_specs: HashMap::new(),
            declared_header_arities: HashMap::new(),
            in_pipeline: false,
            source_file: None,
            compile_target: CompileTarget::Neutral,
            net_http_serve_symbols: HashSet::new(),
            net_http_protocol_type_names: HashSet::new(),
            current_func_type_params: Vec::new(),
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

    /// C19B-002: install pinned signatures for the C19 interactive os
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

    /// C19B-002: register typed signatures for `taida-lang/os` symbols that
    /// need compile-time Gorillax inner-shape pinning.
    ///
    /// Currently only the C19 interactive variants are pinned, because
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

    fn effective_mold_header_args(md: &MoldDef) -> Vec<MoldHeaderArg> {
        md.name_args
            .as_ref()
            .cloned()
            .unwrap_or_else(|| md.mold_args.clone())
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

    fn validate_mold_root_header(&mut self, md: &MoldDef, header_args: &[MoldHeaderArg]) {
        if md.mold_args.len() != 1 {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "MoldDef '{}' must keep the built-in parent `Mold` header at arity 1, got {}",
                        md.name,
                        md.mold_args.len()
                    ),
                    "Write `Mold[T] => Child[T, U, ...] = @(...)`; extend header slots on the child side, not on `Mold` itself.",
                ),
                span: md.span.clone(),
            });
        }

        self.validate_child_header_prefix(
            "MoldDef",
            &md.name,
            "Mold",
            &md.mold_args,
            header_args,
            &md.span,
        );
        self.validate_unique_mold_type_param_names("MoldDef", &md.name, header_args, &md.span);
    }

    fn validate_child_header_prefix(
        &mut self,
        kind: &str,
        child_name: &str,
        parent_name: &str,
        parent_args: &[MoldHeaderArg],
        child_args: &[MoldHeaderArg],
        span: &Span,
    ) {
        if child_args.len() < parent_args.len() {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "{} '{}' cannot shrink header arity below parent '{}' (child: {}, parent: {})",
                        kind,
                        child_name,
                        parent_name,
                        child_args.len(),
                        parent_args.len()
                    ),
                    "Keep inherited header slots intact and append any new slots on the child side.",
                ),
                span: span.clone(),
            });
            return;
        }

        for (idx, parent_arg) in parent_args.iter().enumerate() {
            if child_args.get(idx) != Some(parent_arg) {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1407",
                        format!(
                            "{} '{}' must preserve inherited header slot {} from '{}' exactly; expected {}, got {}",
                            kind,
                            child_name,
                            idx + 1,
                            parent_name,
                            Self::header_arg_label(parent_arg),
                            child_args
                                .get(idx)
                                .map(Self::header_arg_label)
                                .unwrap_or_else(|| "<missing>".to_string())
                        ),
                        "Keep inherited header slots as an exact prefix and append new slots only after the parent header.",
                    ),
                    span: span.clone(),
                });
            }
        }
    }

    fn validate_unique_mold_type_param_names(
        &mut self,
        kind: &str,
        name: &str,
        header_args: &[MoldHeaderArg],
        span: &Span,
    ) {
        let mut seen = HashSet::<String>::new();
        let mut duplicates = Vec::<String>::new();
        for arg in header_args {
            if let MoldHeaderArg::TypeParam(tp) = arg
                && !seen.insert(tp.name.clone())
                && !duplicates.contains(&tp.name)
            {
                duplicates.push(tp.name.clone());
            }
        }

        if !duplicates.is_empty() {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "{} '{}' reuses header type parameter name(s): {}",
                        kind,
                        name,
                        duplicates.join(", ")
                    ),
                    "Use each header type parameter name at most once; append new child slots with distinct names.",
                ),
                span: span.clone(),
            });
        }
    }

    fn validate_mold_extension_bindings(
        &mut self,
        def: MoldBindingDef<'_>,
        parent_arity: usize,
        header_args: &[MoldHeaderArg],
        fields: &[FieldDef],
        inherited_field_names: &HashSet<String>,
    ) {
        let positional_field_count = fields
            .iter()
            .filter(|f| {
                !f.is_method
                    && f.default_value.is_none()
                    && f.name != "filling"
                    && !inherited_field_names.contains(&f.name)
            })
            .count();

        let extra_args = header_args.len().saturating_sub(parent_arity);
        let mut remaining_field_slots = positional_field_count;
        let mut unbound_type_params = Vec::new();
        let mut unbound_header_args = Vec::new();
        for arg in header_args.iter().skip(parent_arity) {
            if remaining_field_slots > 0 {
                remaining_field_slots -= 1;
                continue;
            }
            match arg {
                MoldHeaderArg::TypeParam(tp) => {
                    unbound_type_params.push(tp.name.clone());
                    unbound_header_args.push(tp.name.clone());
                }
                MoldHeaderArg::Concrete(ty) => {
                    unbound_header_args.push(format!(":{}", Self::type_expr_to_string(ty)));
                }
            }
        }

        if extra_args > 0 && !unbound_header_args.is_empty() {
            let (message, hint) = if unbound_type_params.len() == unbound_header_args.len() {
                (
                    format!(
                        "{} '{}' has unbound type parameter(s): {}. additional child-side header arguments must map to new non-default fields after the inherited prefix",
                        def.kind,
                        def.name,
                        unbound_type_params.join(", ")
                    ),
                    "Add new required non-default fields on the child definition so every appended type parameter has a binding target.",
                )
            } else {
                (
                    format!(
                        "{} '{}' has header argument(s) without binding target(s): {}. additional child-side header arguments must map to new non-default fields after the inherited prefix",
                        def.kind,
                        def.name,
                        unbound_header_args.join(", ")
                    ),
                    "Add new required non-default fields on the child definition so every appended header argument has a binding target.",
                )
            };
            self.errors.push(TypeError {
                message: Self::binding_diag("E1401", message, hint),
                span: def.span.clone(),
            });
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

    fn inheritance_uses_headers(inh: &InheritanceDef) -> bool {
        inh.parent_args.is_some() || inh.child_args.is_some()
    }

    fn inheritance_child_arity(&self, inh: &InheritanceDef, parent_arity: usize) -> usize {
        inh.child_args
            .as_ref()
            .map(Vec::len)
            .or_else(|| inh.parent_args.as_ref().map(Vec::len))
            .unwrap_or(parent_arity)
    }

    fn validate_inheritance_header_arities(
        &mut self,
        inh: &InheritanceDef,
        parent_header: Option<&[MoldHeaderArg]>,
    ) {
        if Self::inheritance_uses_headers(inh) && parent_header.is_none() {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "InheritanceDef '{}' can only declare `Parent[...] => Child[...]` headers when parent '{}' is a mold-like type",
                        inh.child, inh.parent
                    ),
                    "Use header syntax only when inheriting from `Mold[...]` or another mold-derived child header.",
                ),
                span: inh.span.clone(),
            });
            return;
        }

        let parent_arity = parent_header.map(|args| args.len()).unwrap_or_else(|| {
            self.declared_header_arities
                .get(&inh.parent)
                .copied()
                .unwrap_or(0)
        });

        if let Some(parent_args) = &inh.parent_args
            && parent_args.len() != parent_arity
        {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "InheritanceDef '{}' must spell the parent header for '{}' with {} slot(s), got {}",
                        inh.child,
                        inh.parent,
                        parent_arity,
                        parent_args.len()
                    ),
                    "Use the parent type's formal header arity when writing `Parent[...] => Child[...]`.",
                ),
                span: inh.span.clone(),
            });
        }

        let child_arity = self.inheritance_child_arity(inh, parent_arity);
        if child_arity < parent_arity {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1407",
                    format!(
                        "InheritanceDef '{}' cannot shrink header arity below parent '{}' (child: {}, parent: {})",
                        inh.child, inh.parent, child_arity, parent_arity
                    ),
                    "Keep inherited header slots intact and append any new slots on the child side.",
                ),
                span: inh.span.clone(),
            });
        }

        if let Some(parent_header) = parent_header {
            let parent_args = inh.parent_args.as_deref().unwrap_or(parent_header);
            self.validate_child_header_prefix(
                "InheritanceDef",
                &inh.child,
                &inh.parent,
                parent_header,
                parent_args,
                &inh.span,
            );
            let child_args = inh.child_args.as_deref().unwrap_or(parent_args);
            self.validate_child_header_prefix(
                "InheritanceDef",
                &inh.child,
                &inh.parent,
                parent_header,
                child_args,
                &inh.span,
            );
        }
    }

    fn predeclare_header_metadata(&mut self, statements: &[Statement]) {
        self.mold_header_specs.clear();
        self.declared_header_arities.clear();

        for stmt in statements {
            match stmt {
                Statement::TypeDef(td) => {
                    self.declared_header_arities.insert(td.name.clone(), 0);
                }
                Statement::MoldDef(md) => {
                    let header_args = Self::effective_mold_header_args(md);
                    self.mold_header_specs.insert(
                        md.name.clone(),
                        MoldHeaderSpec {
                            header_args: header_args.clone(),
                        },
                    );
                    self.declared_header_arities
                        .insert(md.name.clone(), header_args.len());
                }
                _ => {}
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            for stmt in statements {
                let Statement::InheritanceDef(inh) = stmt else {
                    continue;
                };

                let parent_header = self
                    .mold_header_specs
                    .get(&inh.parent)
                    .map(|spec| spec.header_args.clone());
                let parent_arity = parent_header
                    .as_ref()
                    .map(Vec::len)
                    .or_else(|| self.declared_header_arities.get(&inh.parent).copied());

                if let Some(parent_header) = parent_header {
                    let child_header = inh
                        .child_args
                        .clone()
                        .or_else(|| inh.parent_args.clone())
                        .unwrap_or_else(|| parent_header.clone());
                    if self
                        .mold_header_specs
                        .get(&inh.child)
                        .map(|spec| spec.header_args.as_slice())
                        != Some(child_header.as_slice())
                    {
                        self.mold_header_specs.insert(
                            inh.child.clone(),
                            MoldHeaderSpec {
                                header_args: child_header.clone(),
                            },
                        );
                        changed = true;
                    }

                    let child_arity = child_header.len();
                    if self.declared_header_arities.get(&inh.child) != Some(&child_arity) {
                        self.declared_header_arities
                            .insert(inh.child.clone(), child_arity);
                        changed = true;
                    }
                } else if !Self::inheritance_uses_headers(inh)
                    && let Some(parent_arity) = parent_arity
                    && self.declared_header_arities.get(&inh.child) != Some(&parent_arity)
                {
                    self.declared_header_arities
                        .insert(inh.child.clone(), parent_arity);
                    changed = true;
                }
            }
        }
    }

    fn validate_generic_function_bindability(&mut self, fd: &FuncDef) -> bool {
        let reserved: Vec<String> = fd
            .type_params
            .iter()
            .filter(|tp| self.type_param_name_is_reserved(&tp.name))
            .map(|tp| tp.name.clone())
            .collect();
        if !reserved.is_empty() {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1510",
                    format!(
                        "Generic function '{}' uses reserved concrete type name(s) as type parameter(s): {}",
                        fd.name,
                        reserved.join(", ")
                    ),
                    "Rename generic type parameters so they do not shadow built-in or concrete type names.",
                ),
                span: fd.span.clone(),
            });
            return false;
        }

        let uninferable: Vec<String> = fd
            .type_params
            .iter()
            .filter(|tp| {
                !fd.params.iter().any(|param| {
                    param
                        .type_annotation
                        .as_ref()
                        .is_some_and(|ty| Self::type_expr_mentions_type_param(ty, &tp.name))
                })
            })
            .map(|tp| tp.name.clone())
            .collect();
        if uninferable.is_empty() {
            return true;
        }

        self.errors.push(TypeError {
            message: Self::binding_diag(
                "E1510",
                format!(
                    "Generic function '{}' has uninferable type parameter(s): {}",
                    fd.name,
                    uninferable.join(", ")
                ),
                "In inference-only generic functions, every type parameter must appear in a parameter type annotation.",
            ),
            span: fd.span.clone(),
        });
        false
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
    fn contains_unknown(ty: &Type) -> bool {
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

    /// RCB-50: Check whether a type contains an unresolved type variable.
    ///
    /// A `Named` type that is not registered in the type registry is
    /// an unresolved generic type parameter (e.g. `T`, `U`).  When
    /// either the body type or the declared return type contains such
    /// a variable, the return-type check must be suppressed because
    /// the checker cannot meaningfully compare them.
    /// D28B-023 / D28B-024: look up an active enclosing function's `TypeParam`
    /// by name, walking the stack of nested generic functions inside-out.
    /// Returns `None` if the name does not refer to any active type parameter.
    fn lookup_active_type_param(&self, name: &str) -> Option<&TypeParam> {
        for frame in self.current_func_type_params.iter().rev() {
            if let Some(tp) = frame.iter().find(|tp| tp.name == name) {
                return Some(tp);
            }
        }
        None
    }

    /// D28B-024: returns true when `name` is an active generic type parameter
    /// whose declared subtype constraint is a numeric primitive (`Num` / `Int`
    /// / `Float`). Such a type variable is treated as numeric for arithmetic
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

    /// D28B-023: if `name` is an active generic type parameter whose
    /// declared subtype constraint is a function type (e.g. `F <= :T => :T`),
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

    /// RCB-50: Check whether a type is a mold-defined Named type.
    ///
    /// Custom mold instantiations (e.g. `AlwaysFail[x]()`) return
    /// `Type::Named("AlwaysFail")` from `infer_expr_type`, but the
    /// checker cannot predict what the mold's `solidify` function
    /// actually produces at runtime.  We suppress E1601 in this case.
    fn is_mold_defined_named(&self, ty: &Type) -> bool {
        matches!(ty, Type::Named(name) if self.registry.mold_defs.contains_key(name))
    }

    /// Push a new scope (e.g., entering a function body).
    fn push_scope(&mut self) {
        self.scope_stack.push(HashMap::new());
    }

    /// Pop a scope (e.g., leaving a function body).
    fn pop_scope(&mut self) {
        self.scope_stack.pop();
    }

    fn validate_http_serve_protocol_capability(&mut self, callee_name: &str, args: &[Expr]) {
        if !self.net_http_serve_symbols.contains(callee_name) {
            return;
        }
        if self.compile_target.is_wasm() {
            self.errors.push(TypeError {
                message: format!(
                    "[E1612] {} does not support taida-lang/net HTTP API 'httpServe'. \
                     Hint: Use the interpreter, JS, or native backend instead.",
                    self.compile_target.label()
                ),
                span: args
                    .first()
                    .map(|arg| arg.span().clone())
                    .unwrap_or_else(|| Span {
                        start: 0,
                        end: 0,
                        line: 1,
                        column: 1,
                    }),
            });
            return;
        }
        let Some(tls_expr) = args.get(5) else {
            return;
        };
        if let Expr::BuchiPack(fields, _) = tls_expr
            && let Some(protocol_field) = fields.iter().find(|field| field.name == "protocol")
        {
            match &protocol_field.value {
                Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => (),
                Expr::EnumVariant(enum_name, variant_name, span)
                    if self.net_http_protocol_type_names.contains(enum_name)
                        && self.compile_target.is_js()
                        && matches!(variant_name.as_str(), "H2" | "H3") =>
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1611] `httpServe(..., tls <= @(..., protocol <= {}:{}()))` is not supported on the JS backend. \
                             Hint: JS supports only `{}:H1()`; use the interpreter or native backend for HTTP/2 and HTTP/3.",
                            enum_name, variant_name, enum_name
                        ),
                        span: span.clone(),
                    });
                }
                Expr::IntLit(_, span) | Expr::FloatLit(_, span) | Expr::BoolLit(_, span) => {
                    self.errors.push(TypeError {
                        message: "[E1506] `httpServe` tls.protocol literal must be HttpProtocol or Str. \
                             Hint: Use `HttpProtocol:H1()` / `HttpProtocol:H2()` / `HttpProtocol:H3()` or a legacy string like \"h1.1\"."
                            .to_string(),
                        span: span.clone(),
                    });
                }
                _ => (),
            }
        }
    }

    /// Define a variable in the current scope.
    ///
    /// ## Scope stack invariant (N-75)
    ///
    /// `scope_stack` is **always non-empty** after construction. The global scope
    /// is pushed in `TypeChecker::new()` as `vec![HashMap::new()]`, and
    /// `pop_scope()` only removes inner scopes pushed by `push_scope()`.
    /// No code path calls `pop_scope()` without a preceding `push_scope()`,
    /// so the global scope is never popped. Methods like `define_var`,
    /// `define_var_with_span`, `lookup_var`, and `all_visible_vars` all rely
    /// on this invariant via `.last_mut()` / `.iter().rev()`.
    ///
    /// If `span` is provided and the name already exists in the current scope,
    /// a compile error is reported (same-scope redefinition is forbidden).
    /// Shadowing across scopes (inner scope redefines outer) is allowed.
    /// RCB-201: Validate that all imported symbols are actually exported by the target module.
    fn validate_import_symbols(&mut self, imp: &crate::parser::ImportStmt) {
        use crate::parser::Statement as S;

        if imp.path == "taida-lang/net" {
            for sym in &imp.symbols {
                if !is_net_export_name(&sym.name) {
                    self.errors.push(TypeError {
                        message: format!(
                            "Symbol '{}' not found in module '{}'. The module exports: {}",
                            sym.name,
                            imp.path,
                            net_export_list()
                        ),
                        span: imp.span.clone(),
                    });
                }
            }
            return;
        }

        // Skip other core-bundled and npm packages
        if imp.path.starts_with("npm:") || imp.path.starts_with("taida-lang/") {
            return;
        }

        // Need source_file to resolve relative imports
        let source_file = match &self.source_file {
            Some(f) => f.clone(),
            None => return,
        };

        // Resolve the import path to a .td file + optional facade exports
        let (td_path, pkg_manifest_exports): (std::path::PathBuf, Option<Vec<String>>) = if imp
            .path
            .starts_with("./")
            || imp.path.starts_with("../")
            || imp.path.starts_with('/')
        {
            let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
            let path = source_dir.join(&imp.path);
            if path.exists() {
                (path, None)
            } else {
                return; // Cannot resolve — let downstream handle
            }
        } else {
            // Package import — resolve via .taida/deps/
            let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
            let project_root = Self::find_project_root(source_dir);

            let resolution = if let Some(ref ver) = imp.version {
                crate::pkg::resolver::resolve_package_module_versioned(
                    &project_root,
                    &imp.path,
                    ver,
                )
            } else {
                crate::pkg::resolver::resolve_package_module(&project_root, &imp.path)
            };

            match resolution {
                Some(res) => {
                    match &res.submodule {
                        Some(sub) => {
                            let sub_path = res.pkg_dir.join(format!("{}.td", sub));
                            if sub_path.exists() {
                                (sub_path, None)
                            } else {
                                return; // Cannot resolve submodule — let downstream handle
                            }
                        }
                        None => {
                            // Package root import: use centralized facade validation
                            // B11B-023: Delegates to pkg::facade for DRY validation
                            if let Some(ctx) =
                                crate::pkg::facade::resolve_facade_context(&res.pkg_dir)
                            {
                                let sym_names: Vec<String> =
                                    imp.symbols.iter().map(|s| s.name.clone()).collect();
                                let violations = crate::pkg::facade::validate_facade(
                                    &ctx.facade_exports,
                                    &ctx.entry_path,
                                    &sym_names,
                                );
                                for v in &violations {
                                    match v {
                                        crate::pkg::facade::FacadeViolation::HiddenSymbol {
                                            name,
                                            available,
                                        } => {
                                            self.errors.push(TypeError {
                                                    message: format!(
                                                        "[E1701] Symbol '{}' is not part of the public API declared in packages.tdm. \
                                                         Available exports: {}",
                                                        name,
                                                        available.join(", ")
                                                    ),
                                                    span: imp.span.clone(),
                                                });
                                        }
                                        crate::pkg::facade::FacadeViolation::GhostSymbol {
                                            name,
                                        } => {
                                            self.errors.push(TypeError {
                                                    message: format!(
                                                        "[E1701] Symbol '{}' is declared in packages.tdm but not found in the entry module. \
                                                         The entry module must export all symbols listed in the package facade.",
                                                        name
                                                    ),
                                                    span: imp.span.clone(),
                                                });
                                        }
                                    }
                                }
                                if !violations.is_empty() {
                                    return;
                                }
                                (ctx.entry_path, Some(ctx.facade_exports))
                            } else {
                                // No facade — resolve entry module normally
                                let entry_name =
                                    match crate::pkg::manifest::Manifest::from_dir(&res.pkg_dir) {
                                        Ok(Some(manifest)) => manifest.entry,
                                        _ => "main.td".to_string(),
                                    };
                                let entry_path =
                                    if let Some(stripped) = entry_name.strip_prefix("./") {
                                        res.pkg_dir.join(stripped)
                                    } else {
                                        res.pkg_dir.join(&entry_name)
                                    };
                                if entry_path.exists() {
                                    (entry_path, None)
                                } else {
                                    return;
                                }
                            }
                        }
                    }
                }
                None => return, // Package not installed — let downstream handle
            }
        };

        // Parse the target module
        let source = match std::fs::read_to_string(&td_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let (program, _) = crate::parser::parse(&source);

        // Collect explicit export list from entry module's <<< statements
        let mut exports = std::collections::HashSet::new();
        let mut has_export = false;
        for stmt in &program.statements {
            if let S::Export(export_stmt) = stmt {
                has_export = true;
                for sym in &export_stmt.symbols {
                    exports.insert(sym.clone());
                }
            }
        }

        // B11B-023: Facade validation (membership + ghost) is now handled by
        // pkg::facade::validate_facade() above. If we reach here with a facade,
        // it means all symbols passed validation — proceed to normal export check.
        if pkg_manifest_exports.is_some() {
            return;
        }

        // If no <<< found, all symbols are exported (backward compat)
        if !has_export {
            return;
        }

        // Validate each imported symbol against entry module's <<< export list
        for sym in &imp.symbols {
            if !exports.contains(&sym.name) {
                let export_list = if exports.is_empty() {
                    "(nothing)".to_string()
                } else {
                    let mut sorted: Vec<&String> = exports.iter().collect();
                    sorted.sort();
                    sorted
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                self.errors.push(TypeError {
                    message: format!(
                        "[E1701] Symbol '{}' not found in module '{}'. \
                         The module exports: {}",
                        sym.name, imp.path, export_list
                    ),
                    span: imp.span.clone(),
                });
            }
        }
    }

    /// C18-1: Register Enum (and future TypeDef) types that cross the module
    /// boundary so that `Color:Red()` in the importer does not trigger
    /// `[E1608] Unknown enum type 'Color'.`.
    ///
    /// Behaviour:
    /// 1. Resolve the import path (relative, package, or submodule) using the same
    ///    logic as `validate_import_symbols`.
    /// 2. Parse the target module and collect every `EnumDef` whose name is being
    ///    imported by the current statement.
    /// 3. If the importer has **not** already defined an enum with the same local
    ///    name, register it into `self.registry`. The wire-order is the import
    ///    origin (source of truth).
    /// 4. If the importer **has** already defined the enum locally (common pattern
    ///    during the C18 transition), check that the variant list is identical;
    ///    any mismatch emits `[E1618] Enum '<name>' variant order mismatch across
    ///    module boundary.` to catch the silent-bug risk raised in ROOT-5.
    ///
    /// Notes:
    /// - `[E1618]` is allocated for this check because `[E1610]` is already
    ///   occupied by cyclic-inheritance detection. The design rationale is
    ///   recorded in `.dev/C18_BLOCKERS.md` (C18B-001).
    /// - Aliased imports (`>>> ./m.td => @(Color: Paint)`) register the enum
    ///   under the alias, mirroring the interpreter behaviour.
    fn register_imported_types(&mut self, imp: &crate::parser::ImportStmt) {
        use crate::parser::Statement as S;

        // Core bundled packages are handled elsewhere (net / crypto).
        if imp.path.starts_with("npm:") || imp.path.starts_with("taida-lang/") {
            return;
        }

        // Same path-resolution strategy as `validate_import_symbols`.
        let source_file = match &self.source_file {
            Some(f) => f.clone(),
            None => return,
        };

        let td_path: std::path::PathBuf = if imp.path.starts_with("./")
            || imp.path.starts_with("../")
            || imp.path.starts_with('/')
        {
            let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
            let path = source_dir.join(&imp.path);
            if path.exists() { path } else { return }
        } else {
            // Package import — resolve via .taida/deps/
            let source_dir = source_file.parent().unwrap_or(std::path::Path::new("."));
            let project_root = Self::find_project_root(source_dir);
            let resolution = if let Some(ref ver) = imp.version {
                crate::pkg::resolver::resolve_package_module_versioned(
                    &project_root,
                    &imp.path,
                    ver,
                )
            } else {
                crate::pkg::resolver::resolve_package_module(&project_root, &imp.path)
            };
            match resolution {
                Some(res) => match &res.submodule {
                    Some(sub) => {
                        let sub_path = res.pkg_dir.join(format!("{}.td", sub));
                        if sub_path.exists() { sub_path } else { return }
                    }
                    None => {
                        let entry_name =
                            match crate::pkg::manifest::Manifest::from_dir(&res.pkg_dir) {
                                Ok(Some(manifest)) => manifest.entry,
                                _ => "main.td".to_string(),
                            };
                        let entry_path = if let Some(stripped) = entry_name.strip_prefix("./") {
                            res.pkg_dir.join(stripped)
                        } else {
                            res.pkg_dir.join(&entry_name)
                        };
                        if entry_path.exists() {
                            entry_path
                        } else {
                            return;
                        }
                    }
                },
                None => return,
            }
        };

        let source = match std::fs::read_to_string(&td_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let (program, _) = crate::parser::parse(&source);

        // Build a map of imported-symbol-name → local-alias (or the same name).
        let requested: std::collections::HashMap<&str, &str> = imp
            .symbols
            .iter()
            .map(|s| {
                (
                    s.name.as_str(),
                    s.alias.as_deref().unwrap_or(s.name.as_str()),
                )
            })
            .collect();
        if requested.is_empty() {
            return;
        }

        for stmt in &program.statements {
            if let S::EnumDef(ed) = stmt
                && let Some(&local_name) = requested.get(ed.name.as_str())
            {
                let variants: Vec<String> = ed.variants.iter().map(|v| v.name.clone()).collect();

                if let Some(existing) = self.registry.get_enum_variants(local_name) {
                    // Local redefinition already present — must match the
                    // exported module's order exactly.
                    if existing != variants {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1618] Enum '{}' variant order mismatch across module boundary. \
                                 Defined at '{}': [{}]. Imported as: [{}]. \
                                 Hint: Align local redefinition order with the exporting module, \
                                 or remove the local redefinition and rely on the imported type.",
                                local_name,
                                td_path.display(),
                                variants.join(", "),
                                existing.join(", ")
                            ),
                            span: imp.span.clone(),
                        });
                    }
                } else {
                    // No local redefinition — register as if declared here.
                    self.registry.register_enum(local_name, variants);
                    self.declared_concrete_type_names
                        .insert(local_name.to_string());
                    self.declared_header_arities
                        .insert(local_name.to_string(), 0);
                }
            }
        }
    }

    /// Find project root by walking up from the given directory.
    /// Looks for `packages.tdm`, `taida.toml`, `.taida`, or `.git`.
    fn find_project_root(start_dir: &std::path::Path) -> std::path::PathBuf {
        let mut dir = start_dir.to_path_buf();
        loop {
            if dir.join("packages.tdm").exists()
                || dir.join("taida.toml").exists()
                || dir.join(".taida").exists()
                || dir.join(".git").exists()
            {
                return dir;
            }
            if !dir.pop() {
                break;
            }
        }
        start_dir.to_path_buf()
    }

    fn define_var(&mut self, name: &str, ty: Type) {
        self.define_var_with_span(name, ty, None);
    }

    /// Define a variable with a span for duplicate detection.
    fn define_var_with_span(&mut self, name: &str, ty: Type, span: Option<&Span>) {
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
                return;
            }
            scope.insert(name.to_string(), ty);
        }
    }

    /// C13-1 / C13B-007: True if `name` in an intermediate pipeline
    /// step should be treated as a function-like reference (classic
    /// pipeline semantics: call it with the current value). False means
    /// bind-and-forward: the current step's value is bound to `name` and
    /// passed through unchanged.
    ///
    /// A name is considered callable if:
    ///   - the variable is declared with a `Function` type in scope, or
    ///   - the name is registered as a user-defined (possibly generic)
    ///     function / type / mold, or
    ///   - it is a known builtin identifier.
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
        matches!(
            name,
            "debug"
                | "toString"
                | "toStr"
                | "typeOf"
                | "typeof"
                | "jsonEncode"
                | "jsonPretty"
                | "nowMs"
                | "assert"
                | "range"
                | "enumerate"
                | "zip"
                | "hashMap"
                | "setOf"
                | "stdout"
                | "stderr"
                | "exit"
                | "stdin"
                | "stdinLine"
                | "argv"
                | "sleep"
        )
    }

    /// Look up a variable type from the scope stack (innermost first).
    pub fn lookup_var(&self, name: &str) -> Option<Type> {
        for scope in self.scope_stack.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
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

    /// Unwrap a mold type to get its inner value type.
    /// Used for `]=>` and `<=[` unmold operations.
    fn unmold_type(&self, ty: &Type) -> Type {
        match ty {
            // JSON unmolds to dynamic type (needs schema)
            Type::Json => Type::Unknown,
            // Molten is opaque — cannot unmold directly
            Type::Molten => Type::Unknown,
            // Generic mold types: extract the first type argument
            Type::Generic(name, args) => {
                match name.as_str() {
                    "Lax" | "Result" | "Async" | "Gorillax" | "RelaxedGorillax" => {
                        args.first().cloned().unwrap_or(Type::Unknown)
                    }
                    // Stream[T] unmolds to @[T] (List)
                    "Stream" => {
                        let inner = args.first().cloned().unwrap_or(Type::Unknown);
                        Type::List(Box::new(inner))
                    }
                    _ => {
                        // Custom mold types registered in the registry:
                        // extract the first type argument (filling type T)
                        if self.registry.mold_defs.contains_key(name.as_str()) {
                            args.first().cloned().unwrap_or(Type::Unknown)
                        } else {
                            Type::Unknown
                        }
                    }
                }
            }
            // Named type that is a registered mold: unmold to Unknown
            // (type parameter not instantiated, so we can't determine T)
            Type::Named(name) => {
                if self.registry.mold_defs.contains_key(name.as_str()) {
                    Type::Unknown
                } else {
                    // Non-mold named types pass through
                    ty.clone()
                }
            }
            // Unknown stays unknown
            Type::Unknown => Type::Unknown,
            // Non-mold types pass through (runtime will handle)
            _ => ty.clone(),
        }
    }

    /// Check an entire program. Collects type definitions first,
    /// then checks all statements.
    pub fn check_program(&mut self, program: &Program) {
        self.seen_func_defs.clear();
        self.declared_concrete_type_names.clear();
        for stmt in &program.statements {
            match stmt {
                Statement::EnumDef(ed) => {
                    self.declared_concrete_type_names.insert(ed.name.clone());
                }
                Statement::TypeDef(td) => {
                    self.declared_concrete_type_names.insert(td.name.clone());
                }
                Statement::MoldDef(md) => {
                    self.declared_concrete_type_names.insert(md.name.clone());
                }
                Statement::InheritanceDef(inh) => {
                    self.declared_concrete_type_names.insert(inh.child.clone());
                }
                // N-64: Intentional catch-all — the first pass only collects TypeDef,
                // MoldDef, and InheritanceDef names for forward-reference resolution.
                // All other statement kinds (Assignment, FuncDef, Expr, etc.) are
                // processed in the second pass by check_statement().
                _ => {}
            }
        }

        // Predeclare header metadata so generic inheritance validation is not source-order dependent.
        self.predeclare_header_metadata(&program.statements);

        // First pass: register base type definitions and function signatures before inheritances.
        for stmt in &program.statements {
            if !matches!(stmt, Statement::InheritanceDef(_)) {
                self.register_types(stmt);
            }
        }

        // Register inheritances only after their mold-like parents have field metadata available.
        let mut pending_inheritances: Vec<&Statement> = program
            .statements
            .iter()
            .filter(|stmt| matches!(stmt, Statement::InheritanceDef(_)))
            .collect();
        while !pending_inheritances.is_empty() {
            let mut next_round = Vec::new();
            let mut made_progress = false;
            for stmt in pending_inheritances {
                let Statement::InheritanceDef(inh) = stmt else {
                    continue;
                };
                let parent_is_mold_like = self.mold_header_specs.contains_key(&inh.parent);
                if !parent_is_mold_like || self.mold_field_defs.contains_key(&inh.parent) {
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

        // C12-3 / FB-8: promote non-tail mutual recursion to a
        // compile-time error so programs that would overflow the stack at
        // runtime (`Maximum call depth exceeded`) are rejected up front.
        // Tail-only mutual recursion is left to pass — the Interpreter / JS
        // backends handle it via the mutual-TCO trampoline and the Native
        // backend treats it as a regular call (see
        // docs/reference/tail_recursion.md).
        self.check_mutual_recursion_errors(program);
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
        let findings = crate::graph::verify::run_check(
            "mutual-recursion",
            program,
            self.source_file
                .as_deref()
                .and_then(|p| p.to_str())
                .unwrap_or("<program>"),
        );
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
                    start: 0,
                    end: 0,
                });
            self.errors.push(TypeError {
                message: f.message,
                span,
            });
        }
    }

    /// Register type definitions from a statement (first pass).
    fn register_types(&mut self, stmt: &Statement) {
        match stmt {
            Statement::EnumDef(ed) => {
                let has_collision = self.registry.type_defs.contains_key(&ed.name)
                    || self.registry.enum_defs.contains_key(&ed.name)
                    || self.func_types.contains_key(&ed.name)
                    || self.registry.mold_defs.contains_key(&ed.name);
                if has_collision {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1501] Name '{}' is already defined in this scope. \
                             Redefinition in the same scope is not allowed. \
                             Hint: Use a different name, or define it in an inner scope (shadowing is allowed).",
                            ed.name
                        ),
                        span: ed.span.clone(),
                    });
                }
                let mut seen = HashSet::new();
                for variant in &ed.variants {
                    if !seen.insert(variant.name.clone()) {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1501] Enum '{}' redefines variant '{}'. Hint: Enum variants must be unique within the same enum.",
                                ed.name, variant.name
                            ),
                            span: variant.span.clone(),
                        });
                    }
                }
                self.registry.register_enum(
                    &ed.name,
                    ed.variants
                        .iter()
                        .map(|variant| variant.name.clone())
                        .collect(),
                );
                self.declared_header_arities.insert(ed.name.clone(), 0);
            }
            Statement::TypeDef(td) => {
                // E1501: Check for TypeDef name collision with existing types, functions, or molds
                let has_collision = self.registry.type_defs.contains_key(&td.name)
                    || self.registry.enum_defs.contains_key(&td.name)
                    || self.func_types.contains_key(&td.name)
                    || self.registry.mold_defs.contains_key(&td.name);
                if has_collision {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1501] Name '{}' is already defined in this scope. \
                             Redefinition in the same scope is not allowed. \
                             Hint: Use a different name, or define it in an inner scope (shadowing is allowed).",
                            td.name
                        ),
                        span: td.span.clone(),
                    });
                }
                self.validate_class_like_fields("TypeDef", &td.name, &td.fields);
                let fields: Vec<(String, Type)> = td
                    .fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .map(|f| {
                        let ty = f
                            .type_annotation
                            .as_ref()
                            .map(|t| self.registry.resolve_type(t))
                            .unwrap_or(Type::Unknown);
                        (f.name.clone(), ty)
                    })
                    .collect();
                self.registry.register_type(&td.name, fields);
                self.declared_header_arities.insert(td.name.clone(), 0);
            }
            Statement::MoldDef(md) => {
                self.validate_class_like_fields("MoldDef", &md.name, &md.fields);
                let header_args = Self::effective_mold_header_args(md);
                self.validate_mold_root_header(md, &header_args);
                self.validate_mold_extension_bindings(
                    MoldBindingDef {
                        kind: "MoldDef",
                        name: &md.name,
                        span: &md.span,
                    },
                    1,
                    &header_args,
                    &md.fields,
                    &HashSet::new(),
                );
                let type_params = Self::collect_mold_type_param_names(&header_args);
                let fields: Vec<(String, Type)> = md
                    .fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .map(|f| {
                        let ty = f
                            .type_annotation
                            .as_ref()
                            .map(|t| self.registry.resolve_type(t))
                            .unwrap_or(Type::Unknown);
                        (f.name.clone(), ty)
                    })
                    .collect();
                self.registry
                    .register_mold(&md.name, type_params, fields.clone());
                self.registry.register_type(&md.name, fields);
                self.mold_header_specs
                    .insert(md.name.clone(), MoldHeaderSpec { header_args });
                self.mold_field_defs
                    .insert(md.name.clone(), md.fields.clone());
                self.declared_header_arities
                    .insert(md.name.clone(), Self::effective_mold_header_args(md).len());
            }
            Statement::InheritanceDef(inh) => {
                self.validate_class_like_fields("InheritanceDef", &inh.child, &inh.fields);
                let parent_header = self
                    .mold_header_specs
                    .get(&inh.parent)
                    .map(|spec| spec.header_args.clone());
                self.validate_inheritance_header_arities(inh, parent_header.as_deref());
                let extra_fields: Vec<(String, Type)> = inh
                    .fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .map(|f| {
                        let ty = f
                            .type_annotation
                            .as_ref()
                            .map(|t| self.registry.resolve_type(t))
                            .unwrap_or(Type::Unknown);
                        (f.name.clone(), ty)
                    })
                    .collect();
                // RCB-216: Check for incompatible field redefinition.
                // If a child redefines a parent field, the type must be compatible
                // (same type or subtype). Redefining `legs: Int` as `legs: Str` violates LSP.
                if let Some(parent_fields) = self.registry.get_type_fields(&inh.parent) {
                    for (child_name, child_ty) in &extra_fields {
                        if let Some((_, parent_ty)) =
                            parent_fields.iter().find(|(n, _)| n == child_name)
                        {
                            // Both types must be known and compatible
                            if !matches!(parent_ty, Type::Unknown)
                                && !matches!(child_ty, Type::Unknown)
                                && parent_ty != child_ty
                                && !self.registry.is_subtype_of(child_ty, parent_ty)
                            {
                                self.errors.push(TypeError {
                                    message: Self::binding_diag(
                                        "E1410",
                                        format!(
                                            "InheritanceDef '{}' redefines field '{}' with incompatible type '{}' (parent '{}' declares it as '{}')",
                                            inh.child, child_name, child_ty, inh.parent, parent_ty
                                        ),
                                        "A child type's field must be compatible with the parent's field type. \
                                         Use the same type or a subtype.",
                                    ),
                                    span: inh.span.clone(),
                                });
                            }
                        }
                    }
                }

                // RCB-51: Detect cyclic inheritance and emit an error
                // instead of silently accepting it (which causes
                // is_subtype_of to loop forever).
                let registered = if self.registry.is_error_type(&inh.parent) {
                    self.registry
                        .register_error_type(&inh.parent, &inh.child, extra_fields)
                } else {
                    self.registry
                        .register_inheritance(&inh.parent, &inh.child, extra_fields)
                };
                if !registered {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1610] Cyclic inheritance detected: '{}' => '{}' would create a cycle in the inheritance chain. \
                             Hint: Remove one of the inheritance relationships to break the cycle.",
                            inh.parent, inh.child
                        ),
                        span: inh.span.clone(),
                    });
                }

                if let Some(ref parent_header) = parent_header {
                    let child_header = inh
                        .child_args
                        .clone()
                        .or_else(|| inh.parent_args.clone())
                        .unwrap_or_else(|| parent_header.clone());
                    self.validate_unique_mold_type_param_names(
                        "InheritanceDef",
                        &inh.child,
                        &child_header,
                        &inh.span,
                    );
                    let parent_field_defs = self
                        .mold_field_defs
                        .get(&inh.parent)
                        .cloned()
                        .unwrap_or_default();
                    let inherited_field_names: HashSet<String> = parent_field_defs
                        .iter()
                        .map(|field| field.name.clone())
                        .collect();
                    self.validate_mold_extension_bindings(
                        MoldBindingDef {
                            kind: "InheritanceDef",
                            name: &inh.child,
                            span: &inh.span,
                        },
                        parent_header.len(),
                        &child_header,
                        &inh.fields,
                        &inherited_field_names,
                    );

                    let merged_field_defs = Self::merge_field_defs(&parent_field_defs, &inh.fields);
                    let merged_fields: Vec<(String, Type)> = merged_field_defs
                        .iter()
                        .filter(|f| !f.is_method)
                        .map(|f| {
                            let ty = f
                                .type_annotation
                                .as_ref()
                                .map(|t| self.registry.resolve_type(t))
                                .unwrap_or(Type::Unknown);
                            (f.name.clone(), ty)
                        })
                        .collect();
                    self.registry.register_mold(
                        &inh.child,
                        Self::collect_mold_type_param_names(&child_header),
                        merged_fields.clone(),
                    );
                    self.registry.register_type(&inh.child, merged_fields);
                    self.mold_header_specs.insert(
                        inh.child.clone(),
                        MoldHeaderSpec {
                            header_args: child_header.clone(),
                        },
                    );
                    self.mold_field_defs
                        .insert(inh.child.clone(), merged_field_defs);
                }

                let parent_arity = parent_header
                    .as_ref()
                    .map(Vec::len)
                    .or_else(|| self.declared_header_arities.get(&inh.parent).copied())
                    .unwrap_or(0);
                let child_arity = if parent_header.is_some() {
                    self.inheritance_child_arity(inh, parent_arity)
                } else {
                    parent_arity
                };
                self.declared_header_arities
                    .insert(inh.child.clone(), child_arity);
            }
            Statement::FuncDef(fd) => {
                let duplicate_func_name = !self.seen_func_defs.insert(fd.name.clone());
                let generic_is_inferable = if fd.type_params.is_empty() {
                    true
                } else {
                    self.validate_generic_function_bindability(fd)
                };
                if duplicate_func_name {
                    self.invalid_func_defs.insert(fd.name.clone());
                    self.func_types.remove(&fd.name);
                    self.func_param_counts.remove(&fd.name);
                    self.func_param_types.remove(&fd.name);
                    self.generic_func_defs.remove(&fd.name);
                } else if fd.type_params.is_empty() || generic_is_inferable {
                    self.invalid_func_defs.remove(&fd.name);
                    // Register function return type for later lookup
                    let ret_ty = fd
                        .return_type
                        .as_ref()
                        .map(|t| self.registry.resolve_type(t))
                        .unwrap_or(Type::Unknown);
                    self.func_types.insert(fd.name.clone(), ret_ty);
                    self.func_param_counts
                        .insert(fd.name.clone(), fd.params.len());
                    // Register parameter types for partial application type inference
                    let param_types: Vec<Type> = fd
                        .params
                        .iter()
                        .map(|p| {
                            p.type_annotation
                                .as_ref()
                                .map(|t| self.registry.resolve_type(t))
                                .unwrap_or(Type::Unknown)
                        })
                        .collect();
                    self.func_param_types.insert(fd.name.clone(), param_types);
                    if !fd.type_params.is_empty() {
                        self.generic_func_defs.insert(fd.name.clone(), fd.clone());
                    }
                } else {
                    self.invalid_func_defs.insert(fd.name.clone());
                    self.func_types.remove(&fd.name);
                    self.func_param_counts.remove(&fd.name);
                    self.func_param_types.remove(&fd.name);
                    self.generic_func_defs.remove(&fd.name);
                }
            }
            Statement::Import(imp) => {
                // Core bundled package signatures (imported symbol path).
                if imp.path == "taida-lang/crypto" {
                    for sym in &imp.symbols {
                        if sym.name == "sha256" {
                            let local_name = sym.alias.as_ref().unwrap_or(&sym.name).clone();
                            self.func_types.insert(local_name.clone(), Type::Str);
                            self.func_param_counts.insert(local_name, 1);
                        }
                    }
                } else if imp.path == "taida-lang/net" {
                    for sym in &imp.symbols {
                        let local_name = sym.alias.as_ref().unwrap_or(&sym.name);
                        self.register_net_import_symbol(&sym.name, local_name);
                    }
                } else if imp.path == "taida-lang/os" {
                    for sym in &imp.symbols {
                        let local_name = sym.alias.as_ref().unwrap_or(&sym.name).clone();
                        self.register_os_import_symbol(&sym.name, &local_name);
                    }
                }
            }
            _ => {}
        }
    }

    /// Validate class-like definition fields (TypeDef / MoldDef / InheritanceDef).
    /// Non-method fields must have either a type annotation (`field: Type`)
    /// or a default value (`field <= value`).
    fn validate_class_like_fields(&mut self, kind: &str, def_name: &str, fields: &[FieldDef]) {
        for field in fields.iter() {
            // C12B-023 bypass closure (3rd layer, 2026-04-15): reject any
            // `__`-prefixed field name in TypeDef / MoldDef / InheritanceDef
            // bodies. This is the definition-site twin of the expression-site
            // reject in `check_mold_errors_in_expr`. Without this check, a
            // user can forge nominal packs indirectly by declaring
            // `Fake = @(__type <= "Regex", ...)` and then instantiating
            // `Fake(...)`, which materialises a pack whose `__type` field
            // is literally `"Regex"` (see `codegen/lower/molds.rs` and
            // `interpreter/eval.rs` — both copy default field values into
            // the pack verbatim). Reject on the `FieldDef` itself,
            // regardless of `is_method`, so that the rule is uniform
            // across fields and methods.
            self.validate_reserved_internal_field_name(kind, def_name, field);
            if !field.is_method && field.type_annotation.is_none() && field.default_value.is_none()
            {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1400",
                        format!(
                            "{} '{}' field '{}' must declare either a type annotation (`{}: Type`) or a default value (`{} <= value`)",
                            kind, def_name, field.name, field.name, field.name
                        ),
                        "Declare fields as `name: Type` or `name <= default`; bare `name` is not allowed."
                    ),
                    span: field.span.clone(),
                });
            }
        }
    }

    /// C12B-023 bypass closure (3rd layer): reject `FieldDef` whose name
    /// starts with the reserved internal-field prefix (`__`). Shared by
    /// TypeDef / MoldDef / InheritanceDef. Emits `[E1617]` — the same
    /// diagnostic code used for (1) the AST-level `Expr::BuchiPack`/
    /// `Expr::TypeInst` literal reject in `check_mold_errors_in_expr`
    /// and (2) the wasm backend Regex rejection in
    /// `emit_wasm_c::validate_regex_api_for_wasm`. The three
    /// checks form a 3-layer defence (definition / expression /
    /// backend) — any user-authored code path that tries to fabricate
    /// a nominal pack now fails at `taida check`.
    ///
    /// Rationale: `__`-prefix is the language-wide convention for
    /// compiler-internal tags (`__type`, `__value`, `__default`,
    /// `__body_stream`, etc.). These fields are materialised by
    /// runtime-side `Value::BuchiPack(...)` construction (Rust) and
    /// IR ops — never through parser-produced `FieldDef` nodes in
    /// well-formed code. The parser does not synthesise any
    /// `__`-prefixed `FieldDef` (see `parser.rs:1373/1473/1511/1524/
    /// 1571/1590` — all field names come from user source or the
    /// literal `"unmold"`). So this check can unconditionally reject
    /// `__`-prefixed `FieldDef.name` without a built-in exception
    /// escape hatch.
    fn validate_reserved_internal_field_name(
        &mut self,
        kind: &str,
        def_name: &str,
        field: &FieldDef,
    ) {
        if !field.name.starts_with(RESERVED_INTERNAL_FIELD_PREFIX) {
            return;
        }
        self.errors.push(TypeError {
            message: format!(
                "[E1617] {} '{}' declares field `{}`, whose `__`-prefix is reserved for \
                 compiler-internal use. User definitions must not declare `__`-prefixed \
                 fields: they would materialise as compiler-internal tags (e.g., `__type`, \
                 `__value`) on the runtime pack and fabricate fake nominal-type identity \
                 without the invariants that official constructors guarantee. \
                 Hint: rename the field to a non-`__`-prefixed name, or use the official \
                 constructor (e.g., `Regex(pat, flags?)`, `Lax(...)`, `Async(...)`) \
                 instead of forging the pack by hand.",
                kind, def_name, field.name
            ),
            span: field.span.clone(),
        });
    }

    fn resolve_mold_header_type(&self, ty: &TypeExpr, bound_types: &HashMap<String, Type>) -> Type {
        match ty {
            TypeExpr::Named(name) => bound_types
                .get(name)
                .cloned()
                .unwrap_or_else(|| self.registry.resolve_type(ty)),
            TypeExpr::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|field| {
                        let field_ty = field
                            .type_annotation
                            .as_ref()
                            .map(|ty| self.resolve_mold_header_type(ty, bound_types))
                            .unwrap_or(Type::Unknown);
                        (field.name.clone(), field_ty)
                    })
                    .collect(),
            ),
            TypeExpr::List(inner) => {
                Type::List(Box::new(self.resolve_mold_header_type(inner, bound_types)))
            }
            TypeExpr::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| self.resolve_mold_header_type(arg, bound_types))
                    .collect(),
            ),
            TypeExpr::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| self.resolve_mold_header_type(param, bound_types))
                    .collect(),
                Box::new(self.resolve_mold_header_type(ret, bound_types)),
            ),
        }
    }

    fn mold_header_type_compatible(&self, actual: &Type, expected: &Type) -> bool {
        match (actual, expected) {
            (_, Type::Unknown) | (Type::Unknown, _) => true,
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

    /// Validate custom mold instantiation binding rules for `[]` and `()`.
    fn validate_custom_mold_inst_bindings(
        &mut self,
        name: &str,
        type_args: &[Expr],
        fields: &[BuchiField],
        span: &Span,
    ) {
        let mold_fields = match self.mold_field_defs.get(name).cloned() {
            Some(f) => f,
            None => return,
        };

        let required_fields: Vec<String> = mold_fields
            .iter()
            .filter(|f| !f.is_method && f.default_value.is_none() && f.name != "filling")
            .map(|f| f.name.clone())
            .collect();
        let optional_fields: Vec<String> = mold_fields
            .iter()
            .filter(|f| !f.is_method && f.default_value.is_some())
            .map(|f| f.name.clone())
            .collect();

        // filling + non-default fields
        let required_positional = 1 + required_fields.len();
        if type_args.len() < required_positional {
            let missing_names: Vec<String> = std::iter::once("filling".to_string())
                .chain(required_fields.iter().cloned())
                .skip(type_args.len())
                .collect();
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1402",
                    format!(
                        "MoldInst '{}' requires {} positional `[]` argument(s), got {} (missing: {})",
                        name,
                        required_positional,
                        type_args.len(),
                        missing_names.join(", ")
                    ),
                    "Provide missing required values in `[]` order: `filling`, then non-default fields."
                ),
                span: span.clone(),
            });
        }

        if type_args.len() > required_positional {
            self.errors.push(TypeError {
                message: Self::binding_diag(
                    "E1403",
                    format!(
                        "MoldInst '{}' takes {} positional `[]` argument(s), got {}. \
defaulted fields must be provided via `()`",
                        name,
                        required_positional,
                        type_args.len()
                    ),
                    "Move optional/defaulted values from `[]` to named `()` options.",
                ),
                span: span.clone(),
            });
        }

        let required_set: std::collections::HashSet<String> = required_fields.into_iter().collect();
        let optional_set: std::collections::HashSet<String> = optional_fields.into_iter().collect();
        let mut seen = std::collections::HashSet::<String>::new();

        for field in fields {
            if !seen.insert(field.name.clone()) {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1404",
                        format!("MoldInst '{}' has duplicate option '{}'", name, field.name),
                        "Specify each named option in `()` at most once.",
                    ),
                    span: field.span.clone(),
                });
                continue;
            }

            if required_set.contains(&field.name) {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1405",
                        format!(
                            "MoldInst '{}' field '{}' must be passed via `[]`, not `()`",
                            name, field.name
                        ),
                        "Pass non-default fields as positional `[]` arguments in declaration order."
                    ),
                    span: field.span.clone(),
                });
            } else if !optional_set.contains(&field.name) {
                self.errors.push(TypeError {
                    message: Self::binding_diag(
                        "E1406",
                        format!(
                            "MoldInst '{}' has undefined option '{}' in `()`",
                            name, field.name
                        ),
                        "Use only fields declared with defaults as `()` options.",
                    ),
                    span: field.span.clone(),
                });
            }
        }
    }

    fn validate_mold_header_constraints(&mut self, name: &str, type_args: &[Expr], span: &Span) {
        let Some(spec) = self.mold_header_specs.get(name).cloned() else {
            return;
        };

        let mut bound_types = HashMap::<String, Type>::new();
        for (idx, actual_expr) in type_args.iter().enumerate() {
            let actual = self.infer_expr_type(actual_expr);
            let Some(header_arg) = spec.header_args.get(idx) else {
                continue;
            };
            self.validate_single_mold_header_arg(
                name,
                idx,
                &actual,
                header_arg,
                &bound_types,
                span,
            );
            self.bind_mold_header_arg(header_arg, &actual, &mut bound_types);
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

    fn validate_single_mold_header_arg(
        &mut self,
        name: &str,
        idx: usize,
        actual: &Type,
        header_arg: &MoldHeaderArg,
        bound_types: &HashMap<String, Type>,
        span: &Span,
    ) {
        match header_arg {
            MoldHeaderArg::TypeParam(tp) => {
                if let Some(constraint) = &tp.constraint {
                    let expected = self.resolve_mold_header_type(constraint, bound_types);
                    if !self.mold_header_type_compatible(actual, &expected) {
                        self.errors.push(TypeError {
                            message: Self::binding_diag(
                                "E1409",
                                format!(
                                    "MoldInst '{}' positional `[]` argument {} violates constraint on '{}': expected {}, got {}",
                                    name,
                                    idx + 1,
                                    tp.name,
                                    expected,
                                    actual
                                ),
                                "Pass a value whose inferred type satisfies the constrained mold header.",
                            ),
                            span: span.clone(),
                        });
                    }
                }
            }
            MoldHeaderArg::Concrete(concrete) => {
                let expected = self.resolve_mold_header_type(concrete, bound_types);
                if !self.mold_header_type_compatible(actual, &expected) {
                    self.errors.push(TypeError {
                        message: Self::binding_diag(
                            "E1408",
                            format!(
                                "MoldInst '{}' positional `[]` argument {} is fixed to {}, got {}",
                                name,
                                idx + 1,
                                expected,
                                actual
                            ),
                            "Pass a value whose inferred type matches the concrete mold header.",
                        ),
                        span: span.clone(),
                    });
                }
            }
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

    fn substitute_generic_type(
        &self,
        pattern: &Type,
        generic_names: &HashSet<String>,
        bindings: &HashMap<String, Type>,
    ) -> Type {
        match pattern {
            Type::Named(name) if generic_names.contains(name) => bindings
                .get(name)
                .cloned()
                .unwrap_or_else(|| pattern.clone()),
            Type::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|(name, ty)| {
                        (
                            name.clone(),
                            self.substitute_generic_type(ty, generic_names, bindings),
                        )
                    })
                    .collect(),
            ),
            Type::List(inner) => Type::List(Box::new(self.substitute_generic_type(
                inner,
                generic_names,
                bindings,
            ))),
            Type::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| self.substitute_generic_type(arg, generic_names, bindings))
                    .collect(),
            ),
            Type::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| self.substitute_generic_type(param, generic_names, bindings))
                    .collect(),
                Box::new(self.substitute_generic_type(ret, generic_names, bindings)),
            ),
            _ => pattern.clone(),
        }
    }

    fn instantiate_generic_type(
        &self,
        pattern: &Type,
        generic_names: &HashSet<String>,
        bindings: &HashMap<String, Type>,
    ) -> Type {
        match pattern {
            Type::Named(name) if generic_names.contains(name) => {
                bindings.get(name).cloned().unwrap_or(Type::Unknown)
            }
            Type::BuchiPack(fields) => Type::BuchiPack(
                fields
                    .iter()
                    .map(|(name, ty)| {
                        (
                            name.clone(),
                            self.instantiate_generic_type(ty, generic_names, bindings),
                        )
                    })
                    .collect(),
            ),
            Type::List(inner) => Type::List(Box::new(self.instantiate_generic_type(
                inner,
                generic_names,
                bindings,
            ))),
            Type::Generic(name, args) => Type::Generic(
                name.clone(),
                args.iter()
                    .map(|arg| self.instantiate_generic_type(arg, generic_names, bindings))
                    .collect(),
            ),
            Type::Function(params, ret) => Type::Function(
                params
                    .iter()
                    .map(|param| self.instantiate_generic_type(param, generic_names, bindings))
                    .collect(),
                Box::new(self.instantiate_generic_type(ret, generic_names, bindings)),
            ),
            _ => pattern.clone(),
        }
    }

    fn validate_generic_function_bindings(
        &mut self,
        fd: &FuncDef,
        bindings: &HashMap<String, Type>,
        span: &Span,
    ) {
        for type_param in &fd.type_params {
            let Some(actual) = bindings.get(&type_param.name) else {
                continue;
            };
            let Some(constraint) = &type_param.constraint else {
                continue;
            };
            let expected = self.resolve_mold_header_type(constraint, bindings);
            if !self.mold_header_type_compatible(actual, &expected) {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1509] Generic function type parameter '{}' violates its constraint: expected {}, got {}. Hint: Pass arguments that satisfy the declared generic constraint.",
                        type_param.name, expected, actual
                    ),
                    span: span.clone(),
                });
            }
        }
    }

    fn validate_generic_function_inference(
        &mut self,
        fd: &FuncDef,
        bindings: &HashMap<String, Type>,
        span: &Span,
    ) -> bool {
        let missing: Vec<String> = fd
            .type_params
            .iter()
            .filter(|tp| !bindings.contains_key(&tp.name))
            .map(|tp| tp.name.clone())
            .collect();
        if missing.is_empty() {
            return true;
        }

        self.errors.push(TypeError {
            message: Self::binding_diag(
                "E1510",
                format!(
                    "Generic function '{}' could not infer type parameter(s): {}",
                    fd.name,
                    missing.join(", ")
                ),
                "Pass arguments whose annotated parameter types determine every generic type parameter.",
            ),
            span: span.clone(),
        });
        false
    }

    fn validate_function_param_defaults(&mut self, fd: &FuncDef, param_types: &[Type]) {
        let param_names: Vec<String> = fd.params.iter().map(|p| p.name.clone()).collect();

        for (i, param) in fd.params.iter().enumerate() {
            let ty = param_types.get(i).cloned().unwrap_or(Type::Unknown);

            if let Some(default_expr) = &param.default_value {
                let forbidden: HashSet<String> = param_names[i..].iter().cloned().collect();
                if let Some(illegal_ref) =
                    Self::find_forbidden_default_ref(default_expr, &forbidden)
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1302] Default value for parameter '{}' cannot reference '{}' (self or later parameter). Hint: Reference only earlier parameters in default expressions.",
                            param.name, illegal_ref
                        ),
                        span: param.span.clone(),
                    });
                }

                let default_ty = self.infer_expr_type(default_expr);
                if ty != Type::Unknown
                    && default_ty != Type::Unknown
                    && !self.registry.is_subtype_of(&default_ty, &ty)
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1303] Default value type mismatch for parameter '{}': expected {}, got {}. Hint: Make the default expression assignable to the parameter type.",
                            param.name, ty, default_ty
                        ),
                        span: param.span.clone(),
                    });
                }
            }

            self.define_var(&param.name, ty);
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
        match expr {
            // B11B-016: TypeExtends does not accept enum variant literals
            Expr::MoldInst(name, type_args, fields, _) => {
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
                for arg in type_args {
                    self.check_mold_errors_in_expr(arg);
                }
                for f in fields {
                    self.check_mold_errors_in_expr(&f.value);
                }
            }
            Expr::FuncCall(callee, args, _) => {
                self.check_mold_errors_in_expr(callee);
                for arg in args {
                    self.check_mold_errors_in_expr(arg);
                }
            }
            Expr::MethodCall(obj, _, args, _) => {
                self.check_mold_errors_in_expr(obj);
                for arg in args {
                    self.check_mold_errors_in_expr(arg);
                }
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    self.check_mold_errors_in_expr(e);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.check_mold_errors_in_expr(cond);
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
                    self.check_mold_errors_in_expr(&f.value);
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    self.check_mold_errors_in_expr(item);
                }
            }
            Expr::UnaryOp(_, inner, _) => self.check_mold_errors_in_expr(inner),
            Expr::BinaryOp(l, _, r, _) => {
                self.check_mold_errors_in_expr(l);
                self.check_mold_errors_in_expr(r);
            }
            Expr::Throw(inner, _) => self.check_mold_errors_in_expr(inner),
            Expr::FieldAccess(obj, _, _) => self.check_mold_errors_in_expr(obj),
            Expr::Lambda(_, body, _) => self.check_mold_errors_in_expr(body),
            // Leaf expressions — no recursion needed
            _ => {}
        }
    }

    // C12-2c: Walk an expression subtree and emit E1508 for any
    // `.toString(args)` call with a non-empty argument list. Scoped
    // narrowly so that builtin arg contexts (e.g. `stdout(...)`) still
    // reject `.toString(16)` without otherwise changing type inference
    // for those args (avoids triggering E1510 on callable-variable
    // sites and E1602 on Error-type `__type` field access).
    fn check_tostring_arity_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::MethodCall(obj, method, args, span) => {
                self.check_tostring_arity_in_expr(obj);
                for arg in args {
                    self.check_tostring_arity_in_expr(arg);
                }
                if method == "toString" && !args.is_empty() {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1508] Method 'toString' takes 0 argument(s), got {}. \
                             Hint: `.toString()` takes no arguments — use `Str[value]()` or a \
                             format helper if you need radix/precision control.",
                            args.len()
                        ),
                        span: span.clone(),
                    });
                }
            }
            Expr::FuncCall(callee, args, _) => {
                self.check_tostring_arity_in_expr(callee);
                for arg in args {
                    self.check_tostring_arity_in_expr(arg);
                }
            }
            Expr::MoldInst(_, type_args, fields, _) => {
                for arg in type_args {
                    self.check_tostring_arity_in_expr(arg);
                }
                for f in fields {
                    self.check_tostring_arity_in_expr(&f.value);
                }
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for f in fields {
                    self.check_tostring_arity_in_expr(&f.value);
                }
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    self.check_tostring_arity_in_expr(e);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.check_tostring_arity_in_expr(cond);
                    }
                    for s in &arm.body {
                        if let Statement::Expr(e) = s {
                            self.check_tostring_arity_in_expr(e);
                        }
                    }
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    self.check_tostring_arity_in_expr(item);
                }
            }
            Expr::UnaryOp(_, inner, _) => self.check_tostring_arity_in_expr(inner),
            Expr::BinaryOp(l, _, r, _) => {
                self.check_tostring_arity_in_expr(l);
                self.check_tostring_arity_in_expr(r);
            }
            Expr::Throw(inner, _) => self.check_tostring_arity_in_expr(inner),
            Expr::FieldAccess(obj, _, _) => self.check_tostring_arity_in_expr(obj),
            Expr::Lambda(_, body, _) => self.check_tostring_arity_in_expr(body),
            _ => {}
        }
    }

    /// C19B-002: narrow walker that triggers full type inference only on
    /// FieldAccess nodes inside builtin call arguments (e.g.
    /// `stdout(r.__value.stdout)`). This lets us surface pinned-Gorillax
    /// field-access rejections without retroactively tightening other
    /// builtin arg subtrees (BinaryOp / MethodCall / etc.) that pre-C19
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

    /// Type-check a statement (second pass).
    fn check_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::EnumDef(_) => {}
            Statement::Assignment(assign) => {
                let inferred = self.infer_expr_type(&assign.value);

                // If there's a type annotation, check compatibility
                if let Some(type_ann) = &assign.type_annotation {
                    let expected = self.registry.resolve_type(type_ann);
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
                    self.define_var_with_span(&assign.target, expected, Some(&assign.span));
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
                    self.define_var_with_span(&assign.target, inferred, Some(&assign.span));
                }
            }
            Statement::FuncDef(fd) => {
                let ret_ty = fd
                    .return_type
                    .as_ref()
                    .map(|t| self.registry.resolve_type(t))
                    .unwrap_or(Type::Unknown);
                let param_types: Vec<Type> = fd
                    .params
                    .iter()
                    .map(|p| {
                        p.type_annotation
                            .as_ref()
                            .map(|t| self.registry.resolve_type(t))
                            .unwrap_or(Type::Unknown)
                    })
                    .collect();
                // Register the name in scope so duplicate detection still works.
                // Invalid generic functions stay non-callable by using `Unknown`.
                let function_value_ty = if self.invalid_func_defs.contains(&fd.name) {
                    Type::Unknown
                } else {
                    Type::Function(param_types.clone(), Box::new(ret_ty.clone()))
                };
                self.define_var_with_span(&fd.name, function_value_ty, Some(&fd.span));

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
                if has_return_check {
                    let last_stmt = &fd.body[body_len - 1];
                    let body_ty_opt = match last_stmt {
                        Statement::Expr(last_expr) => Some(self.infer_expr_type(last_expr)),
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
                                     Hint: The function body's last statement must be an expression or a tail binding (`name <= expr`, `expr => name`, `expr ]=> name`, `name <=[ expr`) that produces a value.",
                                    fd.name, ret_ty
                                ),
                                span: fd.span.clone(),
                            });
                        }
                    }
                }

                // D28B-023 / D28B-024: balance the type-param stack push above.
                self.current_func_type_params.pop();
                self.pop_scope();
            }
            Statement::Expr(expr) => {
                self.infer_expr_type(expr);
            }
            Statement::ErrorCeiling(ec) => {
                // Push scope for error handler
                self.push_scope();

                // Register the error parameter
                let err_ty = self.registry.resolve_type(&ec.error_type);
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
                                         `expr => name`, `expr ]=> name`, `name <=[ expr`) \
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
                // C19B-002: pin typed signatures for select `taida-lang/os`
                // symbols (runInteractive / execShellInteractive) so that
                // field access through their Gorillax result resolves at
                // compile time. Unpinned os symbols still fall through to
                // `Type::Unknown` below.
                let os_import = imp.path == "taida-lang/os";
                // Register imported symbols as Unknown
                // (We don't have cross-module type info yet)
                for sym in &imp.symbols {
                    let name = sym.alias.as_ref().unwrap_or(&sym.name);
                    if imp.path == "taida-lang/net" {
                        self.register_net_import_symbol(&sym.name, name);
                    }
                    if os_import {
                        self.register_os_import_symbol(&sym.name, name);
                    }
                    self.define_var(name, Type::Unknown);
                }
            }
            Statement::UnmoldForward(uf) => {
                // `expr ]=> target` -- target gets the unmolded (inner) value
                let source_ty = self.infer_expr_type(&uf.source);
                let target_ty = self.unmold_type(&source_ty);
                self.define_var_with_span(&uf.target, target_ty, Some(&uf.span));
            }
            Statement::UnmoldBackward(ub) => {
                // `target <=[ expr`
                let source_ty = self.infer_expr_type(&ub.source);
                let target_ty = self.unmold_type(&source_ty);
                self.define_var_with_span(&ub.target, target_ty, Some(&ub.span));
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

    /// Infer the type of an expression.
    pub fn infer_expr_type(&mut self, expr: &Expr) -> Type {
        match expr {
            Expr::IntLit(_, _) => Type::Int,
            Expr::FloatLit(_, _) => Type::Float,
            Expr::StringLit(_, _) => Type::Str,
            Expr::TemplateLit(_, _) => Type::Str,
            Expr::BoolLit(_, _) => Type::Bool,
            Expr::Gorilla(_) => Type::Unit,
            Expr::Placeholder(_) => Type::Unknown,
            Expr::Hole(_) => Type::Unknown,
            // B11-6a: TypeLiteral is a compile-time type reference, not a value
            Expr::TypeLiteral(_, _, _) => Type::Str,

            Expr::Ident(name, span) => {
                // Look up variable in scope
                if let Some(ty) = self.lookup_var(name) {
                    ty
                } else if self.func_types.contains_key(name)
                    || self.generic_func_defs.contains_key(name)
                    || self.declared_concrete_type_names.contains(name)
                    || self.registry.mold_defs.contains_key(name)
                    || matches!(
                        name.as_str(),
                        "debug"
                            | "toString"
                            | "toStr"
                            | "typeOf"
                            | "typeof"
                            | "jsonEncode"
                            | "jsonPretty"
                            | "nowMs"
                            | "assert"
                            | "range"
                            | "enumerate"
                            | "zip"
                            | "hashMap"
                            | "setOf"
                            | "stdout"
                            | "stderr"
                            | "exit"
                            | "stdin"
                            | "stdinLine"
                            | "argv"
                            | "sleep"
                    )
                {
                    // Known function/type/mold name used as value reference
                    Type::Unknown
                } else {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1502] Undefined variable '{}'. \
                             Hint: Check the variable name for typos, or define it before use.",
                            name
                        ),
                        span: span.clone(),
                    });
                    Type::Unknown
                }
            }

            Expr::BuchiPack(fields, _) => {
                let field_types: Vec<(String, Type)> = fields
                    .iter()
                    .map(|f| {
                        let ty = self.infer_expr_type(&f.value);
                        (f.name.clone(), ty)
                    })
                    .collect();
                Type::BuchiPack(field_types)
            }

            Expr::ListLit(items, span) => {
                if items.is_empty() {
                    Type::List(Box::new(Type::Unknown))
                } else {
                    let first_type = self.infer_expr_type(&items[0]);
                    // リスト要素の同質性チェック (E0401)
                    // Int/Float 混在は Num に統一
                    let mut unified_type = first_type.clone();
                    for (i, item) in items.iter().enumerate().skip(1) {
                        let item_type = self.infer_expr_type(item);
                        if Self::contains_unknown(&item_type)
                            || Self::contains_unknown(&unified_type)
                        {
                            // Unknown を含む型は型推論未完了 — スキップ
                            // unified_type が Unknown で item_type が具体型なら更新
                            if unified_type == Type::Unknown && item_type != Type::Unknown {
                                unified_type = item_type;
                            }
                            continue;
                        }
                        // Int/Float の混在は Num に統一
                        if (unified_type == Type::Int
                            || unified_type == Type::Float
                            || unified_type == Type::Num)
                            && item_type.is_numeric()
                        {
                            if unified_type != item_type {
                                unified_type = Type::Num;
                            }
                            continue;
                        }
                        // BuchiPack 同士は構造的部分型なので許容
                        if matches!(unified_type, Type::BuchiPack(_))
                            && matches!(item_type, Type::BuchiPack(_))
                        {
                            continue;
                        }
                        // 型不一致
                        if item_type != unified_type {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E0401] リスト要素の型が不一致: 先頭要素は {} ですが、位置 {} の要素は {} です",
                                    first_type, i, item_type
                                ),
                                span: span.clone(),
                            });
                            break;
                        }
                    }
                    Type::List(Box::new(unified_type))
                }
            }

            Expr::BinaryOp(left, op, right, span) => {
                let left_type = self.infer_expr_type(left);
                let right_type = self.infer_expr_type(right);
                // D28B-024: a Type::Named(T) where T is an active generic
                // type parameter constrained by a numeric primitive
                // (`T <= :Num` / `:Int` / `:Float`) should be treated as
                // numeric for arithmetic and ordering. Helper closures
                // capture this judgement uniformly across operator arms.
                let left_is_numeric_var =
                    matches!(&left_type, Type::Named(n) if self.type_param_is_numeric(n));
                let right_is_numeric_var =
                    matches!(&right_type, Type::Named(n) if self.type_param_is_numeric(n));
                let left_is_numeric_ext = left_type.is_numeric() || left_is_numeric_var;
                let right_is_numeric_ext = right_type.is_numeric() || right_is_numeric_var;
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul => {
                        if left_is_numeric_ext && right_is_numeric_ext {
                            // D28B-024: when both operands are the SAME
                            // generic numeric type variable, preserve it
                            // (so a body declared `=> :T` type-checks
                            // against the body's tail value of type T).
                            // Mixed `T` + concrete numeric, or two
                            // different numeric type variables, widen to
                            // a concrete numeric primitive (Int / Float)
                            // following the existing precedence rule:
                            // any Float operand widens to Float, else Int.
                            if let (Type::Named(l), Type::Named(r)) = (&left_type, &right_type)
                                && l == r
                                && self.type_param_is_numeric(l)
                            {
                                left_type.clone()
                            } else if matches!(left_type, Type::Float)
                                || matches!(right_type, Type::Float)
                            {
                                Type::Float
                            } else if left_is_numeric_var || right_is_numeric_var {
                                // At least one side is a generic numeric
                                // var; cannot statically pick Int vs Float
                                // without inference at the call site, so
                                // surface as Num and let return-type
                                // compatibility (subtype: Int<:Num,
                                // Float<:Num) absorb the imprecision.
                                Type::Num
                            } else {
                                Type::Int
                            }
                        } else if matches!(op, BinOp::Add)
                            && matches!(left_type, Type::Str)
                            && matches!(right_type, Type::Str)
                        {
                            Type::Str
                        } else if left_type == Type::Unknown || right_type == Type::Unknown {
                            Type::Unknown
                        } else {
                            self.errors.push(TypeError {
                                message: format!(
                                    "Cannot apply {:?} to {} and {}",
                                    op, left_type, right_type
                                ),
                                span: span.clone(),
                            });
                            Type::Unknown
                        }
                    }
                    BinOp::Eq | BinOp::NotEq => {
                        // FL-4: Equality operators allow any types but warn on incompatible comparisons
                        if left_type != Type::Unknown
                            && right_type != Type::Unknown
                            && !Self::contains_unknown(&left_type)
                            && !Self::contains_unknown(&right_type)
                            && left_type != right_type
                            && !(left_type.is_numeric() && right_type.is_numeric())
                            // D28B-024: equality between generic numeric
                            // type variables (or such a variable and a
                            // concrete numeric primitive) is allowed.
                            && !(left_is_numeric_ext && right_is_numeric_ext)
                            // Allow structurally compatible types (e.g. BuchiPack subtypes)
                            && !self.registry.is_subtype_of(&left_type, &right_type)
                            && !self.registry.is_subtype_of(&right_type, &left_type)
                        {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1605] Cannot compare {} with {} using {:?}. \
                                     Hint: Both operands should be of compatible types.",
                                    left_type, right_type, op
                                ),
                                span: span.clone(),
                            });
                        }
                        Type::Bool
                    }
                    BinOp::Lt | BinOp::Gt | BinOp::GtEq => {
                        // FL-4: Ordering operators require numeric or string operands.
                        //
                        // C18-4: Same-Enum ordering is also allowed. `>=` / `<=` /
                        // `<` / `>` compare the declared ordinal position of the
                        // two variants. Cross-Enum and Enum↔Int ordering stays
                        // rejected with `[E1605]` — use `Ordinal[]` to obtain
                        // the Int explicitly (added in C18-3). The declared
                        // order of an Enum is therefore semantic; see
                        // `docs/guide/01_types.md` for the author contract.
                        if left_type != Type::Unknown
                            && right_type != Type::Unknown
                            && !Self::contains_unknown(&left_type)
                            && !Self::contains_unknown(&right_type)
                        {
                            let both_numeric = left_type.is_numeric() && right_type.is_numeric();
                            let both_str =
                                matches!(left_type, Type::Str) && matches!(right_type, Type::Str);
                            let same_enum = match (&left_type, &right_type) {
                                (Type::Named(a), Type::Named(b)) => {
                                    a == b && self.registry.is_enum_type(a)
                                }
                                _ => false,
                            };
                            // D28B-024: ordering between generic numeric
                            // type variables (or such a variable and a
                            // concrete numeric primitive) is allowed.
                            let both_numeric_ext = left_is_numeric_ext && right_is_numeric_ext;
                            let valid = both_numeric || both_numeric_ext || both_str || same_enum;
                            if !valid {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1605] Cannot compare {} with {} using {:?}. \
                                         Hint: Ordering comparison requires numeric, string, or same-Enum operands. \
                                         For Enum↔Int comparisons use `Ordinal[<enum>]()` to obtain the Int first.",
                                        left_type, right_type, op
                                    ),
                                    span: span.clone(),
                                });
                            }
                        }
                        Type::Bool
                    }
                    BinOp::And | BinOp::Or => {
                        // FL-4: Logical operators require Bool operands
                        if left_type != Type::Unknown
                            && !Self::contains_unknown(&left_type)
                            && !matches!(left_type, Type::Bool)
                        {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1606] Logical operator {:?} requires Bool operands, got {} on left side. \
                                     Hint: Use a boolean expression or comparison.",
                                    op, left_type
                                ),
                                span: span.clone(),
                            });
                        }
                        if right_type != Type::Unknown
                            && !Self::contains_unknown(&right_type)
                            && !matches!(right_type, Type::Bool)
                        {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1606] Logical operator {:?} requires Bool operands, got {} on right side. \
                                     Hint: Use a boolean expression or comparison.",
                                    op, right_type
                                ),
                                span: span.clone(),
                            });
                        }
                        Type::Bool
                    }
                    BinOp::Concat => Type::Str,
                }
            }

            Expr::UnaryOp(op, inner, span) => {
                let inner_type = self.infer_expr_type(inner);
                match op {
                    UnaryOp::Neg => {
                        if inner_type.is_numeric() || inner_type == Type::Unknown {
                            inner_type
                        } else {
                            // FL-4: Report non-numeric operand for unary negation
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1607] Unary negation `-` requires a numeric operand, got {}. \
                                     Hint: Use `-` only with Int or Float values.",
                                    inner_type
                                ),
                                span: span.clone(),
                            });
                            Type::Unknown
                        }
                    }
                    UnaryOp::Not => {
                        // FL-4: Not operator requires Bool operand
                        if inner_type != Type::Unknown
                            && !Self::contains_unknown(&inner_type)
                            && !matches!(inner_type, Type::Bool)
                        {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1607] Logical not `!` requires a Bool operand, got {}. \
                                     Hint: Use `!` only with boolean expressions.",
                                    inner_type
                                ),
                                span: span.clone(),
                            });
                        }
                        Type::Bool
                    }
                }
            }

            Expr::FuncCall(func, args, span) => {
                // C-5c: Reject old `_` partial application syntax in function call args.
                // Pipeline context (`data => f(_)`) is allowed — `_` refers to pipe value.
                if !self.in_pipeline {
                    for arg in args.iter() {
                        if let Expr::Placeholder(ph_span) = arg {
                            self.errors.push(TypeError {
                                message: "[E1502] Use empty slot syntax `f(5, )` instead of `f(5, _)` for partial application. \
                                     Hint: Remove the `_` and leave the argument position empty.".to_string(),
                                span: ph_span.clone(),
                            });
                        }
                    }
                }

                // C-5d: Reject partial application (Placeholder or Hole) in TypeDef/BuchiPack instantiation.
                // TypeDef calls look like FuncCall where callee is an uppercase Ident.
                if let Expr::Ident(callee_name, _) = func.as_ref()
                    && callee_name.chars().next().is_some_and(|c| c.is_uppercase())
                    && !self.func_types.contains_key(callee_name.as_str())
                {
                    // This is likely a TypeDef instantiation
                    for arg in args.iter() {
                        match arg {
                            Expr::Placeholder(ph_span) => {
                                self.errors.push(TypeError {
                                        message: "[E1503] Partial application is not supported for TypeDef/BuchiPack instantiation. \
                                             Hint: Provide all fields explicitly when creating a TypeDef instance.".to_string(),
                                        span: ph_span.clone(),
                                    });
                            }
                            Expr::Hole(h_span) => {
                                self.errors.push(TypeError {
                                        message: "[E1503] Partial application is not supported for TypeDef/BuchiPack instantiation. \
                                             Hint: Provide all fields explicitly when creating a TypeDef instance.".to_string(),
                                        span: h_span.clone(),
                                    });
                            }
                            _ => {}
                        }
                    }
                }

                // Count holes in the argument list
                let hole_count = args.iter().filter(|a| matches!(a, Expr::Hole(_))).count();

                // Try to resolve return type from function name
                if let Expr::Ident(name, _) = func.as_ref() {
                    self.validate_http_serve_protocol_capability(name, args);

                    if let Some(fd) = self.generic_func_defs.get(name).cloned() {
                        let param_patterns: Vec<Type> = fd
                            .params
                            .iter()
                            .map(|param| {
                                param
                                    .type_annotation
                                    .as_ref()
                                    .map(|ty| self.registry.resolve_type(ty))
                                    .unwrap_or(Type::Unknown)
                            })
                            .collect();
                        let ret_pattern = fd
                            .return_type
                            .as_ref()
                            .map(|ty| self.registry.resolve_type(ty))
                            .unwrap_or(Type::Unknown);
                        let generic_names: HashSet<String> =
                            fd.type_params.iter().map(|tp| tp.name.clone()).collect();
                        let mut bindings = HashMap::<String, Type>::new();

                        if args.len() > fd.params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1301] Function '{}' takes at most {} argument(s), got {}. Hint: Remove extra arguments or update the function signature.",
                                    name,
                                    fd.params.len(),
                                    args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        if hole_count > 0 && args.len() != fd.params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] Partial application of '{}' requires exactly {} slot(s) (got {}). \
                                     Hint: Provide a value or empty slot for each parameter.",
                                    name,
                                    fd.params.len(),
                                    args.len()
                                ),
                                span: span.clone(),
                            });
                        }

                        for (i, arg) in args.iter().enumerate() {
                            if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                continue;
                            }
                            let Some(pattern) = param_patterns.get(i) else {
                                continue;
                            };
                            let actual_ty = self.infer_expr_type(arg);
                            if actual_ty == Type::Unknown {
                                continue;
                            }
                            if !self.bind_generic_type_pattern(
                                pattern,
                                &actual_ty,
                                &generic_names,
                                &mut bindings,
                            ) {
                                let expected_ty = self.substitute_generic_type(
                                    pattern,
                                    &generic_names,
                                    &bindings,
                                );
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                         Hint: Pass a value of the correct type, or use an explicit conversion.",
                                        i + 1,
                                        name,
                                        actual_ty,
                                        expected_ty
                                    ),
                                    span: span.clone(),
                                });
                            }
                        }

                        if !self.validate_generic_function_inference(&fd, &bindings, span) {
                            return Type::Unknown;
                        }
                        self.validate_generic_function_bindings(&fd, &bindings, span);
                        let resolved_ret =
                            self.instantiate_generic_type(&ret_pattern, &generic_names, &bindings);

                        if hole_count > 0 {
                            let hole_param_types: Vec<Type> = args
                                .iter()
                                .enumerate()
                                .filter(|(_, arg)| matches!(arg, Expr::Hole(_)))
                                .map(|(i, _)| {
                                    param_patterns
                                        .get(i)
                                        .map(|pattern| {
                                            self.instantiate_generic_type(
                                                pattern,
                                                &generic_names,
                                                &bindings,
                                            )
                                        })
                                        .unwrap_or(Type::Unknown)
                                })
                                .collect();
                            return Type::Function(hole_param_types, Box::new(resolved_ret));
                        }

                        return resolved_ret;
                    }

                    // First check func_types (registered function return types)
                    if let Some(ret_ty) = self.func_types.get(name).cloned() {
                        if let Some(expected) = self.func_param_counts.get(name).copied() {
                            if args.len() > expected {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1301] Function '{}' takes at most {} argument(s), got {}. Hint: Remove extra arguments or update the function signature.",
                                        name, expected, args.len()
                                    ),
                                    span: span.clone(),
                                });
                            }
                            // Slot count (args.len()) must equal arity when holes are present
                            if hole_count > 0 && args.len() != expected {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1505] Partial application of '{}' requires exactly {} slot(s) (got {}). \
                                         Hint: Provide a value or empty slot for each parameter.",
                                        name, expected, args.len()
                                    ),
                                    span: span.clone(),
                                });
                            }
                        }
                        // E1506: Check argument types against registered parameter types
                        if let Some(param_types) = self.func_param_types.get(name).cloned() {
                            for (i, arg) in args.iter().enumerate() {
                                // Skip holes (partial application) and placeholders
                                if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                    continue;
                                }
                                if let Some(expected_ty) = param_types.get(i) {
                                    if *expected_ty == Type::Unknown {
                                        continue;
                                    }
                                    let actual_ty = self.infer_expr_type(arg);
                                    if actual_ty == Type::Unknown {
                                        continue;
                                    }
                                    if !self.registry.is_subtype_of(&actual_ty, expected_ty) {
                                        self.errors.push(TypeError {
                                            message: format!(
                                                "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                                 Hint: Pass a value of the correct type, or use an explicit conversion.",
                                                i + 1, name, actual_ty, expected_ty
                                            ),
                                            span: span.clone(),
                                        });
                                    }
                                }
                            }
                        }
                        // If holes present, return a function type (partial application)
                        if hole_count > 0 {
                            // Use registered param types to infer concrete hole types
                            let registered_param_types = self.func_param_types.get(name);
                            let hole_param_types: Vec<Type> = args
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| matches!(a, Expr::Hole(_)))
                                .map(|(i, _)| {
                                    registered_param_types
                                        .and_then(|pts| pts.get(i).cloned())
                                        .unwrap_or(Type::Unknown)
                                })
                                .collect();
                            return Type::Function(hole_param_types, Box::new(ret_ty));
                        }
                        return ret_ty;
                    }
                    // Check if variable holds a function type
                    if let Some(Type::Function(params, ret)) = self.lookup_var(name) {
                        if args.len() > params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1301] Function value '{}' takes at most {} argument(s), got {}. Hint: Remove extra arguments or adjust the function type.",
                                    name, params.len(), args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        if hole_count > 0 && args.len() != params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] Partial application of '{}' requires exactly {} slot(s) (got {}). \
                                     Hint: Provide a value or empty slot for each parameter.",
                                    name, params.len(), args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        // E1506: Check argument types against function parameter types
                        for (i, arg) in args.iter().enumerate() {
                            if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                continue;
                            }
                            if let Some(expected_ty) = params.get(i) {
                                if *expected_ty == Type::Unknown {
                                    continue;
                                }
                                let actual_ty = self.infer_expr_type(arg);
                                if actual_ty == Type::Unknown {
                                    continue;
                                }
                                if !self.registry.is_subtype_of(&actual_ty, expected_ty) {
                                    self.errors.push(TypeError {
                                        message: format!(
                                            "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                             Hint: Pass a value of the correct type, or use an explicit conversion.",
                                            i + 1, name, actual_ty, expected_ty
                                        ),
                                        span: span.clone(),
                                    });
                                }
                            }
                        }
                        if hole_count > 0 {
                            // Collect the types of the hole positions from the original param types
                            let hole_param_types: Vec<Type> = args
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| matches!(a, Expr::Hole(_)))
                                .map(|(i, _)| params.get(i).cloned().unwrap_or(Type::Unknown))
                                .collect();
                            return Type::Function(hole_param_types, ret);
                        }
                        return *ret;
                    }
                    // D28B-023: variable's declared type is a generic type
                    // parameter whose subtype constraint is a function type
                    // (e.g. `applyFn[T, F <= :T => :T] x: T fn: F = fn(x)`),
                    // so resolving `fn(x)` should dispatch on the constraint's
                    // function shape rather than fall through to [E1510].
                    if let Some(Type::Named(var_name)) = self.lookup_var(name)
                        && let Some(Type::Function(params, ret)) =
                            self.type_param_function_constraint(&var_name)
                    {
                        if args.len() > params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1301] Function value '{}' takes at most {} argument(s), got {}. Hint: Remove extra arguments or adjust the function type.",
                                    name, params.len(), args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        if hole_count > 0 && args.len() != params.len() {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1505] Partial application of '{}' requires exactly {} slot(s) (got {}). \
                                     Hint: Provide a value or empty slot for each parameter.",
                                    name, params.len(), args.len()
                                ),
                                span: span.clone(),
                            });
                        }
                        // E1506: argument type compatibility against the
                        // declared function-constraint params. Skip when
                        // either side mentions an unresolved type variable
                        // (the body of the enclosing generic function does
                        // not bind T to a concrete type yet).
                        for (i, arg) in args.iter().enumerate() {
                            if matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                continue;
                            }
                            let Some(expected_ty) = params.get(i) else {
                                continue;
                            };
                            if *expected_ty == Type::Unknown
                                || self.contains_unresolved_type_var(expected_ty)
                            {
                                continue;
                            }
                            let actual_ty = self.infer_expr_type(arg);
                            if actual_ty == Type::Unknown
                                || self.contains_unresolved_type_var(&actual_ty)
                            {
                                continue;
                            }
                            if !self.registry.is_subtype_of(&actual_ty, expected_ty) {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1506] Argument {} of '{}' has type {}, expected {}. \
                                         Hint: Pass a value of the correct type, or use an explicit conversion.",
                                        i + 1, name, actual_ty, expected_ty
                                    ),
                                    span: span.clone(),
                                });
                            }
                        }
                        if hole_count > 0 {
                            let hole_param_types: Vec<Type> = args
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| matches!(a, Expr::Hole(_)))
                                .map(|(i, _)| params.get(i).cloned().unwrap_or(Type::Unknown))
                                .collect();
                            return Type::Function(hole_param_types, ret);
                        }
                        return *ret;
                    }
                    // FL-23: Check if variable is a non-function type being called
                    if let Some(var_ty) = self.lookup_var(name)
                        && !matches!(var_ty, Type::Unknown)
                    {
                        // D28B-023: when the rejected variable's type is an
                        // active generic type parameter (a Named type that
                        // is not registered as a concrete type) but it has
                        // no function-type constraint, the user likely
                        // declared `[T] x: T fn: T` (no constraint) or
                        // `[T, F <= :SomeNonFunction] fn: F`. Append a
                        // targeted hint so the diagnostic guides toward
                        // adding `<= :A => :B` or providing the function
                        // type at the call site.
                        let hint_extra = match &var_ty {
                            Type::Named(n) if self.lookup_active_type_param(n).is_some() => {
                                " For higher-order generic functions, declare the \
                                 callable with a function-type constraint, e.g. \
                                 `[T, F <= :T => :T] x: T fn: F = fn(x) => :T`."
                            }
                            _ => "",
                        };
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1510] Cannot call '{}' of type {} as a function. \
                                 Hint: Only functions and molds can be called.{}",
                                name, var_ty, hint_extra
                            ),
                            span: span.clone(),
                        });
                        return Type::Unknown;
                    }
                    // Check if it's a known builtin
                    // E1507: Builtin arity check
                    // (name, min_args, max_args)
                    let builtin_arity: Option<(usize, usize)> = match name.as_str() {
                        "debug" => Some((1, 2)), // debug(value) or debug(label, value)
                        "toString" | "toStr" => Some((1, 1)),
                        "typeOf" | "typeof" => Some((1, 1)),
                        "jsonEncode" | "jsonPretty" => Some((1, 1)),
                        "nowMs" => Some((0, 0)),
                        "assert" => Some((1, 2)), // assert(cond) or assert(cond, msg)
                        "range" => Some((2, 3)),  // range(start, end) or range(start, end, step)
                        "enumerate" => Some((1, 1)),
                        "zip" => Some((2, 2)),
                        "hashMap" => Some((0, 1)),
                        "setOf" => Some((1, 1)),
                        "stdout" => Some((1, 1)),
                        "stderr" => Some((1, 1)),
                        "exit" => Some((1, 1)),
                        // C20-3 (ROOT-13): prompt is optional. The prelude
                        // runtime, Native lowering and LSP / docs all treat
                        // `stdin()` (no-prompt) as valid. Before C20 the
                        // checker rejected it with [E1507].
                        "stdin" => Some((0, 1)),
                        // C20-2: stdinLine is the UTF-8-aware successor to
                        // `stdin`. Prompt is optional; result is
                        // `Async[Lax[Str]]` and callers must unmold via
                        // `]=>` to get the inner `Lax[Str]`.
                        "stdinLine" => Some((0, 1)),
                        "argv" => Some((0, 0)),
                        "sleep" => Some((1, 1)),
                        // C12 Phase 6 (FB-5): Regex(pattern, flags?)
                        // returns a :Regex BuchiPack.
                        "Regex" => Some((1, 2)),
                        _ => None,
                    };
                    if let Some((min_args, max_args)) = builtin_arity
                        && (args.len() < min_args || args.len() > max_args)
                    {
                        let arity_desc = if min_args == max_args {
                            format!("{}", min_args)
                        } else {
                            format!("{}-{}", min_args, max_args)
                        };
                        self.errors.push(TypeError {
                                message: format!(
                                    "[E1507] Builtin '{}' takes {} argument(s), got {}. \
                                     Hint: Check the function signature and provide the correct number of arguments.",
                                    name, arity_desc, args.len()
                                ),
                                span: span.clone(),
                            });
                    }
                    // C12-2c: walk builtin args specifically for
                    // `.toString(args)` arity violations so that nested
                    // method calls inside (e.g.) `stdout(n.toString(16))`
                    // are still rejected. Scoped narrowly to `toString`
                    // to avoid changing type-inference semantics for
                    // other builtin arg contexts.
                    //
                    // C19B-002: also walk for FieldAccess nodes whose
                    // receiver type is a pinned Gorillax (or reducible
                    // to one) so that `.__value.<bogus>` chains inside
                    // builtin args surface the same E1602-style rejection
                    // as when assigned to a variable. We deliberately do
                    // NOT recurse into BinaryOp / arithmetic subtrees: that
                    // would retroactively surface pre-existing Str+Int
                    // tolerance in examples like `"foo" + lax.getOrDefault(0)`.
                    if builtin_arity.is_some() && name != "debug" {
                        for arg in args.iter() {
                            if !matches!(arg, Expr::Hole(_) | Expr::Placeholder(_)) {
                                self.check_tostring_arity_in_expr(arg);
                                self.check_pinned_field_access_in_expr(arg);
                            }
                        }
                    }
                    let base_ty = match name.as_str() {
                        // debug returns its argument (pass-through)
                        "debug" => {
                            if let Some(first_arg) = args.first() {
                                self.infer_expr_type(first_arg)
                            } else {
                                Type::Unit
                            }
                        }
                        "toString" | "toStr" => Type::Str,
                        "typeOf" | "typeof" => Type::Str,
                        "jsonEncode" | "jsonPretty" => Type::Str,
                        "nowMs" => Type::Int,
                        // Prelude functions
                        "assert" => Type::Unit,
                        "range" => Type::List(Box::new(Type::Int)),
                        "enumerate" => Type::List(Box::new(Type::Unknown)),
                        "zip" => Type::List(Box::new(Type::Unknown)),
                        "hashMap" => Type::Named("HashMap".to_string()),
                        "setOf" => Type::Named("Set".to_string()),
                        // C12-5 (FB-18): stdout/stderr now return Int (bytes
                        // written) instead of Unit so that `Value::Unit` stays
                        // unobservable from Taida surface. `exit` remains Unit
                        // because it never returns (process terminates).
                        "stdout" | "stderr" => Type::Int,
                        "exit" => Type::Unit,
                        "stdin" => Type::Str,
                        // C20-2: `stdinLine` pins its result to
                        // `Async[Lax[Str]]` so that callers are forced to
                        // unmold via `]=>` and then reason about the Lax
                        // (failure on EOF / IO error returns the default
                        // `""`). Direct `<=` binding leaves the Async in
                        // place — the Lax is not reachable without an
                        // unmold, which matches Taida's Async discipline.
                        "stdinLine" => Type::Generic(
                            "Async".to_string(),
                            vec![Type::Generic("Lax".to_string(), vec![Type::Str])],
                        ),
                        "argv" => Type::List(Box::new(Type::Str)),
                        "sleep" => Type::Generic("Async".to_string(), vec![Type::Unit]),
                        // C12 Phase 6 (FB-5): Regex(pattern, flags?)
                        // returns an opaque named :Regex type (internally
                        // a BuchiPack with `__type <= "Regex"`).
                        "Regex" => Type::Named("Regex".to_string()),
                        _ => Type::Unknown,
                    };
                    if hole_count > 0 {
                        let hole_param_types: Vec<Type> =
                            (0..hole_count).map(|_| Type::Unknown).collect();
                        return Type::Function(hole_param_types, Box::new(base_ty));
                    }
                    base_ty
                } else {
                    // Calling a non-ident expression (e.g. lambda call)
                    let func_type = self.infer_expr_type(func);
                    match func_type {
                        Type::Function(params, ret) => {
                            if args.len() > params.len() {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1301] Function call takes at most {} argument(s), got {}. Hint: Remove extra arguments or adjust the callee signature.",
                                        params.len(), args.len()
                                    ),
                                    span: span.clone(),
                                });
                            }
                            if hole_count > 0 && args.len() != params.len() {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1505] Partial application requires exactly {} slot(s) (got {}). \
                                         Hint: Provide a value or empty slot for each parameter.",
                                        params.len(), args.len()
                                    ),
                                    span: span.clone(),
                                });
                            }
                            if hole_count > 0 {
                                let hole_param_types: Vec<Type> = args
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, a)| matches!(a, Expr::Hole(_)))
                                    .map(|(i, _)| params.get(i).cloned().unwrap_or(Type::Unknown))
                                    .collect();
                                return Type::Function(hole_param_types, ret);
                            }
                            *ret
                        }
                        Type::Unknown => Type::Unknown,
                        _ => {
                            // FL-23: non-function call
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1510] Cannot call non-function value of type {}. \
                                     Hint: Only functions and molds can be called.",
                                    func_type
                                ),
                                span: span.clone(),
                            });
                            Type::Unknown
                        }
                    }
                }
            }

            Expr::MethodCall(obj, method, args, span) => {
                let obj_type = self.infer_expr_type(obj);
                // E1508: Method call argument count and type checking
                self.check_method_args(&obj_type, method, args, span);
                self.infer_method_return_type(&obj_type, method)
            }

            Expr::FieldAccess(obj, field, span) => {
                let obj_type = self.infer_expr_type(obj);
                match &obj_type {
                    Type::BuchiPack(fields) => {
                        if let Some((_, ty)) = fields.iter().find(|(name, _)| name == field) {
                            ty.clone()
                        } else {
                            self.errors.push(TypeError {
                                message: format!(
                                    "Field '{}' does not exist on type {}",
                                    field, obj_type
                                ),
                                span: span.clone(),
                            });
                            Type::Unknown
                        }
                    }
                    Type::Named(type_name) => {
                        // Look up field in registered type definition
                        if let Some(fields) = self.registry.get_type_fields(type_name) {
                            if let Some((_, ty)) = fields.iter().find(|(name, _)| name == field) {
                                ty.clone()
                            } else if field.starts_with("__") {
                                // C19B-002: skip compiler-internal `__*` fields
                                // (`__type`, `__value`, `__error`, `__default`,
                                // etc.). Error-derived packs surface these at
                                // runtime even when they aren't in the user's
                                // explicit field list, and flagging them here
                                // would regress existing patterns like
                                // `err.__type` on Error-inheriting named types.
                                Type::Unknown
                            } else {
                                // FL-2: Report undefined field access on Named types
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1602] Field '{}' does not exist on type '{}'. \
                                         Hint: Check the type definition for available fields.",
                                        field, type_name
                                    ),
                                    span: span.clone(),
                                });
                                Type::Unknown
                            }
                        } else {
                            Type::Unknown
                        }
                    }
                    // C19B-002: Gorillax / RelaxedGorillax `__value` / `__error`
                    // dot-access. When the Gorillax is parameterized — e.g.
                    // `runInteractive` → `Gorillax[@(code: Int)]` — accessing
                    // `.__value` must yield the inner BuchiPack so that a
                    // further `.stdout` / `.bogus` access is rejected by the
                    // BuchiPack branch above.
                    //
                    // `hasValue` is always Bool. `__error` is deliberately
                    // returned as Unknown because the error inner shape
                    // (IoError / ProcessError) is heterogeneous and not pinned
                    // by this checker; falling back to Unknown keeps existing
                    // `r.__error.code` / `.kind` callers compiling.
                    Type::Generic(name, args)
                        if name == "Gorillax" || name == "RelaxedGorillax" =>
                    {
                        match field.as_str() {
                            "__value" => args.first().cloned().unwrap_or(Type::Unknown),
                            "hasValue" => Type::Bool,
                            "__error" | "throw" | "__type" => Type::Unknown,
                            _ => {
                                // Only surface an error for fields that are
                                // clearly not Gorillax envelope slots. Unknown
                                // user-level names fall through to Unknown so
                                // we don't regress any callers that treat a
                                // Gorillax as a dyn pack on purpose.
                                Type::Unknown
                            }
                        }
                    }
                    Type::Unknown => Type::Unknown,
                    _ => Type::Unknown,
                }
            }

            // IndexAccess removed in v0.5.0 — use .get(i) instead
            Expr::CondBranch(arms, span) => self.check_cond_branch(arms, span),

            Expr::Pipeline(exprs, _) => {
                // Pipeline: walk all expressions, set in_pipeline for non-first elements.
                //
                // C13-1 / C13B-007: In a pure `=>` pipeline, an intermediate
                // `=> name` step acts as a bind-and-forward: the value of
                // the preceding step is bound to `name` in a scope that
                // covers the remaining pipeline steps. When the intermediate
                // step is an `Expr::Ident(name)` and `name` is NOT already a
                // known function / type / mold / builtin, we register it as
                // a local binding carrying the current step's type rather
                // than reporting `[E1502] Undefined variable`.
                let old_in_pipeline = self.in_pipeline;
                let last_idx = exprs.len().saturating_sub(1);
                // A fresh scope holds any intermediate bind-and-forward bindings.
                self.push_scope();
                let mut result_type = Type::Unknown;
                for (i, pipe_expr) in exprs.iter().enumerate() {
                    if i > 0 {
                        self.in_pipeline = true;
                    }
                    if i > 0
                        && i < last_idx
                        && let Expr::Ident(name, _) = pipe_expr
                        && !self.is_pipeline_callable_ident(name)
                    {
                        // Intermediate bind-and-forward: carry the current
                        // step's type and make `name` visible to later steps.
                        // result_type is unchanged (value passes through).
                        self.define_var(name, result_type.clone());
                        continue;
                    }
                    result_type = self.infer_expr_type(pipe_expr);
                }
                self.pop_scope();
                self.in_pipeline = old_in_pipeline;
                result_type
            }

            Expr::MoldInst(name, type_args, fields, mold_span) => {
                // C-5e: Reject Mold[_]() direct binding outside pipeline.
                // In pipeline (`data => Trim[_]()`), `_` refers to the pipe value — allowed.
                if !self.in_pipeline {
                    for arg in type_args.iter() {
                        if let Expr::Placeholder(ph_span) = arg {
                            self.errors.push(TypeError {
                                message: format!(
                                    "[E1504] `{}[_]()` cannot be used outside a pipeline. \
                                     The `_` placeholder in mold type arguments is only valid inside a pipeline expression (`data => {}[_]()`). \
                                     Hint: Pass a concrete value to the mold, e.g., `{}[value]()`.",
                                    name, name, name
                                ),
                                span: ph_span.clone(),
                            });
                        }
                    }
                }

                self.validate_custom_mold_inst_bindings(name, type_args, fields, mold_span);
                self.validate_mold_header_constraints(name, type_args, mold_span);
                match name.as_str() {
                    // JSON[raw, Schema]() returns Lax (wrapping the schema type)
                    "JSON" => Type::Generic("Lax".to_string(), vec![Type::Unknown]),
                    // Async[T] wraps a value
                    "Async" => Type::Generic(
                        "Async".to_string(),
                        vec![
                            type_args
                                .first()
                                .map(|a| self.infer_expr_type(a))
                                .unwrap_or(Type::Unknown),
                        ],
                    ),
                    // Cancel[async]() returns Async[T] (or Async[Unknown] fallback)
                    "Cancel" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .map(|t| match t {
                                Type::Generic(name, args) if name == "Async" => {
                                    args.first().cloned().unwrap_or(Type::Unknown)
                                }
                                other => other,
                            })
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Async".to_string(), vec![inner])
                    }
                    // Result[value, predicate] returns Result type
                    "Result" => Type::Generic(
                        "Result".to_string(),
                        vec![
                            type_args
                                .first()
                                .map(|a| self.infer_expr_type(a))
                                .unwrap_or(Type::Unknown),
                            Type::Unknown, // predicate type
                        ],
                    ),
                    // Lax[value]() returns Lax[T]
                    "Lax" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Lax".to_string(), vec![inner])
                    }
                    // Div[x, y]() and Mod[x, y]() return Lax[Num]
                    "Div" | "Mod" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Num);
                        let inner = if inner.is_numeric() { inner } else { Type::Num };
                        Type::Generic("Lax".to_string(), vec![inner])
                    }
                    // Type conversion molds: Str[x]() -> Lax[Str], Int[x]() -> Lax[Int], etc.
                    "Str" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                    "Int" => Type::Generic("Lax".to_string(), vec![Type::Int]),
                    "Float" => Type::Generic("Lax".to_string(), vec![Type::Float]),
                    "Bool" => Type::Generic("Lax".to_string(), vec![Type::Bool]),
                    "Bytes" => Type::Generic("Lax".to_string(), vec![Type::Bytes]),
                    "UInt8" => Type::Generic("Lax".to_string(), vec![Type::Int]),
                    "Char" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                    "CodePoint" => Type::Generic("Lax".to_string(), vec![Type::Int]),
                    "Utf8Encode" => Type::Generic("Lax".to_string(), vec![Type::Bytes]),
                    "Utf8Decode" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                    "U16BE" | "U16LE" | "U32BE" | "U32LE" => {
                        Type::Generic("Lax".to_string(), vec![Type::Bytes])
                    }
                    "U16BEDecode" | "U16LEDecode" | "U32BEDecode" | "U32LEDecode" => {
                        Type::Generic("Lax".to_string(), vec![Type::Int])
                    }
                    "BytesCursor" => Type::BuchiPack(vec![
                        ("bytes".to_string(), Type::Bytes),
                        ("offset".to_string(), Type::Int),
                        ("length".to_string(), Type::Int),
                    ]),
                    "BytesCursorRemaining" => Type::Int,
                    "BytesCursorTake" => Type::Generic(
                        "Lax".to_string(),
                        vec![Type::BuchiPack(vec![
                            ("value".to_string(), Type::Bytes),
                            (
                                "cursor".to_string(),
                                Type::BuchiPack(vec![
                                    ("bytes".to_string(), Type::Bytes),
                                    ("offset".to_string(), Type::Int),
                                    ("length".to_string(), Type::Int),
                                ]),
                            ),
                        ])],
                    ),
                    "BytesCursorU8" => Type::Generic(
                        "Lax".to_string(),
                        vec![Type::BuchiPack(vec![
                            ("value".to_string(), Type::Int),
                            (
                                "cursor".to_string(),
                                Type::BuchiPack(vec![
                                    ("bytes".to_string(), Type::Bytes),
                                    ("offset".to_string(), Type::Int),
                                    ("length".to_string(), Type::Int),
                                ]),
                            ),
                        ])],
                    ),
                    "BitAnd" | "BitOr" | "BitXor" | "BitNot" => Type::Int,
                    "ShiftL" | "ShiftR" | "ShiftRU" => {
                        Type::Generic("Lax".to_string(), vec![Type::Int])
                    }
                    "ToRadix" => Type::Generic("Lax".to_string(), vec![Type::Str]),
                    "ByteSet" => Type::Generic("Lax".to_string(), vec![Type::Bytes]),
                    "BytesToList" => Type::List(Box::new(Type::Int)),
                    // HOF molds return the appropriate type
                    // If input is Stream[T], output is also Stream[U]
                    "Map" | "Filter" | "Sort" | "Unique" | "Flatten" | "Reverse" | "Take"
                    | "TakeWhile" | "Drop" | "DropWhile" | "Append" | "Prepend" | "Zip"
                    | "Enumerate" => {
                        // These return a list or stream (same or transformed)
                        if let Some(first_arg) = type_args.first() {
                            let arg_type = self.infer_expr_type(first_arg);
                            if matches!(arg_type, Type::Generic(ref n, _) if n == "Stream") {
                                // Stream input: return Stream (lazy transform)
                                arg_type
                            } else if matches!(arg_type, Type::List(_)) {
                                arg_type
                            } else {
                                Type::List(Box::new(Type::Unknown))
                            }
                        } else {
                            Type::List(Box::new(Type::Unknown))
                        }
                    }
                    // Stream[value]() → Stream[T]
                    "Stream" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Stream".to_string(), vec![inner])
                    }
                    // StreamFrom[list]() → Stream[T]
                    "StreamFrom" => {
                        if let Some(first_arg) = type_args.first() {
                            let arg_type = self.infer_expr_type(first_arg);
                            if let Type::List(inner) = arg_type {
                                Type::Generic("Stream".to_string(), vec![*inner])
                            } else {
                                Type::Generic("Stream".to_string(), vec![Type::Unknown])
                            }
                        } else {
                            Type::Generic("Stream".to_string(), vec![Type::Unknown])
                        }
                    }
                    "Fold" | "Foldr" | "Reduce" => {
                        // Returns the accumulator type (first arg)
                        if let Some(first_arg) = type_args.first() {
                            self.infer_expr_type(first_arg)
                        } else {
                            Type::Unknown
                        }
                    }
                    // String / Bytes operation molds
                    // B11-5d: If[cond, then, else]() returns the type of the then branch
                    // B11B-014: check branch type compatibility (same as | |> E1603)
                    "If" => {
                        if type_args.len() >= 3 {
                            let then_ty = self.infer_expr_type(&type_args[1]);
                            let else_ty = self.infer_expr_type(&type_args[2]);
                            if !(then_ty == Type::Unknown
                                || else_ty == Type::Unknown
                                || Self::contains_unknown(&then_ty)
                                || Self::contains_unknown(&else_ty)
                                || self.registry.is_subtype_of(&else_ty, &then_ty)
                                || then_ty.is_numeric() && else_ty.is_numeric())
                            {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1603] Condition branch type mismatch: then branch returns {}, but else branch returns {}. \
                                         Hint: Both branches of If[] should return the same type.",
                                        then_ty, else_ty
                                    ),
                                    span: mold_span.clone(),
                                });
                            }
                            then_ty
                        } else if type_args.len() >= 2 {
                            self.infer_expr_type(&type_args[1])
                        } else {
                            Type::Unknown
                        }
                    }
                    // B11-6e: TypeIs[value, :TypeName]() → Bool
                    "TypeIs" => Type::Bool,
                    // B11-6e: TypeExtends[:TypeA, :TypeB]() → Bool
                    // Note: E1613 (variant rejection) is checked by
                    // check_mold_errors_in_expr(), not here, to ensure it
                    // fires regardless of expression context.
                    "TypeExtends" => Type::Bool,
                    "Upper" | "Lower" | "Trim" | "Replace" | "Repeat" | "Pad" => Type::Str,
                    // C26B-018 (B)(C): byte-level primitive + single-alloc repeat/join
                    "ByteSlice" | "StringRepeatJoin" => Type::Str,
                    "ByteLength" => Type::Int,
                    "ByteAt" => Type::Generic("Lax".into(), vec![Type::Int]),
                    "CharAt" => Type::Generic("Lax".into(), vec![Type::Str]),
                    "Slice" => {
                        if let Some(first_arg) = type_args.first() {
                            let t = self.infer_expr_type(first_arg);
                            if t == Type::Bytes {
                                Type::Bytes
                            } else {
                                Type::Str
                            }
                        } else {
                            Type::Str
                        }
                    }
                    "Split" | "Chars" => Type::List(Box::new(Type::Str)),
                    // Number operation molds
                    "Abs" | "Clamp" => {
                        if let Some(first_arg) = type_args.first() {
                            let t = self.infer_expr_type(first_arg);
                            if t.is_numeric() { t } else { Type::Num }
                        } else {
                            Type::Num
                        }
                    }
                    "Floor" | "Ceil" | "Round" | "Truncate" => Type::Int,
                    "ToFixed" => Type::Str,
                    // List/Bytes operation molds
                    "Concat" => {
                        if let Some(first_arg) = type_args.first() {
                            let t = self.infer_expr_type(first_arg);
                            if t == Type::Bytes {
                                Type::Bytes
                            } else if matches!(t, Type::List(_)) || t == Type::Unknown {
                                t
                            } else {
                                Type::List(Box::new(Type::Unknown))
                            }
                        } else {
                            Type::List(Box::new(Type::Unknown))
                        }
                    }
                    "Join" => Type::Str,
                    "Sum" => Type::Num,
                    "Find" => Type::Generic("Lax".to_string(), vec![Type::Unknown]),
                    "FindIndex" | "Count" => Type::Int,
                    // Gorillax[value]() returns Gorillax[T]
                    "Gorillax" => {
                        let inner = type_args
                            .first()
                            .map(|a| self.infer_expr_type(a))
                            .unwrap_or(Type::Unknown);
                        Type::Generic("Gorillax".to_string(), vec![inner])
                    }
                    // Molten[]() returns Molten (no type arguments allowed)
                    "Molten" => {
                        if !type_args.is_empty() {
                            self.errors.push(TypeError {
                                message: "Molten takes no type arguments: Molten[]()".to_string(),
                                span: mold_span.clone(),
                            });
                        }
                        Type::Molten
                    }
                    // Cage[molten, F] where F: :Molten => :U → Gorillax[U]
                    // Cage requires Molten type as first argument
                    "Cage" => {
                        if let Some(first_arg) = type_args.first() {
                            let first_type = self.infer_expr_type(first_arg);
                            if first_type != Type::Molten && first_type != Type::Unknown {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "Cage requires Molten type as first argument, got {}",
                                        first_type
                                    ),
                                    span: mold_span.clone(),
                                });
                            }
                        }
                        // Extract the return type U from the second argument (function F):
                        // - If F is a lambda `_ x = expr`, infer body type directly → Gorillax[U]
                        // - If F is a function reference, infer its type → Function(params, ret) → extract ret
                        // - Otherwise, fall back to Unknown (safe)
                        let inner = if type_args.len() >= 2 {
                            let second_arg = &type_args[1];
                            match second_arg {
                                Expr::Lambda(_params, body, _span) => {
                                    // Lambda: infer the body expression type directly
                                    self.infer_expr_type(body)
                                }
                                _ => {
                                    // Function reference or other expression:
                                    // infer its type, then extract return type if it's a Function type
                                    let fn_type = self.infer_expr_type(second_arg);
                                    match fn_type {
                                        Type::Function(_, ret) => *ret,
                                        _ => Type::Unknown,
                                    }
                                }
                            }
                        } else {
                            Type::Unknown
                        };
                        Type::Generic("Gorillax".to_string(), vec![inner])
                    }
                    _ => {
                        // Look up in mold definitions
                        if self.registry.mold_defs.contains_key(name) {
                            Type::Named(name.clone())
                        } else if self.generic_func_defs.contains_key(name)
                            || self.func_types.contains_key(name)
                            || matches!(self.lookup_var(name), Some(Type::Function(_, _)))
                        {
                            // C20B-014 (ROOT-17) + C20B-016 (ROOT-19):
                            // user-defined function called via mold syntax
                            // `Fn[args]()`. Pre-C20B-016 this branch only
                            // rejected named fields and returned the raw
                            // function return type — arity, type-mismatch,
                            // partial-application and generic-inference
                            // validation were silently skipped, so
                            // `add[1, "x"]()` passed `taida check` while
                            // `add(1, "x")` correctly surfaced `[E1506]`.
                            //
                            // Post-fix: reject named fields first, then
                            // synthesize the equivalent `FuncCall` and
                            // delegate to the normal function-call path.
                            // This is the exact same AST shape the parser
                            // would have produced for `Fn(args)`, so every
                            // downstream rule (generic-func E1301 / E1506 /
                            // E1505, non-generic E1301 / E1506, function
                            // value E1301 / E1506 / E1505) fires uniformly.
                            if !fields.is_empty() {
                                self.errors.push(TypeError {
                                    message: format!(
                                        "[E1511] User-defined function '{}' called via mold syntax \
                                         cannot accept named fields '()'. \
                                         Pass arguments positionally: {}[arg1, arg2]() or {}(arg1, arg2).",
                                        name, name, name
                                    ),
                                    span: mold_span.clone(),
                                });
                            }
                            // Synthesize `name(type_args)` with the mold
                            // span and recurse. The callee span is the
                            // `mold_span` itself; positional args are the
                            // `type_args` list (which for `Fn[a, b]()` are
                            // the runtime values, cf. lower_molds.rs).
                            let synth_callee = Expr::Ident(name.clone(), mold_span.clone());
                            let synth_call = Expr::FuncCall(
                                Box::new(synth_callee),
                                type_args.clone(),
                                mold_span.clone(),
                            );
                            self.infer_expr_type(&synth_call)
                        } else {
                            Type::Unknown
                        }
                    }
                }
            }

            Expr::Unmold(inner, _) => {
                // Unmolding a Mold[T] returns T
                let inner_type = self.infer_expr_type(inner);
                match &inner_type {
                    Type::Generic(name, args) => {
                        match name.as_str() {
                            "Lax" | "Result" | "Async" => {
                                // Return the first type argument (the wrapped value type)
                                args.first().cloned().unwrap_or(Type::Unknown)
                            }
                            "Stream" => {
                                // Stream[T] unmolds to @[T] (List)
                                let inner = args.first().cloned().unwrap_or(Type::Unknown);
                                Type::List(Box::new(inner))
                            }
                            _ => Type::Unknown,
                        }
                    }
                    _ => Type::Unknown,
                }
            }

            Expr::Lambda(params, body, _span) => {
                let param_types: Vec<Type> = params
                    .iter()
                    .map(|p| {
                        p.type_annotation
                            .as_ref()
                            .map(|t| self.registry.resolve_type(t))
                            .unwrap_or(Type::Unknown)
                    })
                    .collect();
                // Push scope with lambda params so body references don't trigger E1502
                self.push_scope();
                for (i, p) in params.iter().enumerate() {
                    self.define_var(
                        &p.name,
                        param_types.get(i).cloned().unwrap_or(Type::Unknown),
                    );
                }
                // Try to infer return type from the body expression
                let ret_type = self.infer_expr_type(body);
                self.pop_scope();
                Type::Function(param_types, Box::new(ret_type))
            }

            Expr::EnumVariant(enum_name, variant_name, span) => {
                if !self.registry.is_enum_type(enum_name) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1608] Unknown enum type '{}'. Hint: Define `Enum => {} = ...` before using {}:{}().",
                            enum_name, enum_name, enum_name, variant_name
                        ),
                        span: span.clone(),
                    });
                    Type::Unknown
                } else if self
                    .registry
                    .get_enum_variant_ordinal(enum_name, variant_name)
                    .is_none()
                {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1608] Unknown enum variant '{}:{}()'. Hint: Use one of the variants declared on '{}'.",
                            enum_name, variant_name, enum_name
                        ),
                        span: span.clone(),
                    });
                    Type::Unknown
                } else {
                    Type::Named(enum_name.clone())
                }
            }

            Expr::TypeInst(name, _, _) => Type::Named(name.clone()),
            Expr::Throw(_, _) => Type::Unknown,
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
        // Infer type from the first arm
        let first_ty = if let Some(first_arm) = arms.first() {
            // Check condition type
            if let Some(cond) = &first_arm.condition {
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
                        span: first_arm.span.clone(),
                    });
                }
            }
            // Each arm gets its own scope for local bindings (e.g. ]=>)
            self.push_scope();
            for body_stmt in &first_arm.body {
                self.check_statement(body_stmt);
            }
            let ty = self.arm_result_type(first_arm);
            self.pop_scope();
            ty
        } else {
            Type::Unknown
        };

        // Check subsequent arms for type consistency
        for arm in arms.iter().skip(1) {
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
            if arm_ty != Type::Unknown
                && !(first_ty == Type::Unknown
                    || Self::contains_unknown(&first_ty)
                    || Self::contains_unknown(&arm_ty)
                    || self.registry.is_subtype_of(&arm_ty, &first_ty)
                    // Allow Int/Float mixing (both are Num)
                    || first_ty.is_numeric() && arm_ty.is_numeric())
            {
                self.errors.push(TypeError {
                    message: format!(
                        "[E1603] Condition branch type mismatch: first arm returns {}, but this arm returns {}. \
                         Hint: All arms of a condition branch should return the same type.",
                        first_ty, arm_ty
                    ),
                    span: span.clone(),
                });
            }
            self.pop_scope();
        }

        first_ty
    }

    /// C13-1: Infer the type of an arm's result. The result is:
    /// - `Statement::Expr(e)` → the inferred type of `e`
    /// - `Statement::Assignment(_)` / `UnmoldForward(_)` / `UnmoldBackward(_)`
    ///   → the registered type of the bound target (already recorded by
    ///   the preceding `check_statement` loop).
    /// - Anything else (definitions, imports, …) → `Type::Unknown`.
    ///
    /// Must be called *after* `check_statement` has processed the arm
    /// body so that tail-binding targets are present in scope.
    fn arm_result_type(&mut self, arm: &CondArm) -> Type {
        let Some(last_stmt) = arm.body.last() else {
            return Type::Unknown;
        };
        match last_stmt {
            Statement::Expr(e) => self.infer_expr_type(e),
            Statement::Assignment(a) => self.lookup_var(&a.target).unwrap_or(Type::Unknown),
            Statement::UnmoldForward(u) => self.lookup_var(&u.target).unwrap_or(Type::Unknown),
            Statement::UnmoldBackward(u) => self.lookup_var(&u.target).unwrap_or(Type::Unknown),
            _ => Type::Unknown,
        }
    }
}

#[path = "checker_methods.rs"]
mod checker_methods;

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "checker_tests.rs"]
mod tests;

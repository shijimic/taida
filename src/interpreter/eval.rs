/// Tree-walking interpreter for Taida Lang.
///
/// Key behaviors:
/// - All variables are immutable
/// - No null/undefined — out-of-bounds access returns default values
/// - Error ceiling (|==) catches thrown errors
/// - Gorilla (><) terminates the program immediately
/// - Standard methods via auto-mold
/// - Module system with >>> / <<< operators
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};

use super::env::Environment;
use super::value::{FuncValue, Value};
use crate::parser::*;

/// Maximum function call depth before RuntimeError (RCB-301 / SEC-002).
/// Prevents stack overflow from deeply recursive (non-TCO) calls.
/// Set conservatively to account for multiple Rust stack frames per call
/// (eval_statements + eval_expr + call_function = ~5 frames per recursion,
/// plus debug builds have larger frames).
const MAX_CALL_DEPTH: usize = 256;

/// Runtime error (distinct from thrown Taida errors).
#[derive(Debug, Clone)]
pub struct RuntimeError {
    pub message: String,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Runtime error: {}", self.message)
    }
}

/// Control flow signal for the interpreter.
#[derive(Debug)]
pub(crate) enum Signal {
    /// Normal value
    Value(Value),
    /// A Taida error was thrown — propagate to nearest error ceiling
    Throw(Value),
    /// Gorilla — terminate immediately
    Gorilla,
    /// Tail call — restart a function with new arguments (TCO).
    /// For self-recursion, the target is the current function.
    /// For mutual recursion, `mutual_tail_call_target` on the Interpreter is set.
    TailCall(Vec<Value>),
}

/// Cached module: exported symbols from a loaded .td file.
#[derive(Debug, Clone)]
pub(crate) struct LoadedModule {
    /// Exported symbols (name -> value)
    pub(crate) exports: HashMap<String, Value>,
    /// QF-17: TypeDef field definitions exported from this module
    pub(crate) type_defs: HashMap<String, Vec<FieldDef>>,
    /// Mold field definitions exported from this module.
    pub(crate) mold_defs: HashMap<String, Vec<FieldDef>>,
    /// Enum definitions exported from this module
    pub(crate) enum_defs: HashMap<String, Vec<String>>,
    /// QF-17: TypeDef methods exported from this module
    pub(crate) type_methods: HashMap<String, HashMap<String, FuncDef>>,
}

/// Idle resource entry tracked by pool runtime state.
#[derive(Debug, Clone)]
pub(crate) struct PoolEntry {
    pub(crate) token: i64,
    pub(crate) resource: Value,
}

/// Minimal in-memory state for taida-lang/pool.
#[derive(Debug, Clone)]
pub(crate) struct PoolState {
    pub(crate) open: bool,
    pub(crate) max_size: i64,
    pub(crate) max_idle: i64,
    pub(crate) acquire_timeout_ms: i64,
    pub(crate) idle: Vec<PoolEntry>,
    pub(crate) in_use_tokens: HashSet<i64>,
    pub(crate) next_token: i64,
}

/// The Taida interpreter.
pub struct Interpreter {
    pub env: Environment,
    /// Output buffer (for testing — captures print output).
    ///
    /// Used in **buffered mode** (`stream_stdout == false`) — the default for
    /// `Interpreter::new()`. REPL (`main.rs:5091-5108`), in-process Rust test
    /// harness (`tests_extended.rs::eval_with_output`), and JS codegen embedding
    /// all rely on this Vec being populated by `stdout` / `debug` builtins.
    ///
    /// In **stream mode** (`stream_stdout == true`, via `Interpreter::new_streaming()`)
    /// this Vec is left empty — `stdout` / `debug` write directly to the real
    /// `io::stdout()` / `io::stderr()` with immediate flush. (C22-2 / C22B-002)
    pub output: Vec<String>,
    /// Stream mode flag (C22-2 / C22B-002).
    ///
    /// `false` (default): buffered mode — `stdout` / `debug` push to `self.output`.
    /// `true`: stream mode — `stdout` writes to real stdout with immediate flush,
    /// `debug` writes to real stderr. CLI `taida <file>` uses stream mode;
    /// REPL / test / JS codegen embedding uses buffered mode.
    ///
    /// The stream/buffered split is **internal to the Rust API** and has no
    /// Taida surface effect (`stdout()` still returns `Int` bytes and adds the
    /// implicit `\n`). The 3-backend generated code (JS / Native / WASM) is
    /// unaffected — it emits its own I/O directly.
    pub stream_stdout: bool,
    /// Number of stdout-visible emissions (from `stdout` **or** `debug`
    /// builtins) seen so far (C22-2).
    ///
    /// In stream mode, `output` Vec is always empty, so `main.rs` cannot use
    /// `output.is_empty()` to decide whether to print the final value. This
    /// counter replaces that check: after `eval_program`, if
    /// `stdout_emissions == 0` and the final value is not `Unit`, print it
    /// (matches existing buffered behavior where `output.is_empty()` conveys
    /// the same signal). Named "emissions" rather than "stdout_count" because
    /// `debug` also increments it — both builtins surface to stdout in stream
    /// mode (see `prelude.rs` for the debug→stdout design rationale).
    pub stdout_emissions: usize,
    /// Current file path (for resolving relative imports)
    pub(crate) current_file: Option<PathBuf>,
    /// Cache of loaded modules (canonical path -> exports)
    pub(crate) loaded_modules: HashMap<PathBuf, LoadedModule>,
    /// Set of modules currently being loaded (for circular import detection)
    pub(crate) loading_modules: HashSet<PathBuf>,
    /// Currently executing function name (for tail call optimization)
    active_function: Option<String>,
    /// When a mutual tail call is detected, this holds the target function name.
    /// Used by call_function's trampoline loop to switch to a different function.
    mutual_tail_call_target: Option<String>,
    /// Methods defined in TypeDef/InheritanceDef/MoldDef: type_name -> method_name -> FuncDef
    pub(crate) type_methods: HashMap<String, HashMap<String, FuncDef>>,
    /// TypeDef field definitions: type_name -> Vec<FieldDef> (for JSON schema matching)
    pub(crate) type_defs: HashMap<String, Vec<FieldDef>>,
    /// Enum definitions: enum_name -> variants in ordinal order
    pub(crate) enum_defs: HashMap<String, Vec<String>>,
    /// MoldDef field definitions: mold_name -> Vec<FieldDef> (for filling/unmold lookup)
    pub(crate) mold_defs: HashMap<String, Vec<FieldDef>>,
    /// Symbols declared via `<<<` during module execution.
    /// Empty means no `<<<` was encountered (all symbols exported for backward compat).
    pub(crate) module_exported_symbols: Vec<String>,
    /// Tokio runtime for true async operations.
    /// Used by `]=>` to block_on pending async values and by All/Race/Timeout molds.
    pub(crate) tokio_runtime: Arc<tokio::runtime::Runtime>,
    /// Socket handle table for tcpConnect/socketSend/socketRecv.
    /// Key is a stable interpreter-local handle id returned to Taida code.
    pub(crate) socket_handles:
        Arc<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<tokio::net::TcpStream>>>>>,
    /// Monotonic handle allocator for socket_handles.
    pub(crate) next_socket_id: Arc<AtomicI64>,
    /// Listener handle table for tcpListen.
    /// Key is a stable interpreter-local handle id returned to Taida code.
    pub(crate) listener_handles:
        Arc<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<tokio::net::TcpListener>>>>>,
    /// Monotonic handle allocator for listener_handles.
    pub(crate) next_listener_id: Arc<AtomicI64>,
    /// UDP socket handle table for udpBind/udpSendTo/udpRecvFrom.
    /// Key is a stable interpreter-local handle id returned to Taida code.
    pub(crate) udp_socket_handles:
        Arc<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<tokio::net::UdpSocket>>>>>,
    /// Pool state table for taida-lang/pool.
    pub(crate) pool_states: Arc<Mutex<HashMap<i64, PoolState>>>,
    /// Monotonic handle allocator for pool_states.
    pub(crate) next_pool_id: Arc<AtomicI64>,
    /// Pending throw from a HOF callback (Map/Filter/Fold etc.).
    /// When a callback inside call_function_with_values throws, the thrown value
    /// is stored here so that eval_statements' error ceiling can recover it.
    pub(crate) pending_throw: Option<Value>,
    /// RCB-101: Inheritance parent map (child_name -> parent_name).
    /// Used by error ceiling to check if a thrown error type IS-A the handler type.
    pub(crate) type_parents: HashMap<String, String>,
    /// RCB-301: Current function call depth for stack overflow prevention.
    /// Incremented on each non-TCO function call, decremented on return.
    call_depth: usize,
    /// NET3-2: Active streaming writer for 2-arg httpServe handler.
    /// Set before calling the handler, cleared after the handler returns.
    /// This allows startResponse/writeChunk/endResponse/sseEvent to access
    /// the connection's StreamingWriter state and TcpStream during handler execution.
    /// Safety: The interpreter is single-threaded (!Send). The raw pointers point to
    /// stack-local variables in dispatch_request that outlive the handler call.
    pub(crate) active_streaming_writer: Option<super::net_eval::ActiveStreamingWriter>,
    /// E30B-007 / Lock-G: Context for the addon facade currently being loaded.
    ///
    /// When `load_addon_facade` is evaluating a `<pkg>/taida/<stem>.td` file,
    /// this field holds `Some((package_id, function_arities))` so that the
    /// `RustAddon["fn"](arity <= N)` expression form can:
    ///
    /// 1. Resolve to the canonical addon sentinel
    ///    `Value::str("__taida_addon_call::<package_id>::<fn>")` (matching
    ///    today's `define_force` pre-injection contract).
    /// 2. Validate the declared `arity` field against the manifest
    ///    `[functions]` table and reject drift with `[E1412]`.
    ///
    /// Outside facade execution this is `None` and a `RustAddon[...]`
    /// expression falls through to the generic `MoldInst` path (the
    /// checker will surface a `[E1412]` "context" diagnostic in a later
    /// sub-step; the interpreter contract here is purely runtime).
    pub(crate) loading_addon_facade_ctx: Option<(String, std::collections::BTreeMap<String, u32>)>,
}

impl Interpreter {
    /// E30 Phase 6 / E30B-004: synthetic defaultFn sentinel name.
    ///
    /// `default_for_type_expr` returns a `Value::Function` whose `name`
    /// equals this constant when materialising the default for a
    /// `TypeExpr::Function(_, _)` annotation (declare-only function field's
    /// auto-generated `defaultFn`). `call_function` intercepts the name
    /// before entering the trampoline loop and returns the **return
    /// type's default value** without executing any AST body.
    pub(crate) const DEFAULT_FN_SENTINEL_NAME: &'static str = "__taida_default_fn";

    pub fn new() -> Self {
        // Create a dedicated current-thread tokio runtime for the interpreter.
        // This runtime is used by `]=>` to block_on pending async values
        // and by All/Race/Timeout molds for parallel resolution.
        let tokio_runtime = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect(
                    "Failed to create Tokio runtime for interpreter: system resource exhaustion",
                ),
        );

        Self {
            env: Environment::new(),
            output: Vec::new(),
            stream_stdout: false,
            stdout_emissions: 0,
            current_file: None,
            loaded_modules: HashMap::new(),
            loading_modules: HashSet::new(),
            active_function: None,
            mutual_tail_call_target: None,
            type_methods: HashMap::new(),
            type_defs: HashMap::new(),
            enum_defs: HashMap::new(),
            mold_defs: HashMap::new(),
            module_exported_symbols: Vec::new(),
            tokio_runtime,
            socket_handles: Arc::new(Mutex::new(HashMap::new())),
            next_socket_id: Arc::new(AtomicI64::new(1)),
            listener_handles: Arc::new(Mutex::new(HashMap::new())),
            next_listener_id: Arc::new(AtomicI64::new(1)),
            udp_socket_handles: Arc::new(Mutex::new(HashMap::new())),
            pool_states: Arc::new(Mutex::new(HashMap::new())),
            next_pool_id: Arc::new(AtomicI64::new(1)),
            pending_throw: None,
            type_parents: HashMap::new(),
            call_depth: 0,
            active_streaming_writer: None,
            loading_addon_facade_ctx: None,
        }
    }

    /// Construct a new Interpreter in **stream mode** (C22-2 / C22B-002).
    ///
    /// In stream mode, `stdout(...)` writes directly to the real `io::stdout()`
    /// with immediate flush (line-by-line), and `debug(...)` writes directly to
    /// `io::stderr()`. The `output` Vec is **not** populated.
    ///
    /// This is the mode used by the CLI `taida <file>`
    /// execution path (`main.rs::run_file_cmd`), restoring POSIX-standard
    /// immediate-flush behavior so that TUI / progress / spinner / printf-debug
    /// all work as expected.
    ///
    /// REPL, in-process Rust test harness, and JS codegen embedding continue
    /// to use `Interpreter::new()` (buffered mode) where the `output` Vec is
    /// required for their own display / capture logic.
    ///
    /// The public contract of `stdout` / `debug` is unchanged — still returns
    /// `Int` bytes, still appends the implicit `\n` on each invocation. Only
    /// the flush timing differs between the two modes.
    pub fn new_streaming() -> Self {
        let mut interp = Self::new();
        interp.stream_stdout = true;
        interp
    }

    /// RCB-101: Check if `thrown_type` IS-A `handler_type` by walking the inheritance chain.
    /// Returns true if they are the same type or if `thrown_type` inherits from `handler_type`.
    fn is_error_subtype(&self, thrown_type: &str, handler_type: &str) -> bool {
        if thrown_type == handler_type {
            return true;
        }
        // Walk the inheritance chain: thrown_type -> parent -> grandparent -> ...
        let mut current = thrown_type;
        // Limit chain depth to avoid infinite loops from corrupted data
        for _ in 0..64 {
            if let Some(parent) = self.type_parents.get(current) {
                if parent == handler_type {
                    return true;
                }
                current = parent;
            } else {
                break;
            }
        }
        false
    }

    /// B11-6b: Check if `child_type` extends `parent_type` by walking the inheritance chain.
    /// Used by TypeExtends mold.
    pub(crate) fn check_type_extends(&self, child_type: &str, parent_type: &str) -> bool {
        if child_type == parent_type {
            return true;
        }
        // Walk the inheritance chain: child -> parent -> grandparent -> ...
        let mut current = child_type;
        for _ in 0..64 {
            if let Some(parent) = self.type_parents.get(current) {
                if parent == parent_type {
                    return true;
                }
                current = parent;
            } else {
                break;
            }
        }
        false
    }

    /// Set the current file path for module resolution.
    pub fn set_current_file(&mut self, path: &Path) {
        self.current_file = Some(path.to_path_buf());
    }

    /// Evaluate an entire program.
    pub fn eval_program(&mut self, program: &Program) -> Result<Value, RuntimeError> {
        match self.eval_statements(&program.statements)? {
            Signal::Value(v) => Ok(v),
            Signal::TailCall(_) => {
                // TailCall should not escape to program level
                Err(RuntimeError {
                    message: "Internal error: unhandled tail call".to_string(),
                })
            }
            Signal::Throw(err) => {
                // Gorilla ceiling: unhandled error terminates program
                Err(RuntimeError {
                    message: format!("Unhandled error: {}", err),
                })
            }
            Signal::Gorilla => Ok(Value::Gorilla),
        }
    }

    /// Evaluate a sequence of statements with error ceiling support.
    ///
    /// When an ErrorCeiling statement is encountered, the remaining statements
    /// become "protected" code. Any Throw signal from the protected code is
    /// caught and the error ceiling's handler body is executed.
    pub(crate) fn eval_statements(&mut self, stmts: &[Statement]) -> Result<Signal, RuntimeError> {
        let mut last_value = Value::Unit;
        let mut i = 0;

        while i < stmts.len() {
            if let Statement::ErrorCeiling(ec) = &stmts[i] {
                // The remaining statements after the error ceiling are "protected"
                let protected_stmts = &stmts[i + 1..];

                // Evaluate the protected statements WITHOUT tail-call optimization.
                // TCO on the last protected statement would return Signal::TailCall,
                // bypassing the error ceiling catch below.  The caller (call_function's
                // trampoline) would then execute the function outside this ceiling's scope,
                // so any throw would propagate unhandled.  Using eval_statements_no_tco
                // ensures all function calls actually execute within the ceiling.
                //
                // If a RuntimeError occurs that wraps a Taida throw (from a HOF callback),
                // recover the thrown value from pending_throw and handle it as Signal::Throw.
                let protected_result = match self.eval_statements_no_tco(protected_stmts) {
                    Ok(signal) => signal,
                    Err(runtime_err) => {
                        if let Some(thrown_val) = self.pending_throw.take() {
                            // Recover the Taida throw that was wrapped in a RuntimeError
                            Signal::Throw(thrown_val)
                        } else {
                            return Err(runtime_err);
                        }
                    }
                };
                match protected_result {
                    Signal::Value(v) => return Ok(Signal::Value(v)),
                    Signal::TailCall(args) => return Ok(Signal::TailCall(args)),
                    Signal::Throw(err) => {
                        // RCB-101: Check if the thrown error type matches the handler's error_type.
                        // Extract the expected type name from the ErrorCeiling's type annotation.
                        let handler_type = match &ec.error_type {
                            TypeExpr::Named(name) => name.as_str(),
                            _ => "Error", // fallback: catch-all for complex type exprs
                        };

                        // Extract the actual error type from the thrown value.
                        let thrown_type = match &err {
                            Value::Error(e) => Some(e.error_type.as_str()),
                            Value::BuchiPack(fields) => fields
                                .iter()
                                .find(|(n, _)| n == "__type")
                                .and_then(|(_, v)| {
                                    if let Value::Str(s) = v {
                                        Some(s.as_str())
                                    } else {
                                        None
                                    }
                                }),
                            _ => None,
                        };

                        // Check if the thrown type IS-A the handler type.
                        // "Error" catches everything (base type).
                        // Otherwise, walk the inheritance chain.
                        let type_matches = if handler_type == "Error" {
                            true
                        } else if let Some(thrown) = thrown_type {
                            self.is_error_subtype(thrown, handler_type)
                        } else {
                            // Unknown thrown value type — do NOT catch-all.
                            // Only the "Error" base handler (checked above) should catch everything.
                            // This matches Native/WASM behavior (return 0 for unknown type).
                            false
                        };

                        if type_matches {
                            // Error type matches — run the handler.
                            //
                            // C20B-017 / ROOT-20: handler body may itself raise a
                            // `RuntimeError` (undefined variable, method lookup failure,
                            // etc.). We MUST pop the handler scope on both Ok and Err
                            // paths, otherwise the leaked scope causes outer
                            // `call_function*` cleanup to peel off the wrong layer and
                            // let the enclosing closure scope leak across REPL inputs.
                            //
                            // Pattern B: split the `?` from the call so `pop_scope`
                            // runs before we propagate any error.
                            self.env.push_scope();
                            self.env.define_force(&ec.error_param, err);

                            let handler_result = self.eval_statements(&ec.handler_body);
                            self.env.pop_scope();
                            let handler_signal = handler_result?;

                            match handler_signal {
                                Signal::Value(v) => return Ok(Signal::Value(v)),
                                Signal::TailCall(args) => return Ok(Signal::TailCall(args)),
                                Signal::Throw(err) => return Ok(Signal::Throw(err)),
                                Signal::Gorilla => return Ok(Signal::Gorilla),
                            }
                        } else {
                            // Type does not match — re-throw (propagate to outer ceiling)
                            return Ok(Signal::Throw(err));
                        }
                    }
                    Signal::Gorilla => return Ok(Signal::Gorilla),
                }
            }

            // The last statement in a sequence is in tail position
            let is_last = i == stmts.len() - 1;
            if is_last {
                match self.eval_statement_tail(&stmts[i])? {
                    Signal::Value(v) => last_value = v,
                    Signal::TailCall(args) => return Ok(Signal::TailCall(args)),
                    Signal::Throw(err) => return Ok(Signal::Throw(err)),
                    Signal::Gorilla => return Ok(Signal::Gorilla),
                }
            } else {
                match self.eval_statement(&stmts[i])? {
                    Signal::Value(v) => last_value = v,
                    Signal::TailCall(args) => return Ok(Signal::TailCall(args)),
                    Signal::Throw(err) => return Ok(Signal::Throw(err)),
                    Signal::Gorilla => return Ok(Signal::Gorilla),
                }
            }
            i += 1;
        }

        Ok(Signal::Value(last_value))
    }

    /// Evaluate a statement, returning a control flow signal.
    fn eval_statement(&mut self, stmt: &Statement) -> Result<Signal, RuntimeError> {
        match stmt {
            Statement::Expr(expr) => self.eval_expr(expr),

            Statement::EnumDef(ed) => {
                self.enum_defs.insert(
                    ed.name.clone(),
                    ed.variants
                        .iter()
                        .map(|variant| variant.name.clone())
                        .collect(),
                );
                let _ = self.env.define(
                    &ed.name,
                    Value::pack(vec![
                        ("__type".to_string(), Value::str("EnumDef".to_string())),
                        ("__name".to_string(), Value::str(ed.name.clone())),
                    ]),
                );
                Ok(Signal::Value(Value::Unit))
            }

            // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch (旧 TypeDef/MoldDef/InheritanceDef を統合)
            Statement::ClassLikeDef(cl) => match &cl.kind {
                crate::parser::ClassLikeKind::BuchiPack => {
                    let td = cl;
                    // Register methods defined in the type
                    let mut methods = HashMap::new();
                    for field in &td.fields {
                        if field.is_method
                            && let Some(ref func_def) = field.method_def
                        {
                            methods.insert(field.name.clone(), func_def.clone());
                        }
                    }
                    if !methods.is_empty() {
                        self.type_methods.insert(td.name.clone(), methods);
                    }
                    self.type_defs.insert(td.name.clone(), td.fields.clone());
                    let _ = self.env.define(
                        &td.name,
                        Value::pack(vec![
                            ("__type".to_string(), Value::str("TypeDef".to_string())),
                            ("__name".to_string(), Value::str(td.name.clone())),
                        ]),
                    );
                    Ok(Signal::Value(Value::Unit))
                }
                crate::parser::ClassLikeKind::Mold { .. } => {
                    let md = cl;
                    // Register methods defined in the mold type
                    let mut methods = HashMap::new();
                    for field in &md.fields {
                        if field.is_method
                            && let Some(ref func_def) = field.method_def
                        {
                            methods.insert(field.name.clone(), func_def.clone());
                        }
                    }
                    if !methods.is_empty() {
                        self.type_methods.insert(md.name.clone(), methods);
                    }
                    self.type_defs.insert(md.name.clone(), md.fields.clone());
                    self.mold_defs.insert(md.name.clone(), md.fields.clone());
                    let _ = self.env.define(
                        &md.name,
                        Value::pack(vec![
                            ("__type".to_string(), Value::str("TypeDef".to_string())),
                            ("__name".to_string(), Value::str(md.name.clone())),
                        ]),
                    );
                    Ok(Signal::Value(Value::Unit))
                }
                crate::parser::ClassLikeKind::Inheritance { parent, .. } => {
                    let inh = cl;
                    let inh_parent = parent;
                    let inh_child = &inh.name;
                    let mut methods = self
                        .type_methods
                        .get(inh_parent)
                        .cloned()
                        .unwrap_or_default();
                    for field in &inh.fields {
                        if field.is_method
                            && let Some(ref func_def) = field.method_def
                        {
                            methods.insert(field.name.clone(), func_def.clone());
                        }
                    }
                    if !methods.is_empty() {
                        self.type_methods.insert(inh_child.clone(), methods);
                    }
                    let mut merged_fields = self
                        .type_defs
                        .get(inh_parent)
                        .cloned()
                        .or_else(|| self.mold_defs.get(inh_parent).cloned())
                        .unwrap_or_default();
                    for child_field in &inh.fields {
                        if let Some(existing) = merged_fields
                            .iter_mut()
                            .find(|field| field.name == child_field.name)
                        {
                            *existing = child_field.clone();
                        } else {
                            merged_fields.push(child_field.clone());
                        }
                    }
                    self.type_defs.insert(inh_child.clone(), merged_fields);
                    if self.mold_defs.contains_key(inh_parent) {
                        self.mold_defs
                            .insert(inh_child.clone(), self.type_defs[inh_child].clone());
                    }
                    self.type_parents
                        .insert(inh_child.clone(), inh_parent.clone());
                    let _ = self.env.define(
                        inh_child,
                        Value::pack(vec![
                            ("__type".to_string(), Value::str("TypeDef".to_string())),
                            ("__name".to_string(), Value::str(inh_child.clone())),
                        ]),
                    );
                    Ok(Signal::Value(Value::Unit))
                }
            },

            Statement::FuncDef(fd) => {
                let closure = Arc::new(self.env.snapshot());
                let func = Value::Function(FuncValue {
                    name: fd.name.clone(),
                    params: fd.params.clone(),
                    body: fd.body.clone(),
                    closure,
                    return_type: fd.return_type.clone(),
                    module_type_defs: None,
                    module_enum_defs: None,
                });
                // Use define() to prevent overwriting existing variables/functions.
                // define_force is reserved for internal use only (pipeline, closures, params, prelude).
                if let Err(e) = self.env.define(&fd.name, func) {
                    return Err(RuntimeError { message: e });
                }
                Ok(Signal::Value(Value::Unit))
            }

            Statement::Assignment(assign) => {
                let value = match self.eval_expr(&assign.value)? {
                    Signal::Value(v) => v,
                    Signal::TailCall(args) => return Ok(Signal::TailCall(args)),
                    Signal::Throw(err) => return Ok(Signal::Throw(err)),
                    Signal::Gorilla => return Ok(Signal::Gorilla),
                };
                // Use define() to enforce immutability: re-assignment in same scope is an error.
                // Internal variables (pipeline, closures, params) use define_force().
                if let Err(e) = self.env.define(&assign.target, value.clone()) {
                    return Err(RuntimeError { message: e });
                }
                Ok(Signal::Value(value))
            }

            Statement::ErrorCeiling(ec) => self.eval_error_ceiling(ec),

            Statement::Import(import) => self.eval_import(import),

            Statement::Export(export) => self.eval_export(export),

            Statement::UnmoldForward(uf) => {
                let source_val = match self.eval_expr(&uf.source)? {
                    Signal::Value(v) => v,
                    Signal::TailCall(args) => return Ok(Signal::TailCall(args)),
                    Signal::Throw(err) => return Ok(Signal::Throw(err)),
                    Signal::Gorilla => return Ok(Signal::Gorilla),
                };
                // Unmold: extract the inner value from a Mold wrapper
                // For Async values, ]=> acts as blocking await
                // Rejected Async throws an error (caught by error ceiling)
                //
                // C13-1: Return the unwrapped value as the statement's
                // signal so tail unmold `expr ]=> name` in an expression
                // block yields the bound value.
                match self.unmold_value(source_val)? {
                    Signal::Value(unwrapped) => {
                        // Use define() to enforce immutability for unmold targets
                        if let Err(e) = self.env.define(&uf.target, unwrapped.clone()) {
                            return Err(RuntimeError { message: e });
                        }
                        Ok(Signal::Value(unwrapped))
                    }
                    Signal::Throw(err) => Ok(Signal::Throw(err)),
                    Signal::Gorilla => Ok(Signal::Gorilla),
                    Signal::TailCall(args) => Ok(Signal::TailCall(args)),
                }
            }

            Statement::UnmoldBackward(ub) => {
                let source_val = match self.eval_expr(&ub.source)? {
                    Signal::Value(v) => v,
                    Signal::TailCall(args) => return Ok(Signal::TailCall(args)),
                    Signal::Throw(err) => return Ok(Signal::Throw(err)),
                    Signal::Gorilla => return Ok(Signal::Gorilla),
                };
                // For Async values, <=[ acts as blocking await
                // Rejected Async throws an error (caught by error ceiling)
                //
                // C13-1: Return the unwrapped value as the statement's
                // signal so tail unmold `name <=[ expr` in an expression
                // block yields the bound value.
                match self.unmold_value(source_val)? {
                    Signal::Value(unwrapped) => {
                        // Use define() to enforce immutability for unmold targets
                        if let Err(e) = self.env.define(&ub.target, unwrapped.clone()) {
                            return Err(RuntimeError { message: e });
                        }
                        Ok(Signal::Value(unwrapped))
                    }
                    Signal::Throw(err) => Ok(Signal::Throw(err)),
                    Signal::Gorilla => Ok(Signal::Gorilla),
                    Signal::TailCall(args) => Ok(Signal::TailCall(args)),
                }
            }
        }
    }

    /// C13-1 / C13B-007: Return true if `expr` contains an `Expr::Ident(name, _)`
    /// whose name appears in `bound_names`. Used to decide whether a
    /// pipeline step explicitly consumes a pipeline-scope binding, in
    /// which case auto-injection of the pipeline `current` as an extra
    /// argument is suppressed.
    pub(crate) fn expr_references_any(expr: &Expr, bound_names: &[String]) -> bool {
        fn walk(e: &Expr, names: &[String]) -> bool {
            match e {
                Expr::Ident(n, _) => names.iter().any(|bn| bn == n),
                Expr::BinaryOp(l, _, r, _) => walk(l, names) || walk(r, names),
                Expr::UnaryOp(_, inner, _) => walk(inner, names),
                Expr::FuncCall(callee, args, _) => {
                    walk(callee, names) || args.iter().any(|a| walk(a, names))
                }
                Expr::MethodCall(obj, _, args, _) => {
                    walk(obj, names) || args.iter().any(|a| walk(a, names))
                }
                Expr::FieldAccess(obj, _, _) => walk(obj, names),
                Expr::BuchiPack(fields, _) => fields.iter().any(|f| walk(&f.value, names)),
                Expr::ListLit(items, _) => items.iter().any(|x| walk(x, names)),
                Expr::Pipeline(steps, _) => steps.iter().any(|s| walk(s, names)),
                Expr::MoldInst(_, type_args, fields, _) => {
                    type_args.iter().any(|a| walk(a, names))
                        || fields.iter().any(|f| walk(&f.value, names))
                }
                Expr::Unmold(inner, _) => walk(inner, names),
                Expr::Lambda(_, body, _) => walk(body, names),
                Expr::TypeInst(_, fields, _) => fields.iter().any(|f| walk(&f.value, names)),
                Expr::Throw(inner, _) => walk(inner, names),
                Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
                    arm.condition.as_ref().is_some_and(|c| walk(c, names))
                        || arm.body.iter().any(|s| {
                            if let Statement::Expr(e) = s {
                                walk(e, names)
                            } else {
                                false
                            }
                        })
                }),
                _ => false,
            }
        }
        walk(expr, bound_names)
    }

    /// C13-1 / C13B-007: Decide whether an intermediate pipeline step
    /// `=> name` should call `name` with the current value (classic
    /// function-step semantics) or bind the current value to `name`
    /// and forward it unchanged (bind-and-forward semantics).
    ///
    /// A name is considered callable-in-pipeline if:
    ///   - it already resolves to a `Value::Function` in the current
    ///     environment (user-defined function, prelude function stored
    ///     as a Function), or
    ///   - it resolves to a builtin sentinel `Value::Str` (used for
    ///     addon dispatch / net builtins / crypto), or
    ///   - it is not defined in the environment at all — in which case
    ///     we assume it's a binding target (never an undefined function
    ///     reference, which would have been caught by the checker).
    ///
    /// For any resolved *value* (Int, Str, List, BuchiPack, ...) that
    /// is not a function-like value, the intermediate step is
    /// bind-and-forward: we take the name as a fresh binding target.
    pub(crate) fn is_pipeline_bindable_callable(&self, name: &str) -> bool {
        match self.env.get(name) {
            Some(Value::Function(_)) => true,
            Some(Value::Str(s)) if s.starts_with("__") => {
                // Builtin dispatch sentinels (e.g. "__net_builtin_httpServe",
                // "__crypto_builtin_sha256", "__taida_addon_call::..."). These
                // are callable through try_builtin_func when named as a pipeline
                // step, so preserve classic semantics.
                true
            }
            _ => false,
        }
    }

    /// Evaluate statements without tail-call optimization.
    /// Used for error-ceiling protected code so that function calls in tail
    /// position actually execute within the ceiling's scope instead of being
    /// deferred via Signal::TailCall (which would bypass the catch handler).
    pub(crate) fn eval_statements_no_tco(
        &mut self,
        stmts: &[Statement],
    ) -> Result<Signal, RuntimeError> {
        let mut last_value = Value::Unit;
        for (i, stmt) in stmts.iter().enumerate() {
            // Check for nested error ceilings (delegate to full eval_statements)
            if let Statement::ErrorCeiling(_) = stmt {
                return self.eval_statements(&stmts[i..]);
            }
            match self.eval_statement(stmt)? {
                Signal::Value(v) => last_value = v,
                Signal::TailCall(args) => return Ok(Signal::TailCall(args)),
                Signal::Throw(err) => return Ok(Signal::Throw(err)),
                Signal::Gorilla => return Ok(Signal::Gorilla),
            }
        }
        Ok(Signal::Value(last_value))
    }

    /// Evaluate a statement in tail position.
    /// The only difference from eval_statement is that expression statements
    /// are evaluated with tail-call awareness.
    fn eval_statement_tail(&mut self, stmt: &Statement) -> Result<Signal, RuntimeError> {
        match stmt {
            Statement::Expr(expr) => self.eval_expr_tail(expr),
            // All other statement types delegate to the normal evaluation
            _ => self.eval_statement(stmt),
        }
    }

    /// Evaluate a condition arm body (Vec<Statement>) in tail position.
    /// Non-last statements are evaluated normally; the last is evaluated with tail-call awareness.
    fn eval_cond_arm_body_tail(&mut self, body: &[Statement]) -> Result<Signal, RuntimeError> {
        if body.is_empty() {
            return Ok(Signal::Value(Value::Unit));
        }
        // Evaluate all statements except the last one normally
        for stmt in &body[..body.len() - 1] {
            match self.eval_statement(stmt)? {
                Signal::Value(_) => {} // continue
                other => return Ok(other),
            }
        }
        // Evaluate the last statement in tail position
        self.eval_statement_tail(&body[body.len() - 1])
    }

    /// Evaluate an expression in tail position.
    /// If the expression is a self-recursive or mutual-recursive call,
    /// return TailCall instead of recursing to enable trampoline optimization.
    fn eval_expr_tail(&mut self, expr: &Expr) -> Result<Signal, RuntimeError> {
        match expr {
            // Function call in tail position — check for TCO opportunity
            Expr::FuncCall(callee, args, _) => {
                // C26B-017: A partial application `f(a, )` (with Hole args) is
                // NOT a real function call — it returns a closure. Falling
                // through to the TCO path would evaluate each Hole as
                // `Value::Unit` and issue a spurious tail call to `f` with
                // Unit-filled args, breaking closure capture (symptom:
                // `Cannot add <n> and @()` when the partial is the last
                // expression of a function body). Route partial applications
                // to the normal evaluator so `eval_partial_application` can
                // build the proper closure value.
                let is_partial_application = args.iter().any(|a| matches!(a, Expr::Hole(_)));
                if !is_partial_application && let Expr::Ident(name, _) = callee.as_ref() {
                    let is_self_call = self.active_function.as_deref() == Some(name);
                    // Check if the callee is a user-defined function (for mutual recursion).
                    // Only attempt mutual TCO when inside a function context AND the
                    // function is NOT defined in the current (innermost) scope. Locally
                    // defined functions / lambdas are not mutual recursion targets —
                    // they would not be reachable from the trampoline after scope pop.
                    let is_user_func = !is_self_call
                        && self.active_function.is_some()
                        && matches!(self.env.get(name), Some(Value::Function(_)))
                        && !self.env.is_defined_in_current_scope(name);

                    if is_self_call || is_user_func {
                        // Tail call detected! Evaluate args and return TailCall signal
                        let mut arg_values = Vec::new();
                        for arg in args {
                            let val = match self.eval_expr(arg)? {
                                Signal::Value(v) => v,
                                Signal::TailCall(tc) => return Ok(Signal::TailCall(tc)),
                                Signal::Throw(err) => return Ok(Signal::Throw(err)),
                                Signal::Gorilla => return Ok(Signal::Gorilla),
                            };
                            arg_values.push(val);
                        }
                        if is_user_func {
                            // Mutual tail call: set the target function name
                            self.mutual_tail_call_target = Some(name.clone());
                        }
                        return Ok(Signal::TailCall(arg_values));
                    }
                }
                // Not a user-defined function call in tail position, evaluate normally
                self.eval_expr(expr)
            }

            // Condition branches: each arm body is in tail position
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    match &arm.condition {
                        Some(cond) => {
                            let cond_val = match self.eval_expr(cond)? {
                                Signal::Value(v) => v,
                                other => return Ok(other),
                            };
                            if cond_val.is_truthy() {
                                return self.eval_cond_arm_body_tail(&arm.body);
                            }
                        }
                        None => {
                            // Default case (| _ |>)
                            return self.eval_cond_arm_body_tail(&arm.body);
                        }
                    }
                }
                // No branch matched
                Ok(Signal::Value(Value::Unit))
            }

            // All other expressions: not tail-optimizable, evaluate normally
            _ => self.eval_expr(expr),
        }
    }

    /// Evaluate an expression.
    pub(crate) fn eval_expr(&mut self, expr: &Expr) -> Result<Signal, RuntimeError> {
        match expr {
            Expr::IntLit(n, _) => Ok(Signal::Value(Value::Int(*n))),
            Expr::FloatLit(n, _) => Ok(Signal::Value(Value::Float(*n))),
            Expr::StringLit(s, _) => Ok(Signal::Value(Value::str(s.clone()))),
            Expr::TemplateLit(s, _) => {
                // Template string interpolation: replace ${...} with evaluated values
                let result = self.eval_template_string(s)?;
                Ok(Signal::Value(Value::str(result)))
            }
            Expr::BoolLit(b, _) => Ok(Signal::Value(Value::Bool(*b))),
            Expr::Gorilla(_) => Ok(Signal::Gorilla),
            Expr::Placeholder(_) => Ok(Signal::Value(Value::Unit)),
            Expr::Hole(_) => Ok(Signal::Value(Value::Unit)),
            // B11-6a: TypeLiteral is only valid inside TypeIs/TypeExtends — handled by mold_eval
            Expr::TypeLiteral(name, variant, _) => {
                if let Some(var) = variant {
                    Ok(Signal::Value(Value::str(format!("{}:{}", name, var))))
                } else {
                    Ok(Signal::Value(Value::str(name.clone())))
                }
            }
            Expr::Ident(name, _) => {
                if let Some(val) = self.env.get(name) {
                    Ok(Signal::Value(val.clone()))
                } else {
                    // E30B-007 sub-step B-5 / Lock-G Sub-G4: when an
                    // undefined identifier matches the surrounding addon
                    // manifest's `[functions]` table, emit `[E1413]` with
                    // an explicit-binding hint instead of the generic message.
                    // Legacy facades that referenced bare lowercase addon
                    // function names (relying on the implicit pre-inject)
                    // hit this branch in @e.30 because the pre-inject is
                    // removed. The fix is an explicit `RustAddon[...]`
                    // binding at the top of the facade.
                    if let Some((pkg_id, manifest_arities)) = &self.loading_addon_facade_ctx
                        && let Some(arity) = manifest_arities.get(name)
                    {
                        return Err(RuntimeError {
                            message: format!(
                                "[E1413] addon facade for '{}': bare reference to addon \
                                manifest function '{}' is no longer pre-injected in @e.30 \
                                 (Lock-G Sub-G4: legacy implicit pre-inject removed). Add \
                                 an explicit binding to the top of the facade: `{} <= \
                                 RustAddon[\"{}\"](arity <= {})`. E31 does not provide an \
                                 AST migration command; update the facade manually.",
                                pkg_id, name, name, name, arity
                            ),
                        });
                    }
                    // No null/undefined — but undefined variable is a runtime error
                    Err(RuntimeError {
                        message: format!("Undefined variable: '{}'", name),
                    })
                }
            }

            Expr::BuchiPack(fields, _) => {
                let mut result_fields = Vec::new();
                for field in fields {
                    let value = match self.eval_expr(&field.value)? {
                        Signal::Value(v) => v,
                        other => return Ok(other),
                    };
                    result_fields.push((field.name.clone(), value));
                }
                Ok(Signal::Value(Value::pack(result_fields)))
            }

            Expr::ListLit(items, _) => {
                let mut result_items = Vec::new();
                for item in items {
                    let value = match self.eval_expr(item)? {
                        Signal::Value(v) => v,
                        other => return Ok(other),
                    };
                    result_items.push(value);
                }
                Ok(Signal::Value(Value::list(result_items)))
            }

            Expr::BinaryOp(left, op, right, _) => {
                let left_val = match self.eval_expr(left)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                let right_val = match self.eval_expr(right)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                self.eval_binary_op(&left_val, op, &right_val)
            }

            Expr::UnaryOp(op, inner, _) => {
                let val = match self.eval_expr(inner)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                match op {
                    UnaryOp::Neg => match val {
                        Value::Int(n) => Ok(Signal::Value(Value::Int(-n))),
                        Value::Float(n) => Ok(Signal::Value(Value::Float(-n))),
                        _ => Err(RuntimeError {
                            message: format!("Cannot negate {}", val),
                        }),
                    },
                    UnaryOp::Not => Ok(Signal::Value(Value::Bool(!val.is_truthy()))),
                }
            }

            Expr::FuncCall(callee, args, span) => {
                // Check if any argument is a Hole (empty slot) — partial application.
                // Note: Old `_` (Placeholder) partial application is rejected by checker
                // (E1502) before reaching this point. Only Hole-based empty-slot syntax
                // `f(5, )` is handled here.
                let has_hole = args.iter().any(|a| matches!(a, Expr::Hole(_)));

                if has_hole {
                    return self.eval_partial_application(callee, args, span);
                }

                // Check built-in functions FIRST (before evaluating callee)
                if let Expr::Ident(name, _) = callee.as_ref()
                    && let Some(result) = self.try_builtin_func(name, args)?
                {
                    return Ok(result);
                }

                let callee_val = match self.eval_expr(callee)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };

                match callee_val {
                    Value::Function(func) => self.call_function(&func, args),
                    _ => Err(RuntimeError {
                        message: format!(
                            "Cannot call non-function value: {}",
                            callee_val.to_error_display(200)
                        ),
                    }),
                }
            }

            Expr::MethodCall(obj, method, args, _) => {
                let obj_val = match self.eval_expr(obj)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                self.eval_method_call(&obj_val, method, args)
            }

            Expr::FieldAccess(obj, field, _) => {
                let obj_val = match self.eval_expr(obj)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                match &obj_val {
                    Value::BuchiPack(fields) => {
                        if let Some((_, val)) = fields.iter().find(|(n, _)| n == field) {
                            Ok(Signal::Value(val.clone()))
                        } else {
                            Err(RuntimeError {
                                message: format!("Field '{}' does not exist", field),
                            })
                        }
                    }
                    Value::Error(_) => {
                        if let Some(val) = obj_val.get_error_field(field) {
                            Ok(Signal::Value(val))
                        } else {
                            Err(RuntimeError {
                                message: format!("Field '{}' does not exist on error", field),
                            })
                        }
                    }
                    _ => Err(RuntimeError {
                        message: format!(
                            "Cannot access field '{}' on {}",
                            field,
                            obj_val.to_error_display(200)
                        ),
                    }),
                }
            }

            // IndexAccess removed in v0.5.0 — use .get(i) instead
            Expr::CondBranch(arms, _) => self.eval_cond_branch(arms),

            Expr::Pipeline(exprs, _) => {
                // Pipeline: evaluate left to right, passing result through each step
                // Each step can be:
                //   - FuncCall with _ (Placeholder): replace _ with current value and call
                //   - FuncCall without _: pass current as first argument
                //   - Ident: assign current to that variable (handled at statement level)
                //   - Other expression: evaluate with _ bound to current
                //
                // C13-1 / C13B-007: In a pure `=>` pipeline, an intermediate
                // `=> name` step (where `name` is not an already-defined
                // function) acts as a **bind-and-forward**: the current
                // pipeline value is bound to `name` *and* passed through
                // unchanged so later steps can reference it.  The binding
                // is scoped to the remainder of this pipeline statement.
                //
                // When a later step explicitly references a bound name,
                // classic auto-injection of `current` as the first argument
                // is suppressed — the user has taken explicit control via
                // the named binding.
                let last_idx = exprs.len().saturating_sub(1);
                let mut bound_any = false;
                let mut bound_names: Vec<String> = Vec::new();
                let mut current = Value::Unit;
                for (i, expr) in exprs.iter().enumerate() {
                    if i == 0 {
                        current = match self.eval_expr(expr)? {
                            Signal::Value(v) => v,
                            other => {
                                if bound_any {
                                    self.env.pop_scope();
                                }
                                return Ok(other);
                            }
                        };
                    } else if i < last_idx
                        && let Expr::Ident(name, _) = expr
                        && !self.is_pipeline_bindable_callable(name)
                    {
                        // Intermediate `=> name`: bind-and-forward.
                        if !bound_any {
                            self.env.push_scope();
                            bound_any = true;
                        }
                        self.env.define_force(name, current.clone());
                        bound_names.push(name.clone());
                        // current is unchanged; continue to next step.
                    } else if !bound_names.is_empty()
                        && Self::expr_references_any(expr, &bound_names)
                    {
                        // The user explicitly references one of the
                        // pipeline-scope bindings inside this step. Evaluate
                        // the step as-written without auto-injecting
                        // `current` as an extra argument.
                        current = match self.eval_expr(expr)? {
                            Signal::Value(v) => v,
                            other => {
                                if bound_any {
                                    self.env.pop_scope();
                                }
                                return Ok(other);
                            }
                        };
                    } else {
                        current = match self.eval_pipeline_step(expr, current)? {
                            Signal::Value(v) => v,
                            other => {
                                if bound_any {
                                    self.env.pop_scope();
                                }
                                return Ok(other);
                            }
                        };
                    }
                }
                if bound_any {
                    self.env.pop_scope();
                }
                Ok(Signal::Value(current))
            }

            Expr::MoldInst(name, type_args, fields, span) => {
                // E30B-007 / Lock-G: explicit addon-binding form
                // `RustAddon["fn"](arity <= N)`. Inside an addon facade
                // load context (`loading_addon_facade_ctx`) this expression
                // resolves to the addon sentinel string; the `arity <= N`
                // metadata is validated against the manifest `[functions]`
                // table and any drift is rejected as `[E1412]`.
                //
                // Outside facade context the form is a structural error
                // (binding only makes sense in a facade). The interpreter
                // surfaces a `[E1412]` runtime error here as a defensive
                // diagnostic; the checker surfacing of this code at compile
                // time lands in sub-step B-5 once the terminal addon
                // submodule has been migrated by the TM track.
                if name == "RustAddon" {
                    return self.eval_rust_addon_binding(type_args, fields, span);
                }

                // Check for new operation mold types (str, num, list with fields support)
                if let Some(result) = self.try_operation_mold(name, type_args, fields)? {
                    return Ok(result);
                }

                // RC2B-207: Addon-backed "operation mold" bridge.
                //
                // The RC2 design uses mold syntax for effectful addon
                // queries (`TerminalSize[]()`, `ReadKey[]()`). When a
                // mold name is bound in the current env to an addon
                // sentinel (`Value::str("__taida_addon_call::...")`),
                // we dispatch through the addon call path with the
                // positional `type_args` as arguments and no named
                // fields. This preserves the normal mold path for
                // every non-addon identifier so there is zero change
                // to Str/JSON/user-defined mold behaviour.
                //
                // `fields` (named `foo <= bar` slots) are forbidden on
                // addon molds because the addon ABI has positional
                // arity only; mixing them would require a calling
                // convention the addon surface does not have.
                #[cfg(feature = "native")]
                if let Some(Value::Str(tag)) = self.env.get(name)
                    && tag.starts_with("__taida_addon_call::")
                {
                    if !fields.is_empty() {
                        return Err(RuntimeError {
                            message: format!(
                                "Addon mold '{}' does not accept named fields. \
                                 Pass arguments positionally: {}[arg1, arg2]().",
                                name, name
                            ),
                        });
                    }
                    if let Some(signal) = self.try_addon_func(name, type_args)? {
                        return Ok(signal);
                    }
                }

                // C20B-014 (ROOT-17): User-defined function called via mold syntax.
                //
                // Pre-C20B-014, `Fn[arg, arg]()` for a user-defined function
                // `Fn` silently entered the generic mold-wrap path below and
                // returned `@(__value <= first_arg, __type <= "Fn")` instead
                // of invoking the function. `taida check` did not detect this
                // (checker fell through to `Type::Unknown`), the Interpreter
                // silently wrapped, Native failed at lowering with
                // "unsupported mold type", and only JS accidentally worked
                // (its `__taida_solidify(Fn(...))` fallback calls the fn).
                //
                // Fix: if `name` resolves to a `Value::Function` in scope
                // AND no MoldDef is registered for the same name, dispatch
                // the call as `Fn(args)` with `type_args` used positionally.
                // `fields` (named `k <= v` slots) are rejected — user
                // functions have no named-field ABI.
                //
                // Guard ordering: this branch runs *after* builtin molds
                // (`try_operation_mold`) and addon sentinels, so shadowing
                // a builtin mold name with a local user fn does not change
                // behaviour. It runs *before* generic mold instantiation,
                // so user fns are no longer wrapped.
                if !self.mold_defs.contains_key(name)
                    && let Some(Value::Function(func)) = self.env.get(name).cloned()
                {
                    if !fields.is_empty() {
                        return Err(RuntimeError {
                            message: format!(
                                "User-defined function '{}' called via mold syntax \
                                 cannot accept named fields '()'. \
                                 Pass arguments positionally: {}[arg1, arg2]() or {}(arg1, arg2).",
                                name, name, name
                            ),
                        });
                    }
                    return self.call_function(&func, type_args);
                }

                // Generic/custom mold instantiation.
                let mut named_values = HashMap::<String, Value>::new();
                for field in fields {
                    let value = match self.eval_expr(&field.value)? {
                        Signal::Value(v) => v,
                        other => return Ok(other),
                    };
                    named_values.insert(field.name.clone(), value);
                }

                let mut positional_values = Vec::<Value>::new();
                for arg in type_args {
                    let value = match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(other),
                    };
                    positional_values.push(value);
                }
                let first_type_arg = positional_values.first().cloned();
                let mut result_fields = Vec::<(String, Value)>::new();

                // Check MoldDef for `solidify` / `unmold` definitions
                let mold_fields = self.mold_defs.get(name).cloned();
                let solidify_method = mold_fields.as_ref().and_then(|fields| {
                    fields
                        .iter()
                        .find(|f| f.name == "solidify" && f.is_method)
                        .and_then(|f| f.method_def.clone())
                });
                let unmold_method = mold_fields.as_ref().and_then(|fields| {
                    fields
                        .iter()
                        .find(|f| f.name == "unmold" && f.is_method)
                        .and_then(|f| f.method_def.clone())
                });

                if let Some(defs) = &mold_fields {
                    // `filling` is always the first positional argument.
                    if let Some(ref val) = first_type_arg {
                        result_fields.push(("filling".to_string(), val.clone()));
                    }

                    // Additional positional args map to non-default fields (decl-order).
                    let mut consumed = HashSet::<String>::new();
                    let mut positional_iter = positional_values.iter().skip(1);
                    for field_def in defs.iter().filter(|f| {
                        !f.is_method && f.name != "filling" && f.default_value.is_none()
                    }) {
                        if let Some(value) = positional_iter.next() {
                            result_fields.push((field_def.name.clone(), value.clone()));
                            consumed.insert(field_def.name.clone());
                        }
                    }

                    // Remaining fields: named values first, then default values.
                    //
                    // E30 Phase 6 / E30B-004: declare-only function fields
                    // (E30B-002) have `default_value.is_none()` but are still
                    // materialised here so that their synthetic defaultFn
                    // (Lock-D verdict) is bound on the result pack — without
                    // this, `m.declareOnlyField(...)` would fail with
                    // `Unknown method` since the field would be absent. The
                    // synthetic FuncValue itself is produced by
                    // `default_for_field_def` → `default_for_type_expr`.
                    for field_def in defs.iter().filter(|f| !f.is_method && f.name != "filling") {
                        if consumed.contains(&field_def.name) {
                            continue;
                        }
                        if let Some(value) = named_values.get(&field_def.name) {
                            result_fields.push((field_def.name.clone(), value.clone()));
                            consumed.insert(field_def.name.clone());
                            continue;
                        }
                        if field_def.default_value.is_some() || field_def.is_declare_only_fn_field()
                        {
                            let mut visiting = HashSet::new();
                            let default_val =
                                self.default_for_field_def(field_def, &mut visiting)?;
                            result_fields.push((field_def.name.clone(), default_val));
                            consumed.insert(field_def.name.clone());
                        }
                    }

                    // Preserve undeclared named options for runtime compatibility.
                    for (name, value) in &named_values {
                        if name != "filling" && !consumed.contains(name) {
                            result_fields.push((name.clone(), value.clone()));
                        }
                    }
                } else {
                    // Unknown mold name at runtime: keep named fields as-is.
                    for (name, value) in named_values {
                        result_fields.push((name, value));
                    }
                }

                // Store type args as __value (first arg) for unmolding
                if let Some(ref val) = first_type_arg {
                    result_fields.push(("__value".to_string(), val.clone()));
                }

                // If MoldDef defines a custom `unmold` method, build a closure and store as __unmold.
                // The closure captures `filling` (the type arg value) so the unmold body can reference it.
                if let Some(ref func_def) = unmold_method {
                    let mut closure = HashMap::new();
                    // Inject `filling` into closure so the unmold body can access it
                    if let Some(ref val) = first_type_arg {
                        closure.insert("filling".to_string(), val.clone());
                    }
                    // Also inject all instance fields into the closure
                    for (field_name, field_val) in &result_fields {
                        if field_name != "__type"
                            && field_name != "__value"
                            && field_name != "__unmold"
                        {
                            closure.insert(field_name.clone(), field_val.clone());
                        }
                    }
                    let unmold_func = Value::Function(FuncValue {
                        name: "__unmold".to_string(),
                        params: func_def.params.clone(),
                        body: func_def.body.clone(),
                        closure: Arc::new(closure),
                        return_type: func_def.return_type.clone(),
                        module_type_defs: None,
                        module_enum_defs: None,
                    });
                    result_fields.push(("__unmold".to_string(), unmold_func));
                }

                // Add a __type field to track the type
                result_fields.push(("__type".to_string(), Value::str(name.clone())));
                let instance = Value::pack(result_fields.clone());

                // `solidify` overrides what Name[args]() evaluates to.
                if let Some(ref func_def) = solidify_method {
                    let mut closure = HashMap::new();
                    for (field_name, field_val) in &result_fields {
                        closure.insert(field_name.clone(), field_val.clone());
                    }
                    closure.insert("self".to_string(), instance.clone());
                    let solidify_func = FuncValue {
                        name: "__solidify".to_string(),
                        params: func_def.params.clone(),
                        body: func_def.body.clone(),
                        closure: Arc::new(closure),
                        return_type: func_def.return_type.clone(),
                        module_type_defs: None,
                        module_enum_defs: None,
                    };
                    return self.call_function_preserving_signals(&solidify_func, &[]);
                }

                Ok(Signal::Value(instance))
            }

            Expr::Unmold(inner, _) => {
                // Unmold: extract value from Mold wrapper
                let val = match self.eval_expr(inner)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                Ok(Signal::Value(val))
            }

            Expr::Lambda(params, body, _) => {
                let closure = Arc::new(self.env.snapshot());
                Ok(Signal::Value(Value::Function(FuncValue {
                    name: "<lambda>".to_string(),
                    params: params.clone(),
                    body: vec![Statement::Expr(*body.clone())],
                    closure,
                    return_type: None,
                    module_type_defs: None,
                    module_enum_defs: None,
                })))
            }

            Expr::EnumVariant(enum_name, variant_name, _) => {
                let ordinal = self
                    .enum_defs
                    .get(enum_name)
                    .and_then(|variants| {
                        variants.iter().position(|variant| variant == variant_name)
                    })
                    .ok_or_else(|| RuntimeError {
                        message: format!(
                            "Unknown enum variant: '{}:{}()'",
                            enum_name, variant_name
                        ),
                    })?;
                // C18-2 / C18-4: Return a tagged `EnumVal` so jsonEncode can
                // emit the variant name and Ordinal[] / ordering can assert
                // the source enum. `EnumVal` compares equal to
                // `Value::Int(ordinal)` (see `impl PartialEq`) so existing
                // callers that compared against Int ordinals still work.
                Ok(Signal::Value(Value::EnumVal(
                    enum_name.clone(),
                    ordinal as i64,
                )))
            }

            Expr::TypeInst(name, fields, _) => {
                let mut provided_fields = Vec::new();
                for field in fields {
                    let value = match self.eval_expr(&field.value)? {
                        Signal::Value(v) => v,
                        other => return Ok(other),
                    };
                    provided_fields.push((field.name.clone(), value));
                }

                let mut result_fields = Vec::new();
                // If TypeDef exists, inject defaults for omitted typed/defaulted fields.
                if let Some(type_fields) = self.type_defs.get(name).cloned() {
                    let mut consumed = HashSet::new();
                    let mut visiting = HashSet::new();
                    for field_def in type_fields.iter().filter(|f| !f.is_method) {
                        if let Some((_, provided)) = provided_fields
                            .iter()
                            .rev()
                            .find(|(n, _)| n == &field_def.name)
                        {
                            result_fields.push((field_def.name.clone(), provided.clone()));
                            consumed.insert(field_def.name.clone());
                        } else {
                            let default_val =
                                self.default_for_field_def(field_def, &mut visiting)?;
                            result_fields.push((field_def.name.clone(), default_val));
                        }
                    }
                    // Preserve extra undeclared fields for structural flexibility.
                    for (name, value) in provided_fields {
                        if !consumed.contains(&name) {
                            result_fields.push((name, value));
                        }
                    }
                } else {
                    result_fields = provided_fields;
                }
                result_fields.push(("__type".to_string(), Value::str(name.clone())));
                Ok(Signal::Value(Value::pack(result_fields)))
            }

            Expr::Throw(inner, _) => {
                let val = match self.eval_expr(inner)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                Ok(Signal::Throw(val))
            }
        }
    }

    fn default_for_field_def(
        &mut self,
        field: &FieldDef,
        visiting: &mut HashSet<String>,
    ) -> Result<Value, RuntimeError> {
        if let Some(default_expr) = &field.default_value {
            return match self.eval_expr(default_expr)? {
                Signal::Value(v) => Ok(v),
                Signal::Throw(err) => Err(RuntimeError {
                    message: format!(
                        "Failed to evaluate default value for field '{}': throw({})",
                        field.name, err
                    ),
                }),
                Signal::TailCall(_) => Err(RuntimeError {
                    message: format!(
                        "Failed to evaluate default value for field '{}': tail call is not allowed",
                        field.name
                    ),
                }),
                Signal::Gorilla => Err(RuntimeError {
                    message: format!(
                        "Failed to evaluate default value for field '{}': gorilla",
                        field.name
                    ),
                }),
            };
        }

        if let Some(type_expr) = &field.type_annotation {
            return self.default_for_type_expr(type_expr, visiting);
        }

        Ok(Value::Unit)
    }

    fn default_for_type_expr(
        &mut self,
        type_expr: &TypeExpr,
        visiting: &mut HashSet<String>,
    ) -> Result<Value, RuntimeError> {
        match type_expr {
            TypeExpr::Named(name) => match name.as_str() {
                "Int" | "Num" => Ok(Value::Int(0)),
                "Float" => Ok(Value::Float(0.0)),
                "Str" => Ok(Value::str(String::new())),
                "Bytes" => Ok(Value::bytes(Vec::new())),
                "Bool" => Ok(Value::Bool(false)),
                "JSON" => Ok(Value::default_json()),
                "Molten" => Ok(Value::default_molten()),
                _ => {
                    if visiting.contains(name) {
                        return Ok(Value::pack(vec![(
                            "__type".to_string(),
                            Value::str(name.clone()),
                        )]));
                    }
                    if let Some(type_fields) = self.type_defs.get(name).cloned() {
                        visiting.insert(name.clone());
                        let mut fields = Vec::new();
                        for field_def in type_fields.iter().filter(|f| !f.is_method) {
                            let default_val = self.default_for_field_def(field_def, visiting)?;
                            fields.push((field_def.name.clone(), default_val));
                        }
                        visiting.remove(name);
                        fields.push(("__type".to_string(), Value::str(name.clone())));
                        Ok(Value::pack(fields))
                    } else if self.enum_defs.contains_key(name) {
                        Ok(Value::EnumVal(name.clone(), 0))
                    } else {
                        Ok(Value::Unit)
                    }
                }
            },
            TypeExpr::List(_) => Ok(Value::list(Vec::new())),
            TypeExpr::BuchiPack(fields) => {
                let mut result = Vec::new();
                for field_def in fields.iter().filter(|f| !f.is_method) {
                    let default_val = self.default_for_field_def(field_def, visiting)?;
                    result.push((field_def.name.clone(), default_val));
                }
                Ok(Value::pack(result))
            }
            TypeExpr::Generic(name, args) => {
                if name == "Lax" {
                    let inner = if let Some(inner_ty) = args.first() {
                        self.default_for_type_expr(inner_ty, visiting)?
                    } else {
                        Value::Unit
                    };
                    return Ok(Value::pack(vec![
                        ("hasValue".to_string(), Value::Bool(false)),
                        ("__value".to_string(), inner.clone()),
                        ("__default".to_string(), inner),
                        ("__type".to_string(), Value::str("Lax".to_string())),
                    ]));
                }
                // E30 Phase 6 / E30B-004 / Lock-D: `Async[T]` defaultFn
                // resolves immediately with `T`'s default value.
                if name == "Async" {
                    let inner = if let Some(inner_ty) = args.first() {
                        self.default_for_type_expr(inner_ty, visiting)?
                    } else {
                        Value::Unit
                    };
                    return Ok(Value::Async(crate::interpreter::value::AsyncValue {
                        status: crate::interpreter::value::AsyncStatus::Fulfilled,
                        value: Box::new(inner),
                        error: Box::new(Value::Unit),
                        task: None,
                    }));
                }
                Ok(Value::Unit)
            }
            // E30 Phase 6 / E30B-004 / Lock-D verdict: synthesise a
            // `defaultFn` value for declare-only function fields. The
            // returned `Value::Function` carries a sentinel name and
            // the original return type; `call_function` short-circuits
            // calls on it to materialise the return type's default value
            // without executing any AST body.
            //
            // PHILOSOPHY: completes "全型にデフォルト値保証 / null/undefined
            // 排除" — function-typed fields previously fell back to
            // `Value::Unit` placeholder which crashes on call. The
            // synthetic defaultFn returns the proper type-default on call
            // so that "デフォルト値保証" extends to function types.
            TypeExpr::Function(params, ret) => {
                let synth_params: Vec<Param> = params
                    .iter()
                    .enumerate()
                    .map(|(i, ty)| Param {
                        name: format!("__arg_{}", i),
                        type_annotation: Some(ty.clone()),
                        default_value: None,
                        span: crate::lexer::Span::new(0, 0, 0, 0),
                    })
                    .collect();
                Ok(Value::Function(FuncValue {
                    name: Self::DEFAULT_FN_SENTINEL_NAME.to_string(),
                    params: synth_params,
                    body: Vec::new(),
                    closure: Arc::new(HashMap::new()),
                    return_type: Some((**ret).clone()),
                    module_type_defs: None,
                    module_enum_defs: None,
                }))
            }
        }
    }

    /// Evaluate an empty-slot partial application: `func(arg1, , arg3)` returns a closure.
    /// The closure, when called with the missing args, fills in the holes (Hole nodes).
    /// Note: Old `_` (Placeholder) partial application is rejected by checker (E1502)
    /// and never reaches this code path.
    fn eval_partial_application(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        _span: &crate::lexer::Span,
    ) -> Result<Signal, RuntimeError> {
        // Evaluate the callee and non-hole arguments
        let mut evaluated_args: Vec<Option<Value>> = Vec::new();
        for arg in args {
            if matches!(arg, Expr::Hole(_)) {
                evaluated_args.push(None);
            } else {
                let val = match self.eval_expr(arg)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                evaluated_args.push(Some(val));
            }
        }

        // Count placeholders — these become the params of the new closure
        let placeholder_count = evaluated_args.iter().filter(|a| a.is_none()).count();

        // Generate parameter names for the closure
        let params: Vec<Param> = (0..placeholder_count)
            .map(|i| Param {
                name: format!("__partial_arg_{}", i),
                type_annotation: None,
                default_value: None,
                span: crate::lexer::Span::new(0, 0, 0, 0),
            })
            .collect();

        // Store pre-evaluated args in the environment so the closure can capture them
        self.env.push_scope();
        let mut captured_arg_names: Vec<Option<String>> = Vec::new();
        for (i, ea) in evaluated_args.iter().enumerate() {
            match ea {
                Some(val) => {
                    let name = format!("__partial_captured_{}", i);
                    self.env.define_force(&name, val.clone());
                    captured_arg_names.push(Some(name));
                }
                None => {
                    captured_arg_names.push(None);
                }
            }
        }

        // Build the body: reconstruct a FuncCall with placeholders replaced by param refs,
        // and captured args replaced by their stored names
        let mut new_args: Vec<Expr> = Vec::new();
        let mut placeholder_idx = 0;
        let dummy_span = crate::lexer::Span::new(0, 0, 0, 0);
        for ca in &captured_arg_names {
            match ca {
                Some(name) => {
                    new_args.push(Expr::Ident(name.clone(), dummy_span.clone()));
                }
                None => {
                    let param_name = format!("__partial_arg_{}", placeholder_idx);
                    new_args.push(Expr::Ident(param_name, dummy_span.clone()));
                    placeholder_idx += 1;
                }
            }
        }

        let body_expr = Expr::FuncCall(Box::new(callee.clone()), new_args, dummy_span.clone());
        let body = vec![Statement::Expr(body_expr)];

        let closure = Arc::new(self.env.snapshot());
        self.env.pop_scope();

        Ok(Signal::Value(Value::Function(FuncValue {
            name: "<partial>".to_string(),
            params,
            body,
            closure,
            return_type: None,
            module_type_defs: None,
            module_enum_defs: None,
        })))
    }

    /// Evaluate an error ceiling block.
    ///
    /// Note: Error ceiling handling is primarily done in `eval_statements`.
    /// This method is only called when an error ceiling appears as an
    /// isolated statement (which should not happen in well-formed code).
    fn eval_error_ceiling(&mut self, _ec: &ErrorCeiling) -> Result<Signal, RuntimeError> {
        // Error ceilings are handled in eval_statements where they can
        // wrap the subsequent statements. If we reach here, it means
        // the error ceiling is not followed by any protected code.
        Ok(Signal::Value(Value::Unit))
    }

    // ── JSON Schema Resolution ─────────────────────────────────

    /// Resolve a JSON schema from an expression AST node.
    /// The schema expression is not evaluated as a value — it is interpreted
    /// as a type descriptor:
    ///   - Ident("Int"/"Str"/"Float"/"Bool") → primitive schema
    ///   - Ident("User") → look up TypeDef by name
    ///   - ListLit([Ident("Pilot")]) → list schema with element type
    pub(crate) fn resolve_json_schema(
        &self,
        expr: &Expr,
    ) -> Result<crate::interpreter::json::JsonSchema, RuntimeError> {
        use crate::interpreter::json::{JsonSchema, PrimitiveType, build_schema_from_typedef};

        match expr {
            Expr::Ident(name, _) => match name.as_str() {
                "Int" => Ok(JsonSchema::Primitive(PrimitiveType::Int)),
                "Str" => Ok(JsonSchema::Primitive(PrimitiveType::Str)),
                "Float" => Ok(JsonSchema::Primitive(PrimitiveType::Float)),
                "Bool" => Ok(JsonSchema::Primitive(PrimitiveType::Bool)),
                type_name => {
                    // C16: TypeDef wins over Enum when both exist (collision
                    // is currently disallowed by the parser, but keep the
                    // precedence explicit for future-proofing).
                    if let Some(fields) = self.type_defs.get(type_name) {
                        Ok(build_schema_from_typedef(
                            type_name,
                            fields,
                            &self.type_defs,
                            &self.enum_defs,
                        ))
                    } else if let Some(variants) = self.enum_defs.get(type_name) {
                        // C16: Enum as top-level JSON schema. On match returns
                        // Value::Int(ordinal); on mismatch returns Lax[Enum].
                        Ok(JsonSchema::Enum(type_name.to_string(), variants.clone()))
                    } else {
                        Err(RuntimeError {
                            message: format!(
                                "Unknown schema type '{}' for JSON casting. Define it as a TypeDef first: {} = @(...)",
                                type_name, type_name
                            ),
                        })
                    }
                }
            },
            // @[Schema] — list type
            Expr::ListLit(items, _) => {
                if items.len() == 1 {
                    let elem_schema = self.resolve_json_schema(&items[0])?;
                    Ok(JsonSchema::List(Box::new(elem_schema)))
                } else if items.is_empty() {
                    Err(RuntimeError {
                        message: "List schema @[] requires an element type: @[TypeName]"
                            .to_string(),
                    })
                } else {
                    Err(RuntimeError {
                        message:
                            "List schema @[...] must have exactly one element type: @[TypeName]"
                                .to_string(),
                    })
                }
            }
            _ => Err(RuntimeError {
                message:
                    "JSON schema must be a type name (e.g., User, Int) or list type (e.g., @[User])"
                        .to_string(),
            }),
        }
    }

    fn bind_params_with_effective_defaults(
        &mut self,
        func: &FuncValue,
        arg_values: &[Value],
    ) -> Result<Option<Signal>, RuntimeError> {
        let enforce_arity = func.name != "<lambda>" && func.name != "<partial>";
        if enforce_arity && arg_values.len() > func.params.len() {
            return Err(RuntimeError {
                message: format!(
                    "Function '{}' expected at most {} argument(s), got {}",
                    func.name,
                    func.params.len(),
                    arg_values.len()
                ),
            });
        }

        for (i, param) in func.params.iter().enumerate() {
            let val = if i < arg_values.len() {
                arg_values[i].clone()
            } else if let Some(default_expr) = &param.default_value {
                match self.eval_expr(default_expr)? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                }
            } else if let Some(type_ann) = &param.type_annotation {
                let mut visiting = std::collections::HashSet::new();
                self.default_for_type_expr(type_ann, &mut visiting)?
            } else {
                Value::Unit
            };
            self.env.define_force(&param.name, val);
        }

        Ok(None)
    }

    /// C20B-015 / ROOT-18: Overlay a function's defining-module TypeDef / enum
    /// registries onto the interpreter's live registries before executing the
    /// function body. Returns the previous state so the caller can restore it
    /// after the body runs.
    ///
    /// Lambdas / partials / internal helpers have `module_type_defs == None`
    /// and this method is a no-op for them — they must see whatever TypeDefs
    /// the currently-executing caller sees (that is the lexical-scope rule
    /// for functions defined inline).
    ///
    /// Overlay semantics: defining-module entries are layered *under* the
    /// current entries, i.e. a local TypeDef with the same name still wins.
    /// This preserves F-56 behaviour (caller's typedef shadows imported one)
    /// while ensuring JSON schema resolution inside the imported function
    /// body has access to every symbol the defining module saw.
    #[allow(clippy::type_complexity)]
    pub(crate) fn push_func_module_scope(
        &mut self,
        func: &FuncValue,
    ) -> (
        HashMap<String, Vec<FieldDef>>,
        HashMap<String, Vec<String>>,
        bool,
    ) {
        let prev_td = self.type_defs.clone();
        let prev_ed = self.enum_defs.clone();
        let mut changed = false;
        if let Some(mtd) = &func.module_type_defs {
            for (k, v) in mtd.iter() {
                self.type_defs.entry(k.clone()).or_insert_with(|| v.clone());
            }
            changed = true;
        }
        if let Some(med) = &func.module_enum_defs {
            for (k, v) in med.iter() {
                self.enum_defs.entry(k.clone()).or_insert_with(|| v.clone());
            }
            changed = true;
        }
        (prev_td, prev_ed, changed)
    }

    /// Restore TypeDef / enum registries that were captured by
    /// `push_func_module_scope`. No-op if `push_func_module_scope` reported
    /// `changed == false`.
    pub(crate) fn pop_func_module_scope(
        &mut self,
        prev_td: HashMap<String, Vec<FieldDef>>,
        prev_ed: HashMap<String, Vec<String>>,
        changed: bool,
    ) {
        if changed {
            self.type_defs = prev_td;
            self.enum_defs = prev_ed;
        }
    }

    /// E30 Phase 6 / E30B-004: Materialise the synthetic defaultFn return.
    ///
    /// Called from `call_function` / `call_function_with_values` /
    /// `call_function_preserving_signals` whenever a `Value::Function` whose
    /// `name` equals `DEFAULT_FN_SENTINEL_NAME` is invoked. The body is
    /// intentionally empty; we only ever produce the return type's default.
    pub(crate) fn invoke_default_fn(&mut self, func: &FuncValue) -> Result<Value, RuntimeError> {
        let ret_ty = match &func.return_type {
            Some(t) => t.clone(),
            None => return Ok(Value::Unit),
        };
        let mut visiting = HashSet::new();
        self.default_for_type_expr(&ret_ty, &mut visiting)
    }

    /// Helper: call a function with pre-evaluated argument values,
    /// preserving all Signal variants (including Throw) for the caller to handle.
    /// Used for __unmold calls where Signal::Throw must be catchable by |==.
    pub(crate) fn call_function_preserving_signals(
        &mut self,
        func: &FuncValue,
        arg_values: &[Value],
    ) -> Result<Signal, RuntimeError> {
        // RCB-301: depth guard
        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            return Err(RuntimeError {
                message: format!(
                    "Maximum call depth ({}) exceeded. Use tail recursion or restructure the code.",
                    MAX_CALL_DEPTH
                ),
            });
        }

        // E30 Phase 6 / E30B-004: synthetic defaultFn fast path.
        if func.name == Self::DEFAULT_FN_SENTINEL_NAME {
            self.call_depth -= 1;
            let _ = arg_values; // arity is enforced by the type, the body is no-op
            return self.invoke_default_fn(func).map(Signal::Value);
        }
        // Internal callback paths (list ops, unmold hooks) should not inherit
        // outer-function tail-call context.
        let prev_active = self.active_function.take();
        // C20B-015 / ROOT-18: overlay defining-module typedef scope
        let (prev_td, prev_ed, td_changed) = self.push_func_module_scope(func);
        // Closure scope
        self.env.push_scope();
        for (name, val) in func.closure.iter() {
            self.env.define_force(name, val.clone());
        }
        // Local scope for parameters and body
        self.env.push_scope();
        if let Some(signal) = self.bind_params_with_effective_defaults(func, arg_values)? {
            self.env.pop_scope();
            self.env.pop_scope();
            self.pop_func_module_scope(prev_td, prev_ed, td_changed);
            self.active_function = prev_active;
            self.call_depth -= 1;
            return Ok(signal);
        }

        // C20B-015 / ROOT-18 (3rd reopen): body evaluation must NOT use `?`.
        // A `RuntimeError` from the body would skip every cleanup step below,
        // leaving the overlayed TypeDef / enum registries, pushed scopes,
        // saved `active_function` and bumped `call_depth` in place. In REPL
        // mode the interpreter is reused across inputs, so the leak lets a
        // subsequent `JSON[raw, Schema]()` silently resolve against an
        // imported module's private typedefs. Capture the result, run the
        // cleanup path unconditionally, then propagate.
        let result = self.eval_statements(&func.body);
        self.env.pop_scope(); // pop local scope
        self.env.pop_scope(); // pop closure scope
        self.pop_func_module_scope(prev_td, prev_ed, td_changed);
        self.active_function = prev_active;
        self.call_depth -= 1;

        result
    }

    /// Helper: call a function with pre-evaluated argument values.
    pub(crate) fn call_function_with_values(
        &mut self,
        func: &FuncValue,
        arg_values: &[Value],
    ) -> Result<Value, RuntimeError> {
        // RCB-301: depth guard
        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            return Err(RuntimeError {
                message: format!(
                    "Maximum call depth ({}) exceeded. Use tail recursion or restructure the code.",
                    MAX_CALL_DEPTH
                ),
            });
        }

        // E30 Phase 6 / E30B-004: synthetic defaultFn fast path.
        if func.name == Self::DEFAULT_FN_SENTINEL_NAME {
            self.call_depth -= 1;
            let _ = arg_values;
            return self.invoke_default_fn(func);
        }

        // Internal callback paths (Map/Filter/etc.) should not inherit
        // outer-function tail-call context.
        let prev_active = self.active_function.take();
        // C20B-015 / ROOT-18: overlay defining-module typedef scope
        let (prev_td, prev_ed, td_changed) = self.push_func_module_scope(func);
        // Closure scope
        self.env.push_scope();
        for (name, val) in func.closure.iter() {
            self.env.define_force(name, val.clone());
        }
        // Local scope for parameters and body
        self.env.push_scope();
        if let Some(signal) = self.bind_params_with_effective_defaults(func, arg_values)? {
            self.env.pop_scope(); // pop local scope
            self.env.pop_scope(); // pop closure scope
            self.pop_func_module_scope(prev_td, prev_ed, td_changed);
            self.active_function = prev_active;
            self.call_depth -= 1;
            return match signal {
                Signal::Value(v) => Ok(v),
                Signal::TailCall(_) => Err(RuntimeError {
                    message: "Unexpected tail call in list operation".to_string(),
                }),
                Signal::Throw(err) => {
                    // Store thrown value so that error ceiling can recover it
                    self.pending_throw = Some(err.clone());
                    Err(RuntimeError {
                        message: format!("Unhandled error in list operation: {}", err),
                    })
                }
                Signal::Gorilla => Ok(Value::Gorilla),
            };
        }

        // C20B-015 / ROOT-18 (3rd reopen): body evaluation must NOT use `?`.
        // Same cleanup invariant as `call_function_preserving_signals`: the
        // overlayed TypeDef / enum registries and pushed scopes must be
        // torn down before propagating any `RuntimeError` from the body.
        let body_result = self.eval_statements(&func.body);
        self.env.pop_scope(); // pop local scope
        self.env.pop_scope(); // pop closure scope
        self.pop_func_module_scope(prev_td, prev_ed, td_changed);
        self.active_function = prev_active;
        self.call_depth -= 1;

        match body_result {
            Ok(Signal::Value(v)) => Ok(v),
            Ok(Signal::TailCall(_)) => Err(RuntimeError {
                message: "Unexpected tail call in list operation".to_string(),
            }),
            Ok(Signal::Throw(err)) => {
                // Store thrown value so that error ceiling can recover it
                self.pending_throw = Some(err.clone());
                Err(RuntimeError {
                    message: format!("Unhandled error in list operation: {}", err),
                })
            }
            Ok(Signal::Gorilla) => Ok(Value::Gorilla),
            Err(err) => Err(err),
        }
    }

    // ── Function Calls ──────────────────────────────────────

    /// Call a function with arguments, with tail call optimization.
    ///
    /// When a function makes a tail call (self-recursive or mutual-recursive),
    /// the interpreter returns a TailCall signal. This method loops
    /// (trampoline) on TailCall signals instead of growing the stack.
    ///
    /// For mutual recursion, `mutual_tail_call_target` is set by eval_expr_tail
    /// to indicate that the next iteration should execute a different function.
    fn call_function(&mut self, func: &FuncValue, args: &[Expr]) -> Result<Signal, RuntimeError> {
        // RCB-301: Guard against stack overflow from deeply recursive calls.
        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            return Err(RuntimeError {
                message: format!(
                    "Maximum call depth ({}) exceeded. Use tail recursion or restructure the code.",
                    MAX_CALL_DEPTH
                ),
            });
        }

        // Evaluate arguments
        let mut arg_values = Vec::new();
        for arg in args {
            let val = match self.eval_expr(arg)? {
                Signal::Value(v) => v,
                other => return Ok(other),
            };
            arg_values.push(val);
        }

        // E30 Phase 6 / E30B-004: synthetic defaultFn fast path.
        //
        // A `Value::Function` whose name is `Self::DEFAULT_FN_SENTINEL_NAME`
        // is the synthetic placeholder generated by `default_for_type_expr`
        // for declare-only function fields. Its body intentionally carries
        // no AST (it must never enter the trampoline loop). When called,
        // it materialises and returns the **return type's default value**
        // per Lock-D verdict (E30 Phase 0, 2026-04-28):
        //   - primitive return → primitive default
        //   - `:Unit` → no-op (Value::Unit)
        //   - `:Async[T]` → resolved Async carrying T's default
        //   - registered class-like → default pack with cycle guard
        //   - opaque alias → not generatable (Phase 5 reject path)
        if func.name == Self::DEFAULT_FN_SENTINEL_NAME {
            self.call_depth -= 1;
            return self.invoke_default_fn(func).map(Signal::Value);
        }

        // Save the previous active function name
        let prev_active = self.active_function.take();

        // Set active function for tail call detection
        if func.name != "<lambda>" {
            self.active_function = Some(func.name.clone());
        }

        // Trampoline loop for tail call optimization
        // current_func tracks which function to execute (may change for mutual recursion)
        let mut current_func = func.clone();
        let mut current_args = arg_values;
        // C20B-015 / ROOT-18: overlay defining-module typedef scope.
        //
        // Invariant: on *every* exit path from this trampoline we restore
        // `self.type_defs` / `self.enum_defs` to the snapshot captured here
        // (`saved_td_root` / `saved_ed_root`), regardless of which callee
        // within the tail-call chain actually required an overlay.
        //
        // Before the fix, the initial callee's `td_changed` flag gated the
        // restore. When the initial callee was a *local* function
        // (`td_changed == false`) and a mutual tail-call retargeted to an
        // *imported* function (whose overlay push succeeded), the exit
        // path skipped the restore and the imported function's private
        // TypeDefs leaked into the caller's scope permanently. Pinned by
        // `c20b_015_interpreter_mutual_tail_call_does_not_leak_overlay`.
        //
        // The unconditional clone-and-restore is the only correct contract
        // here: any subset of iterations may or may not push an overlay,
        // but the caller's pre-call scope is always `saved_*_root` and
        // must be the state we return to. Cost: `HashMap::clone` of the
        // root registries, paid per top-level function call (not per
        // trampoline iteration).
        let saved_td_root = self.type_defs.clone();
        let saved_ed_root = self.enum_defs.clone();
        let _ = self.push_func_module_scope(&current_func);
        loop {
            // Create closure scope (separate from local scope so user variables
            // can shadow captured names without "already defined" errors)
            self.env.push_scope();
            for (name, val) in current_func.closure.iter() {
                self.env.define_force(name, val.clone());
            }

            // Create local scope for parameters and function body
            self.env.push_scope();
            // Bind parameters using effective defaults.
            //
            // C25B-021 / Phase 5-F2-2 Stage B: after binding, release our
            // hold on `current_args` so the env becomes the unique Arc
            // owner for any Value::List passed as an argument. Without
            // this, the Append / Prepend unique-ownership fast path
            // always sees rc>=2 and falls back to a full Vec clone,
            // making the tail-recursive Append loop O(N²).
            //
            // `bind_params_with_effective_defaults` still clones each
            // slot internally, but once it returns the slots themselves
            // are no longer needed for this trampoline iteration — we'll
            // receive a fresh TailCall(new_args) if tail-recursion loops.
            let bind_outcome =
                self.bind_params_with_effective_defaults(&current_func, &current_args);
            current_args.clear();
            match bind_outcome {
                Ok(Some(signal)) => {
                    self.env.pop_scope(); // pop local scope
                    self.env.pop_scope(); // pop closure scope
                    self.type_defs = saved_td_root.clone();
                    self.enum_defs = saved_ed_root.clone();
                    self.active_function = prev_active;
                    self.call_depth -= 1;
                    return Ok(signal);
                }
                Ok(None) => {}
                Err(err) => {
                    self.env.pop_scope(); // pop local scope
                    self.env.pop_scope(); // pop closure scope
                    self.type_defs = saved_td_root.clone();
                    self.enum_defs = saved_ed_root.clone();
                    self.active_function = prev_active;
                    self.call_depth -= 1;
                    return Err(err);
                }
            }

            // Execute body (with error ceiling support).
            //
            // C20B-015 / ROOT-18 (3rd reopen): body evaluation must NOT use
            // `?`. A `RuntimeError` from the body would skip the pops and
            // root-scope restore, leaving `self.type_defs` / `self.enum_defs`
            // holding the defining-module overlay, the closure + local scopes
            // pushed, and `self.active_function` / `self.call_depth` bumped.
            // The REPL reuses this interpreter across inputs, so the leak
            // would make a subsequent top-level `JSON[raw, Schema]()`
            // silently resolve against an imported module's private schemas
            // (or — for enum / TypeDef aliasing — quietly hit wrong data).
            // Bind the result, run cleanup on every failure path, then
            // propagate. On the non-error paths we still need the ability
            // to continue the trampoline (self-TCO) or retarget (mutual
            // TCO) without tearing down the root-scope snapshot, which is
            // handled inside the `Ok(result)` match below.
            let body_result = self.eval_statements(&current_func.body);
            let result = match body_result {
                Ok(signal) => signal,
                Err(err) => {
                    self.env.pop_scope(); // pop local scope
                    self.env.pop_scope(); // pop closure scope
                    self.type_defs = saved_td_root.clone();
                    self.enum_defs = saved_ed_root.clone();
                    self.active_function = prev_active;
                    self.call_depth -= 1;
                    return Err(err);
                }
            };

            self.env.pop_scope(); // pop local scope
            self.env.pop_scope(); // pop closure scope

            match result {
                Signal::TailCall(new_args) => {
                    // Check if this is a mutual tail call (to a different function)
                    if let Some(target_name) = self.mutual_tail_call_target.take() {
                        // Mutual tail call: switch to the target function.
                        // First check the current env scope (handles global/module-level functions).
                        // If not found, check the closure of the current function (handles
                        // imported functions captured in the closure scope, which was already popped).
                        let target_val = self
                            .env
                            .get(&target_name)
                            .cloned()
                            .or_else(|| current_func.closure.get(&target_name).cloned());
                        let target_func_opt = target_val.and_then(|v| match v {
                            Value::Function(f) => Some(f),
                            _ => None,
                        });
                        if let Some(target_func) = target_func_opt {
                            current_func = target_func.clone();
                            self.active_function = Some(target_name);
                            current_args = new_args;
                            // C20B-015 / ROOT-18: target function may be from
                            // a different module. Reset to the caller's
                            // saved root scope, then re-overlay with the new
                            // function's defining-module scope. This keeps
                            // overlays scoped to exactly the iterations that
                            // need them and never leaks a previous iteration's
                            // overlay into a later iteration's scope.
                            self.type_defs = saved_td_root.clone();
                            self.enum_defs = saved_ed_root.clone();
                            let _ = self.push_func_module_scope(&current_func);
                            continue;
                        } else {
                            // Target function not found in any scope — fall back to
                            // normal evaluation instead of erroring. This handles cases
                            // where a non-recursive function call in tail position was
                            // speculatively treated as a mutual tail call.
                            self.active_function = prev_active.clone();
                            self.type_defs = saved_td_root.clone();
                            self.enum_defs = saved_ed_root.clone();
                            // Re-evaluate the original function body without tail call optimization.
                            // We need to re-execute the function with the original args but
                            // as a normal call. Instead, we reconstruct the call.
                            // The simplest safe fallback: look up the function and call it directly.
                            // Since the target wasn't found, just error with a clear message.
                            self.call_depth -= 1;
                            return Err(RuntimeError {
                                message: format!(
                                    "Mutual tail call target '{}' not found",
                                    target_name
                                ),
                            });
                        }
                    }
                    // Self tail call: loop with new arguments
                    current_args = new_args;
                    continue;
                }
                other => {
                    // Normal result: restore and return.
                    self.type_defs = saved_td_root.clone();
                    self.enum_defs = saved_ed_root.clone();
                    self.active_function = prev_active;
                    self.call_depth -= 1;
                    return Ok(other);
                }
            }
        }
    }

    /// Evaluate template string interpolation.
    fn eval_template_string(&mut self, template: &str) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = template.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '$' && chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut expr_str = String::new();
                let mut depth = 1;
                for c in chars.by_ref() {
                    if c == '{' {
                        depth += 1;
                    } else if c == '}' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    expr_str.push(c);
                }
                // Parse and evaluate the interpolated expression
                let (program, errors) = crate::parser::parse(&expr_str);
                if errors.is_empty() && !program.statements.is_empty() {
                    if let Statement::Expr(expr) = &program.statements[0]
                        && let Signal::Value(v) = self.eval_expr(expr)?
                    {
                        result.push_str(&v.to_display_string())
                    }
                } else {
                    result.push_str(&expr_str);
                }
            } else {
                result.push(ch);
            }
        }

        Ok(result)
    }

    /// E30B-007 / Lock-G: evaluate the explicit addon-binding form
    /// `RustAddon["fn"](arity <= N)`.
    ///
    /// Contract:
    /// - `type_args` must contain exactly one entry, a `StringLit` with
    ///   the addon function name. Anything else → `[E1412]`.
    /// - `fields` must contain exactly one entry, name `arity`, with an
    ///   `IntLit` value. Anything else → `[E1412]`.
    /// - Must execute inside an addon facade load context
    ///   (`loading_addon_facade_ctx == Some(_)`). Otherwise → `[E1412]`.
    /// - The function name must exist in the surrounding addon's
    ///   manifest `[functions]` table and the declared arity must match
    ///   the manifest arity. Otherwise → `[E1412]` drift error.
    /// - On success, returns `Value::str("__taida_addon_call::<pkg>::<fn>")`
    ///   so the binding is structurally identical to today's pre-injected
    ///   addon sentinel (the addon dispatch path in `MoldInst` /
    ///   `try_addon_func` uses the sentinel string verbatim).
    fn eval_rust_addon_binding(
        &mut self,
        type_args: &[crate::parser::Expr],
        fields: &[crate::parser::BuchiField],
        _span: &crate::lexer::Span,
    ) -> Result<Signal, RuntimeError> {
        use crate::parser::Expr;

        // Surface form: exactly one type-arg = string literal.
        if type_args.len() != 1 {
            return Err(RuntimeError {
                message: format!(
                    "[E1412] RustAddon[\"fn\"] expects exactly one string \
                     literal inside the brackets (got {})",
                    type_args.len()
                ),
            });
        }
        let fn_name = match &type_args[0] {
            Expr::StringLit(s, _) => s.clone(),
            other => {
                return Err(RuntimeError {
                    message: format!(
                        "[E1412] RustAddon[\"fn\"] requires a string literal; \
                         got {:?}",
                        other
                    ),
                });
            }
        };

        // Surface form: exactly one field `arity <= IntLit`.
        if fields.len() != 1 {
            return Err(RuntimeError {
                message: format!(
                    "[E1412] RustAddon[\"{}\"](arity <= N) expects exactly \
                     one `arity` field (got {})",
                    fn_name,
                    fields.len()
                ),
            });
        }
        if fields[0].name != "arity" {
            return Err(RuntimeError {
                message: format!(
                    "[E1412] RustAddon[\"{}\"] expects field name `arity`, got `{}`",
                    fn_name, fields[0].name
                ),
            });
        }
        let arity_decl: u32 = match &fields[0].value {
            Expr::IntLit(n, _) if *n >= 0 => *n as u32,
            other => {
                return Err(RuntimeError {
                    message: format!(
                        "[E1412] RustAddon[\"{}\"](arity <= N) requires a \
                         non-negative integer literal; got {:?}",
                        fn_name, other
                    ),
                });
            }
        };

        // Must execute inside an addon facade load context.
        let (pkg_id, manifest_arities) = match &self.loading_addon_facade_ctx {
            Some(ctx) => ctx.clone(),
            None => {
                return Err(RuntimeError {
                    message: format!(
                        "[E1412] RustAddon[\"{}\"] binding may only appear \
                         inside an addon facade (`<pkg>/taida/<stem>.td`). \
                         This is the explicit-binding form locked by E30 \
                         Lock-G; user-side code cannot construct addon \
                         sentinels directly.",
                        fn_name
                    ),
                });
            }
        };

        // Function must exist + arity must match manifest entry.
        let manifest_arity = match manifest_arities.get(&fn_name) {
            Some(a) => *a,
            None => {
                return Err(RuntimeError {
                    message: format!(
                        "[E1412] RustAddon[\"{}\"]: function not declared in \
                         addon manifest `[functions]` for package '{}'.",
                        fn_name, pkg_id
                    ),
                });
            }
        };
        if arity_decl != manifest_arity {
            return Err(RuntimeError {
                message: format!(
                    "[E1412] RustAddon[\"{}\"](arity <= {}) drifts from \
                     addon manifest declared arity {} for package '{}'. \
                     Update the facade binding or the manifest entry.",
                    fn_name, arity_decl, manifest_arity, pkg_id
                ),
            });
        }

        // Success: same sentinel string the legacy pre-inject path produces.
        let sentinel = format!("__taida_addon_call::{}::{}", pkg_id, fn_name);
        Ok(Signal::Value(Value::str(sentinel)))
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function: parse and evaluate source code.
pub fn eval(source: &str) -> Result<Value, String> {
    let (program, parse_errors) = crate::parser::parse(source);
    if !parse_errors.is_empty() {
        return Err(parse_errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let mut interpreter = Interpreter::new();
    interpreter
        .eval_program(&program)
        .map_err(|e| e.to_string())
}

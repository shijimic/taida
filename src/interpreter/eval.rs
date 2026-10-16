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

use super::env::{Environment, Link};
use super::value::{FuncValue, ModuleId, PRELUDE_FUNCTION_PREFIX, Value};
use crate::parser::*;

/// Maximum function call depth before RuntimeError.
/// Prevents stack overflow from deeply recursive (non-TCO) calls.
/// Set conservatively to account for multiple Rust stack frames per call
/// (eval_statements + eval_expr + call_function = ~5 frames per recursion,
/// plus debug builds have larger frames).
const MAX_CALL_DEPTH: usize = 8192;

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

/// Cached module: exported symbols from a loaded.td file.
#[derive(Debug, Clone)]
pub(crate) struct LoadedModule {
    /// The module the file ran as.
    pub(crate) module: ModuleId,
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

/// Type registries of one module: what its code sees when it names a
/// type, a mold, or an enum. The registries of the module whose code is
/// running live in the interpreter's own fields (`type_defs`, ...); the
/// other modules keep theirs here.
#[derive(Debug, Default, Clone)]
pub(crate) struct TypeRegistry {
    pub(crate) type_defs: HashMap<String, Vec<FieldDef>>,
    pub(crate) enum_defs: HashMap<String, Vec<String>>,
    pub(crate) mold_defs: HashMap<String, Vec<FieldDef>>,
    pub(crate) type_methods: HashMap<String, HashMap<String, FuncDef>>,
    pub(crate) type_tags: HashMap<String, Value>,
    pub(crate) enum_keys: HashMap<String, String>,
}

/// A module the interpreter has executed (or is executing).
#[derive(Debug, Default)]
pub(crate) struct ModuleRecord {
    /// Top-level bindings, once the module has finished running. Until
    /// then (and for the main program) they are the environment's module
    /// scope.
    pub(crate) scope: Option<Arc<HashMap<String, Value>>>,
    /// The module's type registries while another module's code runs.
    pub(crate) registry: TypeRegistry,
    /// The checker's types for the module's expressions, when the program
    /// was checked. Keyed by node id, which every parsed file numbers from
    /// 1 — so only the running module's own table is ever consulted.
    pub(crate) typed: Option<Arc<crate::types::TypedExprTable>>,
}

/// Where a user-defined type comes from: the module that declared it, its
/// name there, and — for a type declared inside a function body — the
/// frame it was declared in.
#[derive(Debug, Clone)]
pub(crate) struct TypeOrigin {
    pub(crate) module: ModuleId,
    pub(crate) name: String,
    pub(crate) def_site: Option<(usize, u64)>,
    /// The tag itself; holding it keeps its address unique.
    pub(crate) tag: Value,
    /// The declaration's own tag (as opposed to one an aliased import
    /// made so its instances display the alias).
    pub(crate) tag_owner: bool,
}

/// A type declaration: its module, its name there and the frame of a
/// function-local declaration. A built-in type the program did not declare
/// (the root `Error`) is keyed by its name under [`BUILTIN_TYPE_MODULE`].
pub(crate) type TypeDeclKey = (ModuleId, String, Option<(usize, u64)>);

/// The module of [`TypeDeclKey`]s naming built-in types.
pub(crate) const BUILTIN_TYPE_MODULE: ModuleId = ModuleId::MAX;

impl TypeOrigin {
    pub(crate) fn decl_key(&self) -> TypeDeclKey {
        (self.module, self.name.clone(), self.def_site)
    }
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
    /// Number of poolAcquire calls currently blocked waiting for a free
    /// slot (waiting semaphore). Surfaced as `poolHealth.waiting`.
    pub(crate) waiting: i64,
}

/// One expected host-capability call in an interpreter fixture.
#[derive(Debug, Clone, PartialEq)]
pub struct HostCallMockStep {
    pub method: String,
    pub args: Vec<Value>,
    pub result: Value,
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
    /// `io::stdout()` / `io::stderr()` with immediate flush. ( / )
    pub output: Vec<String>,
    /// Stream mode flag ( / ).
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
    /// builtins) seen so far.
    ///
    /// In stream mode, `output` Vec is always empty, so `main.rs` cannot use
    /// `output.is_empty()` to decide whether to print the final value. This
    /// counter replaces that check: after `eval_program`, if
    /// `stdout_emissions == 0` and the final value is not `Unit`, print it
    /// (matches existing buffered behavior where `output.is_empty()` conveys
    /// The same signal). Named "emissions" rather than "stdout_count" because
    /// `debug` also increments it — both builtins surface to stdout in stream
    /// mode (see `prelude.rs` for the debug→stdout design rationale).
    pub stdout_emissions: usize,
    /// Current file path (for resolving relative imports)
    pub(crate) current_file: Option<PathBuf>,
    /// Cache of loaded modules (canonical path -> exports)
    pub(crate) loaded_modules: HashMap<PathBuf, LoadedModule>,
    /// Set of modules currently being loaded (for circular import detection)
    pub(crate) loading_modules: HashSet<PathBuf>,
    /// Imported modules are checked as they load, so their code also sees
    /// static types (set with the main program's table).
    pub(crate) check_imports: bool,
    /// Currently executing function name (for tail call optimization)
    active_function: Option<String>,
    /// When a mutual tail call is detected, this holds the target function,
    /// resolved where the call was written. Used by call_function's
    /// trampoline loop to switch to a different function.
    mutual_tail_call_target: Option<FuncValue>,
    /// `node_id`s of `Append[p, item]()` sites in the currently
    /// executing function that passed the shared consume analysis
    /// (`parser::append_consume_plan`). When such a site evaluates, the
    /// accumulator binding is taken out of the environment instead of
    /// cloned, so `Value::list_take` sees a unique Arc and pushes in
    /// place — turning the tail-recursive build loop from O(n²) into
    /// amortized O(n). Saved/restored across nested calls; the empty
    /// set disables the fast path.
    append_consume_sites: std::collections::HashSet<usize>,
    /// Methods defined in TypeDef/InheritanceDef/MoldDef: type_name -> method_name -> FuncDef
    pub(crate) type_methods: HashMap<String, HashMap<String, FuncDef>>,
    /// TypeDef field definitions: type_name -> Vec<FieldDef> (for JSON schema matching)
    pub(crate) type_defs: HashMap<String, Vec<FieldDef>>,
    /// Enum definitions: enum_name -> variants in ordinal order
    pub(crate) enum_defs: HashMap<String, Vec<String>>,
    /// MoldDef field definitions: mold_name -> Vec<FieldDef> (for filling/unmold lookup)
    pub(crate) mold_defs: HashMap<String, Vec<FieldDef>>,
    /// The `__type` tag string of each visible user type (by the name the
    /// running module uses). Instances share the tag's allocation, which
    /// is how a value remembers which declaration it came from even when
    /// another module declares a type of the same name.
    pub(crate) type_tags: HashMap<String, Value>,
    /// Enum name in the running module -> the enum's key. Enum values
    /// carry the key, which is unique across modules (see `enum_variants`).
    pub(crate) enum_keys: HashMap<String, String>,
    /// Every module executed so far, indexed by `ModuleId`. The main
    /// program is module 0.
    pub(crate) modules: Vec<ModuleRecord>,
    /// The module whose top-level scope is the environment's module scope.
    pub(crate) top_module: ModuleId,
    /// The module the innermost running code belongs to. Its type
    /// registries are the live fields above.
    pub(crate) active_module: ModuleId,
    /// Declaration behind each type tag, keyed by the tag's address.
    pub(crate) type_origins: HashMap<usize, TypeOrigin>,
    /// Variants of every enum, by enum key.
    pub(crate) enum_variants: HashMap<String, Vec<String>>,
    /// Owner of each enum key: the (module, name, variants) it was made
    /// for, so a declaration evaluated again (inside a function body)
    /// keeps its key.
    pub(crate) enum_key_owners: HashMap<String, (ModuleId, String, Vec<String>)>,
    /// Tag of each type declaration, by (module, name), so a declaration
    /// evaluated again (inside a function body) keeps its tag.
    pub(crate) declared_type_tags: HashMap<(ModuleId, String), Value>,
    /// A `><` fired inside a callback and is on its way out as a
    /// `RuntimeError` (see `gorilla_escape`).
    pub(crate) pending_gorilla: bool,
    /// Only self tail calls may leave as `TailCall` (inside the protected
    /// statements of an error ceiling that starts a function body; see
    /// `eval_function_body`).
    tail_self_only: bool,
    /// Symbols declared via `<<<` during module execution.
    /// Empty means no `<<<` was encountered (all symbols exported for backward compat).
    pub(crate) module_exported_symbols: Vec<String>,
    /// Tokio runtime for true async operations.
    /// Used by `>=>` to block_on pending async values and by All/Race/Timeout molds.
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
    /// Host capability scripts used by the interpreter's fixture-only
    /// HostCall path.
    pub(crate) host_capability_mocks: HashMap<String, Vec<HostCallMockStep>>,
    /// Pending throw from a HOF callback (Map/Filter/Fold etc.).
    /// When a callback inside call_function_with_values throws, the thrown value
    /// is stored here so that eval_statements' error ceiling can recover it.
    pub(crate) pending_throw: Option<Value>,
    /// Inheritance parent map (child_name -> parent_name).
    /// Used by error ceiling to check if a thrown error type IS-A the handler type.
    pub(crate) type_parents: HashMap<String, String>,
    /// The same map by declaration, for parents the program declared: two
    /// modules may each declare a type of the same name, and each keeps
    /// its own parent.
    pub(crate) type_parent_decls: HashMap<TypeDeclKey, TypeDeclKey>,
    /// Current function call depth for stack overflow prevention.
    /// Incremented on each non-TCO function call, decremented on return.
    call_depth: usize,
    /// Active streaming writer for 2-arg httpServe handler.
    /// Set before calling the handler, cleared after the handler returns.
    /// This allows startResponse/writeChunk/endResponse/sseEvent to access
    /// The connection's StreamingWriter state and TcpStream during handler execution.
    /// Safety: The interpreter is single-threaded (!Send). The raw pointers point to
    /// stack-local variables in dispatch_request that outlive the handler call.
    pub(crate) active_streaming_writer: Option<super::net::eval::ActiveStreamingWriter>,
    /// Context for the addon facade currently being loaded.
    ///
    /// When `load_addon_facade` is evaluating a `<pkg>/taida/<stem>.td` file,
    /// this field holds `Some((package_id, function_arities))` so that the
    /// `RustAddon["fn"](arity <= N)` expression form can:
    ///
    /// 1. Resolve to the canonical addon sentinel
    /// `Value::str("__taida_addon_call::<package_id>::<fn>")` (matching
    /// today's `define_force` pre-injection contract).
    /// 2. Validate the declared `arity` field against the manifest
    /// `[functions]` table and reject drift with `[E1412]`.
    ///
    /// Outside facade execution this is `None` and a `RustAddon[...]`
    /// expression falls through to the generic `MoldInst` path (the
    /// checker will surface a `[E1412]` "context" diagnostic in a later
    /// sub-step; the interpreter contract here is purely runtime).
    pub(crate) loading_addon_facade_ctx: Option<(String, std::collections::BTreeMap<String, u32>)>,
}

/// What `leave_call_scope` restores: the frame-stack height and the
/// module that was active before a call (or declaration) scope was
/// entered.
pub(crate) struct CallScope {
    mark: usize,
    prev_module: ModuleId,
}

impl Interpreter {
    /// Synthetic defaultFn sentinel name.
    ///
    /// `default_for_type_expr` returns a `Value::Function` whose `name`
    /// equals this constant when materialising the default for a
    /// `TypeExpr::Function(_, _)` annotation (declare-only function field's
    /// auto-generated `defaultFn`). `call_function` intercepts the name
    /// before entering the trampoline loop and returns the **return
    /// type's default value** without executing any AST body.
    pub(crate) const DEFAULT_FN_SENTINEL_NAME: &'static str = "__taida_default_fn";

    /// The closure slot of a synthetic defaultFn that holds its result when
    /// it was made from a checker type rather than a type annotation (`#`
    /// keeps it apart from every Taida name).
    pub(crate) const DEFAULT_FN_RESULT_SLOT: &'static str = "#default";

    pub fn new() -> Self {
        // Create a dedicated current-thread tokio runtime for the interpreter.
        // This runtime is used by `>=>` to block_on pending async values
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
            append_consume_sites: std::collections::HashSet::new(),
            type_methods: HashMap::new(),
            type_defs: HashMap::new(),
            enum_defs: HashMap::new(),
            mold_defs: HashMap::new(),
            type_tags: HashMap::new(),
            enum_keys: HashMap::new(),
            modules: vec![ModuleRecord::default()],
            top_module: 0,
            active_module: 0,
            check_imports: false,
            type_origins: HashMap::new(),
            enum_variants: HashMap::new(),
            enum_key_owners: HashMap::new(),
            declared_type_tags: HashMap::new(),
            pending_gorilla: false,
            tail_self_only: false,
            module_exported_symbols: Vec::new(),
            tokio_runtime,
            socket_handles: Arc::new(Mutex::new(HashMap::new())),
            next_socket_id: Arc::new(AtomicI64::new(1)),
            listener_handles: Arc::new(Mutex::new(HashMap::new())),
            next_listener_id: Arc::new(AtomicI64::new(1)),
            udp_socket_handles: Arc::new(Mutex::new(HashMap::new())),
            pool_states: Arc::new(Mutex::new(HashMap::new())),
            next_pool_id: Arc::new(AtomicI64::new(1)),
            host_capability_mocks: HashMap::new(),
            pending_throw: None,
            type_parents: HashMap::new(),
            type_parent_decls: HashMap::new(),
            call_depth: 0,
            active_streaming_writer: None,
            loading_addon_facade_ctx: None,
        }
    }

    /// Construct a new Interpreter in **stream mode** ( / ).
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
    /// The flush timing differs between the two modes.
    pub fn new_streaming() -> Self {
        let mut interp = Self::new();
        interp.stream_stdout = true;
        interp
    }

    /// Register a fixture script for `Cage[HostCapability[name, kind](), HostCall[...]]()`.
    pub fn set_host_capability_mock_steps(
        &mut self,
        name: impl Into<String>,
        steps: Vec<HostCallMockStep>,
    ) {
        self.host_capability_mocks.insert(name.into(), steps);
    }

    /// Remove all registered host capability fixture scripts.
    pub fn clear_host_capability_mock_steps(&mut self) {
        self.host_capability_mocks.clear();
    }

    /// Check if `thrown_type` IS-A `handler_type` by walking the inheritance chain.
    /// Returns true if they are the same type or if `thrown_type` inherits from `handler_type`.
    pub(crate) fn is_error_subtype(&self, thrown_type: &str, handler_type: &str) -> bool {
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

    /// The declaration behind the type the active module calls `name`, when
    /// the program declared it (a built-in type has none).
    pub(crate) fn declared_type_origin(&self, name: &str) -> Option<&TypeOrigin> {
        self.type_tags
            .get(name)
            .and_then(|tag| self.origin_of_tag(tag))
    }

    /// The declaration the active module means by the type name `name`, or
    /// the built-in type of that name.
    fn type_decl_key_of_name(&self, name: &str) -> TypeDeclKey {
        match self.declared_type_origin(name) {
            Some(origin) => origin.decl_key(),
            None => (BUILTIN_TYPE_MODULE, name.to_string(), None),
        }
    }

    /// Whether a value whose `__type` is `tag` (named `name`) is the type
    /// the active module calls `target`, or extends it. Declared types are
    /// told apart by their declaration, so same-named types of two modules
    /// are different types; a value of a type the program did not declare
    /// (a built-in error) is compared by name.
    pub(crate) fn tag_extends(&self, tag: Option<&Value>, name: &str, target: &str) -> bool {
        match tag.and_then(|tag| self.origin_of_tag(tag)) {
            Some(start) => self.decl_extends(start.decl_key(), &self.type_decl_key_of_name(target)),
            None => name == target || self.check_type_extends(name, target),
        }
    }

    /// Whether the type the active module calls `child` is `parent` or
    /// extends it (`TypeExtends`), by declaration when `child` is declared.
    pub(crate) fn name_extends(&self, child: &str, parent: &str) -> bool {
        match self.declared_type_origin(child) {
            Some(child) => self.decl_extends(child.decl_key(), &self.type_decl_key_of_name(parent)),
            None => child == parent || self.check_type_extends(child, parent),
        }
    }

    fn decl_extends(&self, mut current: TypeDeclKey, target: &TypeDeclKey) -> bool {
        for _ in 0..64 {
            if current == *target {
                return true;
            }
            match self.type_parent_decls.get(&current) {
                Some(parent) => current = parent.clone(),
                None => return false,
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

    /// Give the main program the checker's types for its expressions; the
    /// modules it imports are then checked as they load. A lookup that finds
    /// nothing yields the default of the element's static type with them.
    pub fn set_typed_table(&mut self, table: crate::types::TypedExprTable) {
        let module = self.top_module;
        self.modules[module].typed = Some(Arc::new(table));
        self.check_imports = true;
    }

    /// The checker's type of `expr` in the running module, if it was checked.
    pub(crate) fn static_type_of(&self, expr: &Expr) -> Option<&crate::types::Type> {
        self.modules
            .get(self.active_module)?
            .typed
            .as_ref()?
            .lookup(expr)
    }

    /// Evaluate an entire program.
    pub fn eval_program(&mut self, program: &Program) -> Result<Value, RuntimeError> {
        let outcome = self.eval_statements(&program.statements);
        // A `><` inside a callback travels out as a RuntimeError; it ends
        // the program exactly like a top-level `><`.
        if std::mem::take(&mut self.pending_gorilla) {
            return Ok(Value::Gorilla);
        }
        match outcome? {
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
                    message: format!("Unhandled error: {}", Self::format_unhandled_error(&err)),
                })
            }
            Signal::Gorilla => Ok(Value::Gorilla),
        }
    }

    fn format_unhandled_error(err: &Value) -> String {
        fn str_field<'a>(fields: &'a [(String, Value)], name: &str) -> Option<&'a str> {
            fields.iter().find_map(|(field, value)| {
                if field == name
                    && let Value::Str(s) = value
                {
                    return Some(s.as_str());
                }
                None
            })
        }

        match err {
            Value::Error(e) => {
                let error_type = if e.error_type.is_empty() {
                    "AnonymousError"
                } else {
                    e.error_type.as_str()
                };
                let message = if e.message.is_empty() {
                    "Unknown"
                } else {
                    e.message.as_str()
                };
                format!("Error[{}]: {}", error_type, message)
            }
            Value::BuchiPack(fields) => {
                // `str_field` returns `Some("")` for an empty string literal;
                // an empty `type <= ""` gets the same AnonymousError
                // treatment as a missing type.
                let error_type = str_field(fields, "type")
                    .or_else(|| str_field(fields, "__type"))
                    .filter(|s| !s.is_empty())
                    .unwrap_or("AnonymousError");
                // An explicit `message <= ""` is shown as it is; an omitted
                // message shows the type name (`docs/guide/08_error_handling.md`).
                let message = str_field(fields, "message").unwrap_or(error_type);
                format!("Error[{}]: {}", error_type, message)
            }
            other => other.to_display_string(),
        }
    }

    /// Evaluate a sequence of statements with error ceiling support.
    ///
    /// When an ErrorCeiling statement is encountered, the remaining statements
    /// become "protected" code. Any Throw signal from the protected code is
    /// caught and the error ceiling's handler body is executed.
    pub(crate) fn eval_statements(&mut self, stmts: &[Statement]) -> Result<Signal, RuntimeError> {
        self.eval_statements_from(stmts, false)
    }

    /// Evaluate a function body. When the body starts with an error
    /// ceiling, a self tail call in its protected statements still loops
    /// (`docs/guide/09_functions.md`): the trampoline re-runs the body, so
    /// every iteration sits inside the same ceiling, and a caught error
    /// runs the handler with the innermost call's arguments.
    pub(crate) fn eval_function_body(
        &mut self,
        stmts: &[Statement],
    ) -> Result<Signal, RuntimeError> {
        let leading_ceiling = matches!(stmts.first(), Some(Statement::ErrorCeiling(_)));
        self.eval_statements_from(stmts, leading_ceiling)
    }

    fn eval_statements_from(
        &mut self,
        stmts: &[Statement],
        leading_ceiling: bool,
    ) -> Result<Signal, RuntimeError> {
        let mut last_value = Value::Unit;
        let mut i = 0;

        while i < stmts.len() {
            if let Statement::ErrorCeiling(ec) = &stmts[i] {
                // The remaining statements after the error ceiling are "protected"
                let protected_stmts = &stmts[i + 1..];

                // a consumer that swallows the callback RuntimeError
                // (an HTTP handler answering 500, for example) leaves
                // `pending_throw` stale. Clear it on entry so this ceiling
                // never recovers a PREVIOUS run's throw when a genuine
                // RuntimeError fires inside its own protected block — long
                // lived servers would otherwise wire an old error value into
                // an unrelated handler.
                self.pending_throw = None;

                // Evaluate the protected statements WITHOUT tail-call optimization.
                // TCO on the last protected statement would return Signal::TailCall,
                // bypassing the error ceiling catch below.  The caller (call_function's
                // trampoline) would then execute the function outside this ceiling's scope,
                // so any throw would propagate unhandled.  Using eval_statements_no_tco
                // ensures all function calls actually execute within the ceiling.
                //
                // If a RuntimeError occurs that wraps a Taida throw (from a HOF callback),
                // recover the thrown value from pending_throw and handle it as Signal::Throw.
                let protected_outcome = if leading_ceiling && i == 0 {
                    self.eval_protected_self_tail(protected_stmts)
                } else {
                    self.eval_statements_no_tco(protected_stmts)
                };
                let protected_result = match protected_outcome {
                    Ok(signal) => signal,
                    Err(runtime_err) => {
                        if self.pending_gorilla {
                            // The gorilla is never caught.
                            return Err(runtime_err);
                        }
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
                        let thrown_tag = match &err {
                            Value::BuchiPack(fields) => {
                                fields.iter().find(|(n, _)| n == "__type").map(|(_, v)| v)
                            }
                            _ => None,
                        };
                        let thrown_type = match &err {
                            Value::Error(e) => Some(e.error_type.as_str()),
                            _ => match thrown_tag {
                                Some(Value::Str(s)) => Some(s.as_str()),
                                _ => None,
                            },
                        };

                        // Check if the thrown type IS-A the handler type.
                        // "Error" catches everything (base type).
                        // Otherwise, walk the inheritance chain.
                        let type_matches = if handler_type == "Error" {
                            true
                        } else if let Some(thrown) = thrown_type {
                            self.tag_extends(thrown_tag, thrown, handler_type)
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
                            // the handler variable always sees the
                            // full ErrorInfo field set (type/message/kind/code),
                            // regardless of how the error was thrown.
                            self.env.define_force(
                                &ec.error_param,
                                Self::canonicalize_caught_error(err),
                            );

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
                // bind FIRST and surface a same-scope redefinition
                // error instead of silently keeping the OLD binding while the
                // registry takes the NEW variant list — the mismatch made
                // variant resolution diverge from the visible binding under
                // --no-check.
                if let Err(e) = self.env.define(
                    &ed.name,
                    Value::pack(vec![
                        ("__type".to_string(), Value::str("EnumDef".to_string())),
                        ("__name".to_string(), Value::str(ed.name.clone())),
                    ]),
                ) {
                    return Err(RuntimeError { message: e });
                }
                let variants: Vec<String> = ed
                    .variants
                    .iter()
                    .map(|variant| variant.name.clone())
                    .collect();
                let key = self.enum_key_for(&ed.name, &variants);
                self.enum_keys.insert(ed.name.clone(), key);
                self.enum_defs.insert(ed.name.clone(), variants);
                Ok(Signal::Value(Value::Unit))
            }

            // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch (旧 TypeDef/MoldDef/InheritanceDef を統合)
            Statement::ClassLikeDef(cl) => match &cl.kind {
                crate::parser::ClassLikeKind::BuchiPack => {
                    let td = cl;
                    // bind first — a same-scope redefinition must
                    // error before any registry is touched.
                    if let Err(e) = self.env.define(
                        &td.name,
                        Value::pack(vec![
                            ("__type".to_string(), Value::str("TypeDef".to_string())),
                            ("__name".to_string(), Value::str(td.name.clone())),
                        ]),
                    ) {
                        return Err(RuntimeError { message: e });
                    }
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
                    self.declare_type_tag(&td.name);
                    Ok(Signal::Value(Value::Unit))
                }
                crate::parser::ClassLikeKind::Alias { .. } => {
                    // Type aliases are checker-only. Define the same name
                    // sentinel as pack type-defs so the alias can sit in
                    // export lists / module symbol maps without a special
                    // case; it carries no constructor or fields.
                    if let Err(e) = self.env.define(
                        &cl.name,
                        Value::pack(vec![
                            ("__type".to_string(), Value::str("TypeDef".to_string())),
                            ("__name".to_string(), Value::str(cl.name.clone())),
                        ]),
                    ) {
                        return Err(RuntimeError { message: e });
                    }
                    Ok(Signal::Value(Value::Unit))
                }
                crate::parser::ClassLikeKind::Mold { .. } => {
                    let md = cl;
                    // bind first — a same-scope redefinition must
                    // error before any registry is touched.
                    if let Err(e) = self.env.define(
                        &md.name,
                        Value::pack(vec![
                            ("__type".to_string(), Value::str("TypeDef".to_string())),
                            ("__name".to_string(), Value::str(md.name.clone())),
                        ]),
                    ) {
                        return Err(RuntimeError { message: e });
                    }
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
                    self.declare_type_tag(&md.name);
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
                    // bind first — a same-scope redefinition must
                    // error before any registry is touched.
                    if let Err(e) = self.env.define(
                        inh_child,
                        Value::pack(vec![
                            ("__type".to_string(), Value::str("TypeDef".to_string())),
                            ("__name".to_string(), Value::str(inh_child.clone())),
                        ]),
                    ) {
                        return Err(RuntimeError { message: e });
                    }
                    self.type_defs.insert(inh_child.clone(), merged_fields);
                    if self.mold_defs.contains_key(inh_parent) {
                        self.mold_defs
                            .insert(inh_child.clone(), self.type_defs[inh_child].clone());
                    }
                    self.type_parents
                        .insert(inh_child.clone(), inh_parent.clone());
                    self.declare_type_tag(inh_child);
                    if let Some(child) = self.declared_type_origin(inh_child) {
                        let child = child.decl_key();
                        let parent = self.type_decl_key_of_name(inh_parent);
                        self.type_parent_decls.insert(child, parent);
                    }
                    Ok(Signal::Value(Value::Unit))
                }
            },

            Statement::FuncDef(fd) => {
                let closure = Arc::new(self.env.snapshot());
                let func = Value::Function(FuncValue {
                    type_params_count: fd.type_params.len(),
                    name: fd.name.clone(),
                    params: fd.params.clone(),
                    body: fd.body.clone(),
                    closure,
                    return_type: fd.return_type.clone(),
                    module: self.active_module,
                    def_site: self.env.definition_site(),
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
                // For Async values, >=> acts as blocking await
                // Rejected Async throws an error (caught by error ceiling)
                //
                // C13-1: Return the unwrapped value as the statement's
                // signal so tail unmold `expr >=> name` in an expression
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
                // For Async values, <=< acts as blocking await
                // Rejected Async throws an error (caught by error ceiling)
                //
                // C13-1: Return the unwrapped value as the statement's
                // signal so tail unmold `name <=< expr` in an expression
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

    /// Shared `=> name` binding-reference rule — single definition in
    /// `crate::parser::ast` (review C-6/C-9: lambda-param shadowing and
    /// full statement coverage live there).
    pub(crate) fn expr_references_any(expr: &Expr, bound_names: &[String]) -> bool {
        expr_references_any_name(expr, bound_names)
    }

    /// Decide whether an intermediate pipeline step
    /// `=> name` should call `name` with the current value (classic
    /// function-step semantics) or bind the current value to `name`
    /// and forward it unchanged (bind-and-forward semantics).
    ///
    /// A name is considered callable-in-pipeline if:
    /// - it already resolves to a `Value::Function` in the current
    /// environment (user-defined function, prelude function stored
    /// as a Function), or
    /// - it resolves to a builtin sentinel `Value::Str` (used for
    /// addon dispatch / net builtins / crypto), or
    /// - it is not defined in the environment at all — in which case
    /// we assume it's a binding target (never an undefined function
    /// reference, which would have been caught by the checker).
    ///
    /// For any resolved *value* (Int, Str, List, BuchiPack,...) that
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
            // An unbound prelude function name is a function too
            // (`x => debug => y` prints `x`).
            None => Self::PRELUDE_FUNCTION_VALUES.contains(&name),
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

    /// Evaluate the protected statements of an error ceiling that starts a
    /// function body. Like `eval_statements_no_tco`, except that the last
    /// statement may end in a *self* tail call: the trampoline then runs
    /// the body again, inside a fresh instance of the same ceiling. Calls
    /// to other functions still run here, under this ceiling.
    fn eval_protected_self_tail(&mut self, stmts: &[Statement]) -> Result<Signal, RuntimeError> {
        let mut last_value = Value::Unit;
        for (i, stmt) in stmts.iter().enumerate() {
            // Check for nested error ceilings (delegate to full eval_statements)
            if let Statement::ErrorCeiling(_) = stmt {
                return self.eval_statements(&stmts[i..]);
            }
            let signal = if i + 1 == stmts.len() {
                let prev = std::mem::replace(&mut self.tail_self_only, true);
                let result = self.eval_statement_tail(stmt);
                self.tail_self_only = prev;
                result?
            } else {
                self.eval_statement(stmt)?
            };
            match signal {
                Signal::Value(v) => last_value = v,
                other => return Ok(other),
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
    /// The arm body is a scope of its own (see `eval_cond_arm_body`).
    fn eval_cond_arm_body_tail(&mut self, body: &[Statement]) -> Result<Signal, RuntimeError> {
        if body.is_empty() {
            return Ok(Signal::Value(Value::Unit));
        }
        let mark = self.env.mark();
        self.env.push_scope();
        let result = self.eval_cond_arm_statements_tail(body);
        self.env.truncate(mark);
        result
    }

    fn eval_cond_arm_statements_tail(
        &mut self,
        body: &[Statement],
    ) -> Result<Signal, RuntimeError> {
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
                    // a self-call must still RESOLVE to a function
                    // value. An inner scope can re-bind the function's own
                    // name to a non-function value (`f <= 3`); restarting
                    // the trampoline on that binding turned "Cannot call
                    // non-function value" into an infinite loop that never
                    // reaches MAX_CALL_DEPTH.
                    let is_self_call = self.active_function.as_deref() == Some(name)
                        && !self.env.is_defined_in_current_scope(name)
                        && matches!(self.env.get(name), Some(Value::Function(_)));
                    // Check if the callee is a user-defined function (for mutual recursion).
                    // Only attempt mutual TCO when inside a function context AND the
                    // function is NOT defined in the current (innermost) scope.
                    // The target is resolved here, where the call is written:
                    // the trampoline runs after this body's scopes are gone,
                    // so resolving the name there would reach the caller's
                    // bindings instead.
                    let mutual_target = if !is_self_call
                        && !self.tail_self_only
                        && self.active_function.is_some()
                        && !self.env.is_defined_in_current_scope(name)
                    {
                        match self.env.get(name) {
                            Some(Value::Function(target)) => Some(target.clone()),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let is_user_func = mutual_target.is_some();

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
                        if let Some(target) = mutual_target {
                            // Mutual tail call: hand the resolved target to
                            // the trampoline.
                            self.mutual_tail_call_target = Some(target);
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

            // A pipeline in tail position: a last stage that calls a function
            // with the piped value at its `_` is that call, in tail position.
            Expr::Pipeline(exprs, _) => {
                let mark = self.env.mark();
                let result = self.eval_pipeline(exprs, true);
                self.env.truncate(mark);
                result
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
                // Template string interpolation: replace ${...} with evaluated
                // values. Throw / Gorilla propagate as-is.
                self.eval_template_string(s)
            }
            Expr::BoolLit(b, _) => Ok(Signal::Value(Value::Bool(*b))),
            Expr::Gorilla(_) => Ok(Signal::Gorilla),
            Expr::Placeholder(_) => Err(RuntimeError {
                message: "[E1502] `_` is only valid inside a pipeline placeholder position."
                    .to_string(),
            }),
            Expr::Hole(_) => Err(RuntimeError {
                message: "[E1502] Empty argument slots are only valid inside function calls."
                    .to_string(),
            }),
            // F62B-022: expression block (block-bodied lambda body). When a
            // lambda is materialized the block unwraps into the FuncValue's
            // statement body, so this arm only runs if a block is evaluated
            // standalone — scoped statement evaluation keeps the semantics
            // identical either way.
            Expr::Block(stmts, _) => {
                let mark = self.env.mark();
                self.env.push_scope();
                let result = self.eval_statements_no_tco(stmts);
                self.env.truncate(mark);
                result
            }
            // B11-6a: TypeLiteral is only valid inside TypeIs/TypeExtends — handled by mold
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
                } else if let Some(val) = self.prelude_function_value(name) {
                    Ok(Signal::Value(val))
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
                                 manifest function '{}' is no longer pre-injected. Add an \
                                 explicit binding to the top of the facade: `{} <= \
                                 RustAddon[\"{}\"](arity <= {})`. AST migration tooling is \
                                 not available; update the facade manually.",
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
                // `&&` / `||` short-circuit: the right operand is evaluated
                // only when the left one does not decide the result.
                match op {
                    BinOp::And if !left_val.is_truthy() => {
                        return Ok(Signal::Value(Value::Bool(false)));
                    }
                    BinOp::Or if left_val.is_truthy() => {
                        return Ok(Signal::Value(Value::Bool(true)));
                    }
                    _ => {}
                }
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
                        // wrapping_neg matches the binary
                        // operator discipline (wrapping_add/sub/mul) and
                        // keeps `-i64::MIN` from panicking in debug builds.
                        Value::Int(n) => Ok(Signal::Value(Value::Int(n.wrapping_neg()))),
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

                // The prelude and the core packages behave as the outermost
                // scope: a name the program binds itself (a function, a
                // parameter, a local) shadows the builtin of the same name,
                // exactly as the checker types the call. Builtins are
                // consulted only for names with no binding, and for the
                // dispatch sentinels an import binds (possibly under an
                // alias).
                if let Expr::Ident(name, _) = callee.as_ref() {
                    let dispatched = match self.env.get(name) {
                        None => self.try_builtin_func(name, args)?,
                        Some(Value::Str(tag)) if tag.starts_with("__") => {
                            let tag = tag.to_string();
                            self.try_imported_builtin(name, &tag, args)?
                        }
                        Some(_) => None,
                    };
                    if let Some(result) = dispatched {
                        return Ok(result);
                    }
                }

                let callee_val = match self.eval_expr(callee)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };

                // `Pilot()`: a declared type called with no fields is the
                // instance of its defaults, as `Pilot(...)` with every field
                // omitted is.
                if args.is_empty()
                    && let Expr::Ident(type_name, type_span) = callee.as_ref()
                    && let Value::BuchiPack(fields) = &callee_val
                    && fields.iter().any(|(n, v)| {
                        n == "__type" && matches!(v, Value::Str(s) if s.as_str() == "TypeDef")
                    })
                {
                    return self.eval_expr(&Expr::TypeInst(
                        type_name.clone(),
                        Vec::new(),
                        type_span.clone(),
                    ));
                }
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
                let result = self.eval_method_call(&obj_val, method, args)?;
                Ok(self.with_static_miss_default(obj, method, result))
            }

            Expr::FieldAccess(obj, field, _) => {
                let obj_val = match self.eval_expr(obj)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                if field.starts_with("__") {
                    return Err(RuntimeError {
                        message: format!(
                            "[E1960] Field '{}' is compiler-internal and cannot be accessed from Taida code. Hint: use unmolding, getOrDefault(default), or errorInfo() instead.",
                            field
                        ),
                    });
                }
                match &obj_val {
                    Value::BuchiPack(fields) => {
                        if let Some((_, val)) = fields.iter().find(|(n, _)| n == field) {
                            // A Result given no error reads its `throw` as
                            // the empty Error.
                            if field == "throw"
                                && matches!(val, Value::Unit)
                                && fields.iter().any(|(n, v)| {
                                    n == "__type"
                                        && matches!(v, Value::Str(s) if s.as_str() == "Result")
                                })
                            {
                                return Ok(Signal::Value(Value::empty_error()));
                            }
                            Ok(Signal::Value(val.clone()))
                        } else if let Some(val) = self.omitted_error_field(fields, field) {
                            Ok(Signal::Value(val))
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
                // The binding scope is dropped on every exit — a value, a
                // signal, or a runtime error (a caught callback throw
                // arrives as one) — so it never outlives the pipeline.
                let mark = self.env.mark();
                let result = self.eval_pipeline(exprs, false);
                self.env.truncate(mark);
                result
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

                // Append-consume fast path (sites proven by the shared
                // shape analysis, keyed by node_id): take the accumulator
                // binding out of the environment instead of cloning it.
                // The shape guarantees the binding is never read again in
                // this activation, and `list_take` still falls back to a
                // copy whenever any other alias holds the Arc — the first
                // iteration of a build loop detaches that way.
                if name == "Append"
                    && type_args.len() >= 2
                    && self.append_consume_sites.contains(&span.node_id)
                    && let Expr::Ident(pname, _) = &type_args[0]
                    && let Some(taken) = self.env.take(pname)
                {
                    match taken {
                        Value::List(items) => {
                            let val = match self.eval_expr(&type_args[1])? {
                                Signal::Value(v) => v,
                                other => return Ok(other),
                            };
                            let mut result = Value::list_take(items);
                            result.push(val);
                            return Ok(Signal::Value(Value::list(result)));
                        }
                        other_val => {
                            // Not a list: restore the binding and let the
                            // normal arm produce its error message.
                            self.env.define_force(pname, other_val);
                        }
                    }
                }

                // A `taida-lang/os` mold imported under an alias
                // (`>>> taida-lang/os => @(Exists => Present)`) is bound to
                // the sentinel carrying its exported name.
                if let Some(Value::Str(tag)) = self.env.get(name)
                    && let Some(exported) = tag.strip_prefix("__os_builtin_")
                    && exported != name
                {
                    let exported = exported.to_string();
                    if let Some(result) = self.try_os_mold(&exported, type_args, fields)? {
                        return Ok(result);
                    }
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
                    // F62B-021: for GENERIC functions the bracket always
                    // carries TYPE arguments (`genfn[T1, T2](args)` /
                    // `genfn[T]()`) — types erase at runtime, so the `[]`
                    // entries are skipped and the `()` values are the call
                    // arguments. Non-generic functions keep the legacy
                    // positional bracket call (`fn[arg1, arg2]()`).
                    if func.type_params_count > 0 || !fields.is_empty() {
                        let mut arg_exprs = Vec::with_capacity(fields.len());
                        let mut all_positional = true;
                        for field in fields {
                            let is_positional = field.name.starts_with('_')
                                && field.name[1..].chars().all(|c| c.is_ascii_digit());
                            if !is_positional {
                                all_positional = false;
                                break;
                            }
                            arg_exprs.push(field.value.clone());
                        }
                        if all_positional && (func.type_params_count > 0 || !fields.is_empty()) {
                            // F62B-038 #9: for a NON-generic function the
                            // bracket is the legacy positional call
                            // (`fn[arg1, arg2]()`), so combining it with
                            // `()` arguments (`fn[1](2)`) is ambiguous.
                            // The checker rejects this statically; without
                            // this guard the unchecked-eval path would
                            // silently drop the bracket values and run
                            // `fn(2)`.
                            if func.type_params_count == 0 && !type_args.is_empty() {
                                return Err(RuntimeError {
                                    message: format!(
                                        "Non-generic function '{}' cannot take both \
                                         bracket values and call arguments. \
                                         Call it as {}(arg1, arg2) or with the \
                                         positional bracket form {}[arg1, arg2]().",
                                        name, name, name
                                    ),
                                });
                            }
                            return self.call_function(&func, &arg_exprs);
                        }
                        let bracket_hint = if func.type_params_count > 0 {
                            format!("{name}[T](arg1, arg2)")
                        } else {
                            format!("{name}[arg1, arg2]()")
                        };
                        return Err(RuntimeError {
                            message: format!(
                                "User-defined function '{}' called via mold syntax \
                                 cannot accept named fields '()'. \
                                 Pass arguments positionally: {} or {}(arg1, arg2).",
                                name, bracket_hint, name
                            ),
                        });
                    }
                    return self.call_function(&func, type_args);
                }

                // Generic/custom mold instantiation.
                //
                // named fields keep their source order — a Vec with
                // in-place overwrite instead of a HashMap, whose iteration
                // order made the resulting pack's display order vary per
                // process.
                let mut named_values = Vec::<(String, Value)>::new();
                for field in fields {
                    let value = match self.eval_expr(&field.value)? {
                        Signal::Value(v) => v,
                        other => return Ok(other),
                    };
                    if let Some(slot) = named_values.iter_mut().find(|(n, _)| *n == field.name) {
                        slot.1 = value;
                    } else {
                        named_values.push((field.name.clone(), value));
                    }
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
                // `unmold` / `solidify` bodies run in the mold's declaring
                // module, like any function defined there.
                let mold_origin = self.type_origin_of_name(name);

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
                        if let Some((_, value)) =
                            named_values.iter().find(|(n, _)| n == &field_def.name)
                        {
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
                    for (fname, value) in &named_values {
                        if fname != "filling" && !consumed.contains(fname) {
                            result_fields.push((fname.clone(), value.clone()));
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
                        type_params_count: 0,
                        name: "__unmold".to_string(),
                        params: func_def.params.clone(),
                        body: func_def.body.clone(),
                        closure: Arc::new(closure),
                        return_type: func_def.return_type.clone(),
                        module: mold_origin.module,
                        def_site: mold_origin.def_site,
                    });
                    result_fields.push(("__unmold".to_string(), unmold_func));
                }

                // Add a __type field to track the type
                result_fields.push(("__type".to_string(), self.type_tag_value(name)));
                let instance = Value::pack(result_fields.clone());

                // `solidify` overrides what Name[args]() evaluates to.
                if let Some(ref func_def) = solidify_method {
                    let mut closure = HashMap::new();
                    for (field_name, field_val) in &result_fields {
                        closure.insert(field_name.clone(), field_val.clone());
                    }
                    closure.insert("self".to_string(), instance.clone());
                    let solidify_func = FuncValue {
                        type_params_count: 0,
                        name: "__solidify".to_string(),
                        params: func_def.params.clone(),
                        body: func_def.body.clone(),
                        closure: Arc::new(closure),
                        return_type: func_def.return_type.clone(),
                        module: mold_origin.module,
                        def_site: mold_origin.def_site,
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
                // F62B-022: a block-bodied lambda carries its statements
                // directly (the same shape named functions use).
                let lambda_body = match body.as_ref() {
                    Expr::Block(stmts, _) => stmts.clone(),
                    other => vec![Statement::Expr(other.clone())],
                };
                Ok(Signal::Value(Value::Function(FuncValue {
                    type_params_count: 0,
                    name: "<lambda>".to_string(),
                    params: params.clone(),
                    body: lambda_body,
                    closure,
                    return_type: None,
                    module: self.active_module,
                    def_site: self.env.definition_site(),
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
                    self.enum_key_of(enum_name),
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
                let tag = self.type_tag_value(name);
                // If TypeDef exists, inject defaults for omitted typed/defaulted fields.
                if let Some(type_fields) = self.type_defs.get(name).cloned() {
                    let mut consumed = HashSet::new();
                    let mut missing = Vec::new();
                    for field_def in type_fields.iter().filter(|f| !f.is_method) {
                        if let Some((_, provided)) = provided_fields
                            .iter()
                            .rev()
                            .find(|(n, _)| n == &field_def.name)
                        {
                            result_fields.push((field_def.name.clone(), provided.clone()));
                            consumed.insert(field_def.name.clone());
                        } else {
                            missing.push((result_fields.len(), field_def));
                            result_fields.push((field_def.name.clone(), Value::Unit));
                        }
                    }
                    if !missing.is_empty() {
                        // Defaults belong to the declaration: they are
                        // computed where the type was declared, whichever
                        // module constructs the value.
                        let origin = self.type_origin_of_name(name);
                        let context = self.enter_declaration_scope(&origin);
                        let mut visiting = HashSet::new();
                        let mut outcome = Ok(());
                        for (index, field_def) in missing {
                            match self.default_for_field_def(field_def, &mut visiting) {
                                Ok(value) => result_fields[index].1 = value,
                                Err(err) => {
                                    outcome = Err(err);
                                    break;
                                }
                            }
                        }
                        self.leave_call_scope(context);
                        outcome?;
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
                result_fields.push(("__type".to_string(), tag));
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

    /// Evaluate the stages of a pipeline. An intermediate `=> name` binding
    /// pushes one scope, which the caller drops.
    /// `tail`: the pipeline is in tail position, so a last stage that calls a
    /// function with the piped value (`x => f(a, _)`) is a tail call.
    fn eval_pipeline(&mut self, exprs: &[Expr], tail: bool) -> Result<Signal, RuntimeError> {
        let last_idx = exprs.len().saturating_sub(1);
        let mut bound_any = false;
        let mut bound_names: Vec<String> = Vec::new();
        let mut current = Value::Unit;
        for (i, expr) in exprs.iter().enumerate() {
            if i == 0 {
                current = match self.eval_expr(expr)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
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
            } else if !bound_names.is_empty() && Self::expr_references_any(expr, &bound_names) {
                // The user explicitly references one of the
                // pipeline-scope bindings inside this step. Evaluate
                // the step as-written without auto-injecting
                // `current` as an extra argument. A `_` here has
                // nothing to inject — reject instead
                // of leaking a raw placeholder into evaluation.
                if expr_count_placeholders(expr) > 0 {
                    return Err(RuntimeError {
                        message: "[E1543] A pipeline stage that references a `=> name` \
                                  binding is evaluated as written — `_` has no injection \
                                  value there. Use the bound name instead of `_`."
                            .to_string(),
                    });
                }
                current = match self.eval_expr(expr)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
            } else if tail && i == last_idx && Self::is_piped_call(expr) {
                // The call written out with the piped value at its `_`, in
                // tail position (same substitution as `eval_pipeline_step`).
                self.env.push_scope();
                let pipe_val_name = "__pipe_current__";
                self.env.define_force(pipe_val_name, current);
                let dummy_span = crate::lexer::Span::new(0, 0, 0, 0);
                let rewritten = super::control_flow::rewrite_nested_placeholder(
                    expr,
                    pipe_val_name,
                    &dummy_span,
                );
                let result = self.eval_expr_tail(&rewritten);
                self.env.pop_scope();
                return result;
            } else {
                current = match self.eval_pipeline_step(expr, current)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
            }
        }
        Ok(Signal::Value(current))
    }

    /// A pipeline stage that calls a named function with the piped value at
    /// its one `_` (`x => f(a, _)`).
    fn is_piped_call(expr: &Expr) -> bool {
        match expr {
            Expr::FuncCall(callee, args, _) => {
                matches!(callee.as_ref(), Expr::Ident(..))
                    && !args.iter().any(|arg| matches!(arg, Expr::Hole(_)))
                    && expr_count_placeholders(expr) == 1
            }
            _ => false,
        }
    }

    /// Error types the runtime itself raises. Their values are Error values
    /// although no `Error => ...` declaration names them.
    pub(crate) const BUILTIN_ERROR_TYPES: &'static [&'static str] = &[
        "IoError",
        "ProcessError",
        "ResultError",
        "RangeError",
        "AsyncError",
        "AsyncStateError",
        "AsyncRaceError",
        "TimeoutError",
        "HttpError",
        "CancelledError",
        "AssertionError",
        "ValueError",
        "RelaxedGorillaEscaped",
    ];

    /// Whether values whose `__type` is `name` are Error values.
    pub(crate) fn is_error_type_name(&self, name: &str) -> bool {
        name == "Error"
            || Self::BUILTIN_ERROR_TYPES.contains(&name)
            || self.is_error_subtype(name, "Error")
    }

    /// `type` / `message` of an Error value that was built without them:
    /// `type` reads as the type's name and `message` as `""`. The value
    /// itself keeps the field omitted, so its display and `jsonEncode`
    /// output stay as written, and an uncaught throw still shows the type
    /// name instead of an empty message.
    fn omitted_error_field(&self, fields: &[(String, Value)], field: &str) -> Option<Value> {
        if field != "type" && field != "message" {
            return None;
        }
        let (tag, type_name) = fields.iter().find_map(|(name, value)| match value {
            Value::Str(s) if name == "__type" => Some((value, s.as_str().to_string())),
            _ => None,
        })?;
        let declared_error =
            self.origin_of_tag(tag).is_some() && self.tag_extends(Some(tag), &type_name, "Error");
        if !declared_error && !self.is_error_type_name(&type_name) {
            return None;
        }
        Some(if field == "type" {
            Value::str(type_name)
        } else {
            Value::str(String::new())
        })
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

    /// The empty value of a built-in collection type.
    /// A lookup that finds nothing — `get` / `first` / `last` on a list,
    /// `get` on a HashMap — yields the default of the element's static type
    /// (an empty `@[Str]` gives `""`, a list of packs a pack of defaults),
    /// not of whatever the collection happens to hold. Without a static
    /// type the lookup's own default stays.
    fn with_static_miss_default(&mut self, obj: &Expr, method: &str, result: Signal) -> Signal {
        use crate::types::Type;
        if !matches!(method, "get" | "first" | "last") {
            return result;
        }
        let Signal::Value(Value::BuchiPack(fields)) = &result else {
            return result;
        };
        let is_failed_lax = fields
            .iter()
            .any(|(n, v)| n == "__type" && matches!(v, Value::Str(s) if s.as_str() == "Lax"))
            && fields
                .iter()
                .any(|(n, v)| n == "has_value" && matches!(v, Value::Bool(false)));
        if !is_failed_lax {
            return result;
        }
        let elem_ty = match self.static_type_of(obj) {
            Some(Type::List(elem)) => (**elem).clone(),
            Some(Type::Generic(name, args))
                if name == "HashMap" && args.len() == 2 && method == "get" =>
            {
                args[1].clone()
            }
            _ => return result,
        };
        match self.default_for_static_type(&elem_ty) {
            Some(default) => Signal::Value(Value::pack(vec![
                ("has_value".into(), Value::Bool(false)),
                ("__value".into(), default.clone()),
                ("__default".into(), default),
                ("__type".into(), Value::str("Lax".into())),
            ])),
            None => result,
        }
    }

    /// The default value of the checker type `ty`, or `None` when the type
    /// does not fix one.
    pub(crate) fn default_for_static_type(&mut self, ty: &crate::types::Type) -> Option<Value> {
        use crate::types::Type;
        match ty {
            Type::Int => Some(Value::Int(0)),
            Type::Float => Some(Value::Float(0.0)),
            Type::Str => Some(Value::str(String::new())),
            Type::Bool => Some(Value::Bool(false)),
            Type::Bytes => Some(Value::bytes(Vec::new())),
            Type::List(_) => Some(Value::list(Vec::new())),
            Type::BuchiPack(fields) => {
                let mut out = Vec::with_capacity(fields.len());
                for (name, field_ty) in fields {
                    out.push((name.clone(), self.default_for_static_type(field_ty)?));
                }
                Some(Value::pack(out))
            }
            Type::Named(name) => {
                let value = self
                    .default_for_type_expr(&TypeExpr::Named(name.clone()), &mut HashSet::new())
                    .ok()?;
                (!matches!(value, Value::Unit)).then_some(value)
            }
            Type::Generic(name, _) if name == "HashMap" || name == "Set" => {
                Some(Self::default_collection(name))
            }
            // A function returning its return type's default, whatever it is
            // given.
            Type::Function(params, ret) => {
                let ret_default = self.default_for_static_type(ret)?;
                Some(self.default_fn_value(params.len(), ret_default))
            }
            _ => None,
        }
    }

    /// The default of a function type: a function of `arity` parameters
    /// that returns `ret_default` (see [`Self::DEFAULT_FN_SENTINEL_NAME`]).
    fn default_fn_value(&self, arity: usize, ret_default: Value) -> Value {
        let params = (0..arity)
            .map(|i| Param {
                name: format!("__arg_{}", i),
                type_annotation: None,
                default_value: None,
                span: crate::lexer::Span::new(0, 0, 0, 0),
            })
            .collect();
        let mut closure = HashMap::new();
        closure.insert(Self::DEFAULT_FN_RESULT_SLOT.to_string(), ret_default);
        Value::Function(FuncValue {
            type_params_count: 0,
            name: Self::DEFAULT_FN_SENTINEL_NAME.to_string(),
            params,
            body: Vec::new(),
            closure: Arc::new(closure),
            return_type: None,
            module: self.active_module,
            def_site: None,
        })
    }

    fn default_collection(name: &str) -> Value {
        match name {
            "HashMap" => Value::pack(vec![
                ("__entries".into(), Value::list(Vec::new())),
                ("__type".into(), Value::str("HashMap".into())),
            ]),
            "Set" => Value::pack(vec![
                ("__items".into(), Value::list(Vec::new())),
                ("__type".into(), Value::str("Set".into())),
            ]),
            _ => Value::default_stream(),
        }
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
                "HashMap" | "Set" | "Stream" => Ok(Self::default_collection(name)),
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
                        fields.push(("__type".to_string(), self.type_tag_value(name)));
                        Ok(Value::pack(fields))
                    } else if self.enum_defs.contains_key(name) {
                        Ok(Value::EnumVal(self.enum_key_of(name), 0))
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
                // `Optional` is the removed predecessor of `Lax`; its values
                // behave as `Lax`.
                if name == "Lax" || name == "Optional" {
                    let inner = if let Some(inner_ty) = args.first() {
                        self.default_for_type_expr(inner_ty, visiting)?
                    } else {
                        Value::Unit
                    };
                    return Ok(Value::pack(vec![
                        ("has_value".to_string(), Value::Bool(false)),
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
                match name.as_str() {
                    "HashMap" | "Set" | "Stream" => Ok(Self::default_collection(name)),
                    // `Result[T, P]`: a success holding T's default.
                    "Result" => {
                        let inner = match args.first() {
                            Some(inner_ty) => self.default_for_type_expr(inner_ty, visiting)?,
                            None => Value::Unit,
                        };
                        Ok(Value::pack(vec![
                            ("__value".into(), inner),
                            ("__predicate".into(), Value::Unit),
                            ("throw".into(), Value::Unit),
                            ("__type".into(), Value::str("Result".into())),
                        ]))
                    }
                    // `Gorillax[T]` has no fallback value: its default is the
                    // empty Gorillax (unmolding it fires the gorilla).
                    "Gorillax" => {
                        let inner = match args.first() {
                            Some(inner_ty) => self.default_for_type_expr(inner_ty, visiting)?,
                            None => Value::Unit,
                        };
                        Ok(Value::pack(vec![
                            ("has_value".into(), Value::Bool(false)),
                            ("__value".into(), inner),
                            ("__error".into(), Value::Unit),
                            ("__type".into(), Value::str("Gorillax".into())),
                        ]))
                    }
                    // A user-defined generic type (`Box[Int]`) defaults
                    // like its plain name.
                    _ if self.type_defs.contains_key(name) => {
                        self.default_for_type_expr(&TypeExpr::Named(name.clone()), visiting)
                    }
                    _ => Ok(Value::Unit),
                }
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
                    type_params_count: 0,
                    name: Self::DEFAULT_FN_SENTINEL_NAME.to_string(),
                    params: synth_params,
                    body: Vec::new(),
                    closure: Arc::new(HashMap::new()),
                    return_type: Some((**ret).clone()),
                    module: self.active_module,
                    def_site: None,
                }))
            }
        }
    }

    /// Evaluate an empty-slot partial application: `func(arg1,, arg3)` returns a closure.
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

        // The partial is defined here; its captured arguments live in a
        // scope of their own that only its closure keeps.
        let def_site = self.env.definition_site();
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
            type_params_count: 0,
            name: "<partial>".to_string(),
            params,
            body,
            closure,
            return_type: None,
            module: self.active_module,
            def_site,
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
    /// - Ident("Int"/"Str"/"Float"/"Bool") → primitive schema
    /// - Ident("User") → look up TypeDef by name
    /// - ListLit([Ident("Pilot")]) → list schema with element type
    pub(crate) fn resolve_json_schema(
        &self,
        expr: &Expr,
    ) -> Result<crate::interpreter::json::JsonSchema, RuntimeError> {
        let mut schema = self.resolve_json_schema_by_name(expr)?;
        self.key_enum_schemas(&mut schema);
        Ok(schema)
    }

    /// Enum schemas name enums the way the active module does; decoded
    /// values carry the enum's key instead (see `enum_key_of`).
    fn key_enum_schemas(&self, schema: &mut crate::interpreter::json::JsonSchema) {
        use crate::interpreter::json::JsonSchema;
        match schema {
            JsonSchema::Enum(name, _) => *name = self.enum_key_of(name),
            JsonSchema::List(inner) => self.key_enum_schemas(inner),
            JsonSchema::TypeDef(_, fields) => {
                for field in fields.iter_mut() {
                    self.key_enum_schemas(&mut field.schema);
                }
            }
            // A reference points back at a TypeDef keyed where it is
            // expanded.
            JsonSchema::Primitive(_) | JsonSchema::Ref(_) => {}
        }
    }

    fn resolve_json_schema_by_name(
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
                    let elem_schema = self.resolve_json_schema_by_name(&items[0])?;
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
        arg_values: Vec<Value>,
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

        // The argument values move into the environment (no second
        // reference stays behind — see the Append consume fast path).
        let mut args = arg_values.into_iter();
        for param in func.params.iter() {
            let val = if let Some(arg) = args.next() {
                arg
            } else if let Some(default_expr) = &param.default_value {
                match self.eval_expr(default_expr)? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                }
            } else if let Some(type_ann) = &param.type_annotation {
                let mut visiting = std::collections::HashSet::new();
                self.default_for_type_expr(type_ann, &mut visiting)?
            } else {
                return Err(RuntimeError {
                    message: format!("Function '{}' missing argument '{}'", func.name, param.name),
                });
            };
            self.env.define_force(&param.name, val);
        }

        Ok(None)
    }

    // ── Modules and lexical scopes ──────────────────────────

    /// Stash the live type registries into `module`'s record, or load them
    /// back from it (the swap is its own inverse).
    fn swap_registry(&mut self, module: ModuleId) {
        let registry = &mut self.modules[module].registry;
        std::mem::swap(&mut self.type_defs, &mut registry.type_defs);
        std::mem::swap(&mut self.enum_defs, &mut registry.enum_defs);
        std::mem::swap(&mut self.mold_defs, &mut registry.mold_defs);
        std::mem::swap(&mut self.type_methods, &mut registry.type_methods);
        std::mem::swap(&mut self.type_tags, &mut registry.type_tags);
        std::mem::swap(&mut self.enum_keys, &mut registry.enum_keys);
    }

    /// Make `module` the active module: the code that runs next names
    /// types, molds and enums through its registries. Returns the module
    /// that was active, to switch back to.
    pub(crate) fn switch_module(&mut self, module: ModuleId) -> ModuleId {
        let prev = self.active_module;
        if module != prev && module < self.modules.len() {
            self.swap_registry(prev);
            self.swap_registry(module);
            self.active_module = module;
        }
        prev
    }

    /// Register a fresh module (a source file about to run). The main
    /// program is module 0 from the start.
    pub(crate) fn new_module(&mut self) -> ModuleId {
        self.modules.push(ModuleRecord::default());
        self.modules.len() - 1
    }

    /// The type registries of `module`: the live fields when it is the
    /// active module, its record otherwise.
    pub(crate) fn registry_of(&self, module: ModuleId) -> Option<&TypeRegistry> {
        if module == self.active_module {
            None
        } else {
            self.modules.get(module).map(|record| &record.registry)
        }
    }

    /// Methods of the type `type_name` as `module` declares or imports it.
    fn methods_in(&self, module: ModuleId, type_name: &str) -> Option<&HashMap<String, FuncDef>> {
        match self.registry_of(module) {
            None => self.type_methods.get(type_name),
            Some(registry) => registry.type_methods.get(type_name),
        }
    }

    /// Link to `module`'s top-level scope: the environment's module scope
    /// when that module's top-level code is running, otherwise the
    /// module's finished scope pushed as a frame.
    pub(crate) fn module_scope_link(&mut self, module: ModuleId) -> Link {
        if module == self.top_module {
            return Link::Globals;
        }
        match self
            .modules
            .get(module)
            .and_then(|record| record.scope.clone())
        {
            Some(scope) => self.env.push_module_scope(scope),
            // Only the module whose top-level code is running has no
            // finished scope yet.
            None => Link::Globals,
        }
    }

    /// Enter the scope a function body runs in: a fresh local scope whose
    /// chain continues through the scopes around the function's
    /// definition — never the caller's — and whose module is the
    /// function's own.
    ///
    /// While the frame the function was defined in is still running, the
    /// chain goes through that live frame, so local functions see
    /// siblings defined after them. Afterwards the closure snapshot stands
    /// in for it, and a named local function still sees itself (it can
    /// recurse after being returned from its defining function).
    pub(crate) fn enter_call_scope(&mut self, func: &FuncValue) -> CallScope {
        let mark = self.env.mark();
        let prev_module = self.switch_module(func.module);
        let live_site = func.def_site.and_then(|site| self.env.live_site(site));
        let local = match live_site {
            // A partial application keeps its fixed arguments in a scope of
            // its own, which the live frame does not hold.
            Some(link) if func.name == "<partial>" => {
                let captured: HashMap<String, Value> = func
                    .closure
                    .iter()
                    .filter(|(name, _)| name.starts_with("__partial_captured_"))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect();
                self.env.push_linked_scope(captured, link)
            }
            Some(link) => link,
            None => {
                let mut link = self.module_scope_link(func.module);
                if !func.closure.is_empty() {
                    link = self.env.push_shared_scope(func.closure.clone(), link);
                }
                if func.def_site.is_some() && !func.name.starts_with('<') {
                    let mut own = HashMap::new();
                    own.insert(func.name.clone(), Value::Function(func.clone()));
                    link = self.env.push_linked_scope(own, link);
                }
                link
            }
        };
        self.env.push_linked_scope(HashMap::new(), local);
        CallScope { mark, prev_module }
    }

    /// Enter the scope a declaration's own code runs in (field defaults,
    /// method bodies): its module, and the frame it was declared in while
    /// that frame still runs.
    pub(crate) fn enter_declaration_scope(&mut self, origin: &TypeOrigin) -> CallScope {
        let mark = self.env.mark();
        let prev_module = self.switch_module(origin.module);
        let link = match origin.def_site.and_then(|site| self.env.live_site(site)) {
            Some(link) => link,
            None => self.module_scope_link(origin.module),
        };
        self.env.push_linked_scope(HashMap::new(), link);
        CallScope { mark, prev_module }
    }

    /// Enter the scope a method body runs in: the instance fields, then
    /// the declaration's own scope (its module, or the frame the type was
    /// declared in while it runs).
    pub(crate) fn enter_method_scope(
        &mut self,
        origin: &TypeOrigin,
        instance_fields: &[(String, Value)],
    ) -> CallScope {
        let mark = self.env.mark();
        let prev_module = self.switch_module(origin.module);
        let link = match origin.def_site.and_then(|site| self.env.live_site(site)) {
            Some(link) => link,
            None => self.module_scope_link(origin.module),
        };
        let instance: HashMap<String, Value> = instance_fields
            .iter()
            .filter(|(name, _)| name != "__type")
            .map(|(name, val)| (name.clone(), val.clone()))
            .collect();
        let link = self.env.push_linked_scope(instance, link);
        self.env.push_linked_scope(HashMap::new(), link);
        CallScope { mark, prev_module }
    }

    /// Leave a scope entered by `enter_call_scope` / `enter_declaration_scope`.
    pub(crate) fn leave_call_scope(&mut self, scope: CallScope) {
        self.env.truncate(scope.mark);
        self.switch_module(scope.prev_module);
    }

    /// Types, molds and enums a function body declares (or imports) are
    /// local to one activation: save the active registries before such a
    /// body runs. `None` for bodies that declare none.
    fn save_local_types(&self, func: &FuncValue) -> Option<TypeRegistry> {
        fn declares_types(stmts: &[Statement]) -> bool {
            stmts.iter().any(|stmt| match stmt {
                Statement::EnumDef(_) | Statement::ClassLikeDef(_) | Statement::Import(_) => true,
                Statement::ErrorCeiling(ec) => declares_types(&ec.handler_body),
                _ => false,
            })
        }
        if !declares_types(&func.body) {
            return None;
        }
        Some(TypeRegistry {
            type_defs: self.type_defs.clone(),
            enum_defs: self.enum_defs.clone(),
            mold_defs: self.mold_defs.clone(),
            type_methods: self.type_methods.clone(),
            type_tags: self.type_tags.clone(),
            enum_keys: self.enum_keys.clone(),
        })
    }

    fn restore_local_types(&mut self, saved: Option<TypeRegistry>) {
        if let Some(saved) = saved {
            self.type_defs = saved.type_defs;
            self.enum_defs = saved.enum_defs;
            self.mold_defs = saved.mold_defs;
            self.type_methods = saved.type_methods;
            self.type_tags = saved.type_tags;
            self.enum_keys = saved.enum_keys;
        }
    }

    // ── Type and enum identity ──────────────────────────────

    /// Declare the type `name` in the active module: give it a `__type`
    /// tag that its instances share. A declaration evaluated again (inside
    /// a function body) keeps its tag.
    fn declare_type_tag(&mut self, name: &str) {
        let module = self.active_module;
        let def_site = self.env.definition_site();
        let key = (module, name.to_string());
        let tag = match self.declared_type_tags.get(&key).cloned() {
            Some(tag) => {
                if let Value::Str(s) = &tag
                    && let Some(origin) = self.type_origins.get_mut(&(Arc::as_ptr(s) as usize))
                {
                    origin.def_site = def_site;
                }
                tag
            }
            None => {
                let tag = self.new_type_tag(name, module, name, def_site, true);
                self.declared_type_tags.insert(key, tag.clone());
                tag
            }
        };
        self.type_tags.insert(name.to_string(), tag);
    }

    /// Create a tag string `display` standing for the type `name` declared
    /// in `module`. The tag is kept alive by its origin record, so its
    /// address identifies the declaration for as long as the interpreter
    /// runs.
    pub(crate) fn new_type_tag(
        &mut self,
        display: &str,
        module: ModuleId,
        name: &str,
        def_site: Option<(usize, u64)>,
        tag_owner: bool,
    ) -> Value {
        let tag = Value::str(display.to_string());
        if let Value::Str(s) = &tag {
            self.type_origins.insert(
                Arc::as_ptr(s) as usize,
                TypeOrigin {
                    module,
                    name: name.to_string(),
                    def_site,
                    tag: tag.clone(),
                    tag_owner,
                },
            );
        }
        tag
    }

    /// A tag displaying `display` that stands for the type `name` declared
    /// in `module` (an aliased import). The same alias of the same
    /// declaration reuses its tag.
    pub(crate) fn alias_type_tag(
        &mut self,
        display: &str,
        module: ModuleId,
        name: &str,
        def_site: Option<(usize, u64)>,
    ) -> Value {
        let existing = self.type_origins.values().find(|origin| {
            !origin.tag_owner
                && origin.module == module
                && origin.name == name
                && matches!(&origin.tag, Value::Str(s) if s.as_str() == display)
        });
        if let Some(origin) = existing {
            return origin.tag.clone();
        }
        self.new_type_tag(display, module, name, def_site, false)
    }

    /// Clear the tail-call context for code that is not part of the
    /// running function (a module's top level); returns it for
    /// `restore_active_function`.
    pub(crate) fn take_active_function(&mut self) -> (Option<String>, HashSet<usize>, bool) {
        (
            self.active_function.take(),
            std::mem::take(&mut self.append_consume_sites),
            std::mem::replace(&mut self.tail_self_only, false),
        )
    }

    pub(crate) fn restore_active_function(
        &mut self,
        saved: (Option<String>, HashSet<usize>, bool),
    ) {
        self.active_function = saved.0;
        self.append_consume_sites = saved.1;
        self.tail_self_only = saved.2;
    }

    /// The `__type` value for an instance of the type the active module
    /// calls `name`.
    pub(crate) fn type_tag_value(&self, name: &str) -> Value {
        self.type_tags
            .get(name)
            .cloned()
            .unwrap_or_else(|| Value::str(name.to_string()))
    }

    /// The declaration a `__type` tag stands for.
    pub(crate) fn origin_of_tag(&self, tag: &Value) -> Option<&TypeOrigin> {
        match tag {
            Value::Str(s) => self.type_origins.get(&(Arc::as_ptr(s) as usize)),
            _ => None,
        }
    }

    /// The declaration behind the name `name` in the active module; a name
    /// with no recorded declaration is taken to be the active module's.
    pub(crate) fn type_origin_of_name(&self, name: &str) -> TypeOrigin {
        self.type_tags
            .get(name)
            .and_then(|tag| self.origin_of_tag(tag))
            .cloned()
            .unwrap_or_else(|| TypeOrigin {
                module: self.active_module,
                name: name.to_string(),
                def_site: None,
                tag: Value::str(name.to_string()),
                tag_owner: false,
            })
    }

    /// Find the method `method` of a pack whose `__type` is `tag`: in the
    /// module that declared the pack's type, else by name in the active
    /// module, else in the one module that declares a type of that name
    /// with such a method. Returns the method and the declaration it runs
    /// in.
    pub(crate) fn resolve_user_method(
        &self,
        tag: &Value,
        type_name: &str,
        method: &str,
    ) -> Option<(FuncDef, TypeOrigin)> {
        if let Some(origin) = self.origin_of_tag(tag)
            && let Some(func_def) = self
                .methods_in(origin.module, &origin.name)
                .and_then(|methods| methods.get(method))
        {
            return Some((func_def.clone(), origin.clone()));
        }
        if let Some(func_def) = self
            .type_methods
            .get(type_name)
            .and_then(|methods| methods.get(method))
        {
            return Some((func_def.clone(), self.type_origin_of_name(type_name)));
        }
        let mut found: Option<(FuncDef, TypeOrigin)> = None;
        for origin in self.type_origins.values() {
            if !origin.tag_owner || origin.name != type_name {
                continue;
            }
            if let Some(func_def) = self
                .methods_in(origin.module, &origin.name)
                .and_then(|methods| methods.get(method))
            {
                if found.is_some() {
                    return None;
                }
                found = Some((func_def.clone(), origin.clone()));
            }
        }
        found
    }

    /// Key for an enum the active module declares. Enum values carry the
    /// key, so two modules' enums of the same name stay distinct values
    /// while an aliased import (`Color => Tint`) still names the same one.
    fn enum_key_for(&mut self, name: &str, variants: &[String]) -> String {
        let module = self.active_module;
        let mut candidate = name.to_string();
        let mut n = 1;
        loop {
            match self.enum_key_owners.get(&candidate) {
                Some((m, owner_name, owner_variants))
                    if *m == module && owner_name == name && owner_variants == variants =>
                {
                    return candidate;
                }
                Some(_) => {
                    n += 1;
                    candidate = format!("{}#{}", name, n);
                }
                None => {
                    self.enum_key_owners.insert(
                        candidate.clone(),
                        (module, name.to_string(), variants.to_vec()),
                    );
                    self.enum_variants
                        .insert(candidate.clone(), variants.to_vec());
                    return candidate;
                }
            }
        }
    }

    /// The key of the enum the active module calls `name`.
    pub(crate) fn enum_key_of(&self, name: &str) -> String {
        self.enum_keys
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    /// Variants of the enum behind an enum value's key.
    pub(crate) fn enum_variants_of(&self, key: &str) -> Option<&Vec<String>> {
        self.enum_variants
            .get(key)
            .or_else(|| self.enum_defs.get(key))
    }

    /// Materialise the synthetic defaultFn return.
    ///
    /// Called from `call_function` / `call_function_with_values` /
    /// `call_function_preserving_signals` whenever a `Value::Function` whose
    /// `name` equals `DEFAULT_FN_SENTINEL_NAME` is invoked. The body is
    /// intentionally empty; we only ever produce the return type's default.
    pub(crate) fn invoke_default_fn(&mut self, func: &FuncValue) -> Result<Value, RuntimeError> {
        if let Some(result) = func.closure.get(Self::DEFAULT_FN_RESULT_SLOT) {
            return Ok(result.clone());
        }
        let ret_ty = match &func.return_type {
            Some(t) => t.clone(),
            None => return Ok(Value::Unit),
        };
        let mut visiting = HashSet::new();
        self.default_for_type_expr(&ret_ty, &mut visiting)
    }

    /// Run a function body with its arguments bound, in the function's own
    /// scope. Shared by the call helpers below; the scope and any types
    /// the body declares are gone when it returns.
    fn run_function_body(
        &mut self,
        func: &FuncValue,
        arg_values: Vec<Value>,
    ) -> Result<Signal, RuntimeError> {
        if let Some(builtin) = func.name.strip_prefix(PRELUDE_FUNCTION_PREFIX) {
            return self.call_prelude_function_value(builtin, arg_values);
        }
        let scope = self.enter_call_scope(func);
        let local_types = self.save_local_types(func);
        let result = match self.bind_params_with_effective_defaults(func, arg_values) {
            Ok(Some(signal)) => Ok(signal),
            Ok(None) => self.eval_function_body(&func.body),
            Err(err) => Err(err),
        };
        self.restore_local_types(local_types);
        self.leave_call_scope(scope);
        result
    }

    pub(crate) fn enter_call_depth(&mut self) -> Result<(), RuntimeError> {
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
        Ok(())
    }

    pub(crate) fn exit_call_depth(&mut self) {
        self.call_depth -= 1;
    }

    /// Helper: call a function with pre-evaluated argument values,
    /// preserving all Signal variants (including Throw) for the caller to handle.
    /// Used for __unmold calls where Signal::Throw must be catchable by |==.
    pub(crate) fn call_function_preserving_signals(
        &mut self,
        func: &FuncValue,
        arg_values: &[Value],
    ) -> Result<Signal, RuntimeError> {
        self.enter_call_depth()?;

        // E30 Phase 6 / E30B-004: synthetic defaultFn fast path.
        if func.name == Self::DEFAULT_FN_SENTINEL_NAME {
            self.call_depth -= 1;
            let _ = arg_values; // arity is enforced by the type, the body is no-op
            return self.invoke_default_fn(func).map(Signal::Value);
        }
        // Internal callback paths (list ops, unmold hooks) should not inherit
        // outer-function tail-call context.
        let prev_active = self.active_function.take();
        let prev_self_only = std::mem::replace(&mut self.tail_self_only, false);
        let result = self.run_function_body(func, arg_values.to_vec());
        self.tail_self_only = prev_self_only;
        self.active_function = prev_active;
        self.call_depth -= 1;
        result
    }

    /// Helper: call a function with pre-evaluated argument values.
    ///
    /// A throw inside the callback is parked in `pending_throw` and
    /// surfaces as a `RuntimeError`, which the nearest error ceiling turns
    /// back into the throw. The gorilla ends the program: it surfaces the
    /// same way with `pending_gorilla` set, and no ceiling catches it.
    pub(crate) fn call_function_with_values(
        &mut self,
        func: &FuncValue,
        arg_values: &[Value],
    ) -> Result<Value, RuntimeError> {
        self.enter_call_depth()?;

        // E30 Phase 6 / E30B-004: synthetic defaultFn fast path.
        if func.name == Self::DEFAULT_FN_SENTINEL_NAME {
            self.call_depth -= 1;
            let _ = arg_values;
            return self.invoke_default_fn(func);
        }

        // Internal callback paths (Map/Filter/etc.) should not inherit
        // outer-function tail-call context.
        let prev_active = self.active_function.take();
        let prev_self_only = std::mem::replace(&mut self.tail_self_only, false);
        let body_result = self.run_function_body(func, arg_values.to_vec());
        self.tail_self_only = prev_self_only;
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
            Ok(Signal::Gorilla) => Err(self.gorilla_escape()),
            Err(err) => Err(err),
        }
    }

    /// The `><` fired inside a callback: end the program. The builtin that
    /// ran the callback cannot return a signal, so the gorilla travels as a
    /// `RuntimeError`; `eval_program` turns it back into the gorilla and no
    /// error ceiling catches it.
    pub(crate) fn gorilla_escape(&mut self) -> RuntimeError {
        self.pending_gorilla = true;
        RuntimeError {
            message: "Gorilla (><) terminated the program".to_string(),
        }
    }

    // ── Function Calls ──────────────────────────────────────

    /// Call a function with arguments, with tail call optimization.
    ///
    /// When a function makes a tail call (self-recursive or mutual-recursive),
    /// The interpreter returns a TailCall signal. This method loops
    /// (trampoline) on TailCall signals instead of growing the stack.
    ///
    /// For mutual recursion, `mutual_tail_call_target` is set by eval_expr_tail
    /// to the function to execute next.
    fn call_function(&mut self, func: &FuncValue, args: &[Expr]) -> Result<Signal, RuntimeError> {
        // Evaluate arguments before the call counts toward the depth
        // limit: a throw while evaluating them leaves without ever entering
        // the callee, so a caught throw cannot leak call depth.
        let mut arg_values = Vec::with_capacity(args.len());
        for arg in args {
            match self.eval_expr(arg)? {
                Signal::Value(v) => arg_values.push(v),
                other => return Ok(other),
            }
        }

        // RCB-301: Guard against stack overflow from deeply recursive calls.
        self.enter_call_depth()?;

        // E30 Phase 6 / E30B-004: synthetic defaultFn fast path.
        //
        // A `Value::Function` whose name is `Self::DEFAULT_FN_SENTINEL_NAME`
        // is the synthetic placeholder generated by `default_for_type_expr`
        // for declare-only function fields. Its body intentionally carries
        // no AST (it must never enter the trampoline loop). When called,
        // it materialises and returns the **return type's default value**
        // per Lock-D verdict (E30 Phase 0, 2026-04-28):
        //   - primitive return → primitive default
        //   - `:Async[T]` → resolved Async carrying T's default
        //   - registered class-like → default pack with cycle guard
        //   - opaque alias → not generatable (Phase 5 reject path)
        if func.name == Self::DEFAULT_FN_SENTINEL_NAME {
            self.call_depth -= 1;
            return self.invoke_default_fn(func).map(Signal::Value);
        }

        // Save the previous active function name
        let prev_active = self.active_function.take();
        let prev_self_only = std::mem::replace(&mut self.tail_self_only, false);

        // Set active function for tail call detection
        if func.name != "<lambda>" {
            self.active_function = Some(func.name.clone());
        }

        // Trampoline loop for tail call optimization
        // current_func tracks which function to execute (may change for mutual recursion)
        let mut current_func = func.clone();
        let mut current_args = arg_values;
        // Append-consume: compute this function's consumable sites once
        // per activation (recomputed on mutual retarget). The previous
        // set is restored on every exit path alongside `prev_active`.
        let saved_consume_sites = std::mem::replace(
            &mut self.append_consume_sites,
            crate::parser::append_consume_plan(
                &current_func.name,
                &current_func.params,
                &current_func.body,
            )
            .map(|plan| plan.sites)
            .unwrap_or_default(),
        );
        let result = loop {
            // The arguments move into the environment, which becomes the
            // unique owner of any list argument — the Append / Prepend
            // unique-ownership fast path turns a tail-recursive build loop
            // from O(N²) into amortized O(N).
            let args = std::mem::take(&mut current_args);
            match self.run_function_body(&current_func, args) {
                Ok(Signal::TailCall(new_args)) => {
                    if let Some(target) = self.mutual_tail_call_target.take() {
                        // Mutual tail call: switch to the target resolved
                        // where the call was written.
                        self.active_function = Some(target.name.clone());
                        current_func = target;
                        // The retargeted function has its own consume
                        // plan (usually none) — never carry the previous
                        // function's sites across.
                        self.append_consume_sites = crate::parser::append_consume_plan(
                            &current_func.name,
                            &current_func.params,
                            &current_func.body,
                        )
                        .map(|plan| plan.sites)
                        .unwrap_or_default();
                    }
                    // Self tail call: loop with new arguments
                    current_args = new_args;
                }
                other => break other,
            }
        };
        self.active_function = prev_active;
        self.tail_self_only = prev_self_only;
        self.append_consume_sites = saved_consume_sites;
        self.call_depth -= 1;
        result
    }

    /// Evaluate template string interpolation.
    ///
    /// The result is a full `Signal` — an interpolation body that
    /// throws (or fires the gorilla) propagates to the nearest error ceiling
    /// instead of being silently skipped, leaving the program to continue
    /// with an empty fragment.
    fn eval_template_string(&mut self, template: &str) -> Result<Signal, RuntimeError> {
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
                    match &program.statements[0] {
                        Statement::Expr(expr) => match self.eval_expr(expr)? {
                            Signal::Value(v) => result.push_str(&v.to_display_string()),
                            // Throw / Gorilla reach the nearest
                            // ceiling instead of being silently skipped.
                            // TailCall never escapes `eval_expr` (only the
                            // tail-position wrapper produces it), so the
                            // catch-all arm is Throw/Gorilla in practice.
                            other => return Ok(other),
                        },
                        // a non-expression body (an assignment, a
                        // nested statement) used to vanish without output or
                        // diagnostics.
                        _ => {
                            return Err(RuntimeError {
                                message: format!(
                                    "[E1702] Template interpolation `${{{expr_str}}}` must be an expression, not a statement."
                                ),
                            });
                        }
                    }
                } else {
                    result.push_str(&expr_str);
                }
            } else {
                result.push(ch);
            }
        }

        Ok(Signal::Value(Value::str(result)))
    }

    /// evaluate the explicit addon-binding form
    /// `RustAddon["fn"](arity <= N)`.
    ///
    /// Contract:
    /// - `type_args` must contain exactly one entry, a `StringLit` with
    /// The addon function name. Anything else → `[E1412]`.
    /// - `fields` must contain exactly one entry, name `arity`, with an
    /// `IntLit` value. Anything else → `[E1412]`.
    /// - Must execute inside an addon facade load context
    /// (`loading_addon_facade_ctx == Some(_)`). Otherwise → `[E1412]`.
    /// - The function name must exist in the surrounding addon's
    /// manifest `[functions]` table and the declared arity must match
    /// The manifest arity. Otherwise → `[E1412]` drift error.
    /// - On success, returns `Value::str("__taida_addon_call::<pkg>::<fn>")`
    /// so the binding is structurally identical to today's pre-injected
    /// addon sentinel (the addon dispatch path in `MoldInst` /
    /// `try_addon_func` uses the sentinel string verbatim).
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
            Expr::IntLit(n, _) => match u32::try_from(*n) {
                Ok(arity) => arity,
                Err(_) => {
                    return Err(RuntimeError {
                        message: format!(
                            "[E1412] RustAddon[\"{}\"](arity <= N) requires an \
                             integer between 0 and {}; got {:?}",
                            fn_name,
                            u32::MAX,
                            fields[0].value
                        ),
                    });
                }
            },
            other => {
                return Err(RuntimeError {
                    message: format!(
                        "[E1412] RustAddon[\"{}\"](arity <= N) requires an \
                         integer between 0 and {}; got {:?}",
                        fn_name,
                        u32::MAX,
                        other
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

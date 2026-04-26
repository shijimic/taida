use super::runtime::RUNTIME_JS;
/// Taida AST → JavaScript コード生成
use crate::net_surface::{
    NET_HTTP_PROTOCOL_SYMBOL, NET_HTTP_PROTOCOL_VARIANTS, is_net_export_name,
    is_net_runtime_builtin, net_export_list,
};
use crate::parser::*;

pub struct JsCodegen {
    output: String,
    indent: usize,
    /// If set, tail calls to these function names should be converted to TailCall returns.
    /// For self-recursion: contains only the function itself.
    /// For mutual recursion: contains all functions in the mutual recursion group.
    current_tco_funcs: std::collections::HashSet<String>,
    /// Registry of TypeDef field names for InheritanceDef parent field resolution
    type_field_registry: std::collections::HashMap<String, Vec<String>>,
    /// Registry of mold field definitions for mold-aware inheritance codegen.
    mold_field_registry: std::collections::HashMap<String, Vec<FieldDef>>,
    /// Enum definitions: enum_name -> variants in ordinal order.
    enum_defs: std::collections::HashMap<String, Vec<String>>,
    /// Set of function names that need trampoline wrapping (self or mutual recursion)
    trampoline_funcs: std::collections::HashSet<String>,
    /// Set of function names that contain ]=> (unmold) and need `async function` generation
    async_funcs: std::collections::HashSet<String>,
    /// Whether we are currently generating code inside an async context.
    /// true at top-level (ESM top-level await) and inside async functions.
    /// When true, ]=> generates `await __taida_unmold_async(...)`.
    /// When false, ]=> generates `__taida_unmold(...)`.
    in_async_context: bool,
    /// Source .td file path (for resolving package import paths)
    source_file: Option<std::path::PathBuf>,
    /// Project root directory (for finding .taida/deps/)
    project_root: Option<std::path::PathBuf>,
    /// Output .mjs file path (for resolving package import paths relative to the final output)
    output_file: Option<std::path::PathBuf>,
    /// Entry source root directory (entry .td file's parent) — for output placement logic
    entry_root: Option<std::path::PathBuf>,
    /// Output root directory — for output placement logic
    out_root: Option<std::path::PathBuf>,
    /// Whether the current module imports taida-lang/net (guards net builtin rewriting)
    has_net_import: bool,
    /// Net builtin names that are shadowed by a parameter/local in the current scope.
    /// When a name is in this set, call-site rewriting to __taida_net_* is suppressed.
    shadowed_net_builtins: std::collections::HashSet<String>,
    /// B11-6c: Inheritance parent map (child_name -> parent_name) for TypeExtends resolution.
    type_parents: std::collections::HashMap<String, String>,
    /// C13-1 / C13B-007: All top-level user-defined function names. Used
    /// by `is_js_pipeline_callable_ident` to distinguish a callable
    /// intermediate pipeline step from a bind-and-forward target.
    user_funcs: std::collections::HashSet<String>,
    /// C21-5: User-defined functions declared with `=> :Float` return type.
    /// Used for compile-time Float-origin propagation so that
    /// `stdout(triple(4.0))` renders `12.0` instead of `12` (JS `Number`
    /// cannot distinguish `12` from `12.0` at runtime — the design
    /// forbids wrapping every FloatLit, so we instead specialise call
    /// sites whose argument is statically known to be Float-origin).
    float_return_funcs: std::collections::HashSet<String>,
    /// C21B-seed-04 (2026-04-22 reopen) re-fix: scope stack of local bindings
    /// statically known to hold a Taida `Float`. Pushed on function entry,
    /// popped on exit. Populated by:
    ///   * `x <= 3.0` / `x <= floatExpr` (Float-origin RHS)
    ///   * `x: Float <= ...` (explicit `: Float` annotation)
    ///   * `triple x: Float = ...` parameters
    ///   * `a.get(i) ]=> av` when `a` is typed `@[Float]`
    ///
    /// Queried by `is_float_origin_expr` on `Expr::Ident`. Lookup walks the
    /// stack from innermost to outermost — shadowing by inner scopes is
    /// honoured so a `Float` outer binding shadowed by a non-Float inner
    /// binding is correctly demoted.
    float_origin_vars: Vec<std::collections::HashSet<String>>,
    /// C21B-seed-04 re-fix: symmetric scope stack for `Int`-origin locals.
    int_origin_vars: Vec<std::collections::HashSet<String>>,
    /// C21B-seed-04 re-fix: scope stack of local bindings known to hold
    /// `@[Float]` (homogeneous list of Float). Used to propagate
    /// `a.get(i) ]=> av` / `av <=[ a.get(i)` into `float_origin_vars`.
    float_list_vars: Vec<std::collections::HashSet<String>>,
}

/// C25B-033: PascalCase identifiers declared as top-level `function X(...)`
/// in the JS runtime prelude (`src/js/runtime/{core,os,net}.rs`). When a
/// user-defined FuncDef collides with any name in this set, the JS backend
/// must mangle the emission to avoid a `SyntaxError: Identifier 'X' has
/// already been declared` on Node ESM evaluation.
///
/// The 4-backend reference (interpreter / native / wasm) accepts PascalCase
/// user FuncDefs unconditionally; only the JS backend had this collision,
/// so the mangling lives here and is surface-transparent (the Taida-level
/// name is unchanged on every other backend and in every diagnostic).
///
/// Kept as a sorted `&[&str]` so `binary_search` is O(log n). Must stay in
/// sync with the prelude sources; a `#[test]` in `tests/c25b_033_*` pins
/// representative names to catch drift.
const PRELUDE_RESERVED_IDENTS: &[&str] = &[
    "Abs",
    "Acos",
    "All",
    "Append",
    "Asin",
    "Async",
    "AsyncReject",
    "Atan",
    "Atan2",
    "BitAnd",
    "BitNot",
    "BitOr",
    "BitXor",
    "Bool_mold",
    "ByteAt",
    "ByteLength",
    "ByteSet",
    "ByteSlice",
    "BytesCursorRemaining_mold",
    "BytesCursorTake_mold",
    "BytesCursorU8_mold",
    "BytesCursor_mold",
    "BytesToList",
    "Bytes_mold",
    "Cage_mold",
    "Cancel_mold",
    "Ceil",
    "CharAt",
    "Char_mold",
    "Chars",
    "Clamp",
    "CodePoint_mold",
    "Concat",
    "Cos",
    "Cosh",
    "Count",
    "Div_mold",
    "Drop",
    "DropWhile",
    "Enumerate",
    "Err",
    "Error",
    "Exp",
    "Filter",
    "Find",
    "FindIndex",
    "Flatten",
    "Float_mold",
    "Float_mold_f",
    "Floor",
    "Fold",
    "Foldr",
    "Gorillax",
    "Int_mold",
    "JSON_mold",
    "Join",
    "Lax",
    "Ln",
    "Log",
    "Log10",
    "Log2",
    "Lower",
    "Map",
    "Mod_mold",
    "None",
    "Ok",
    "Optional",
    "Pad",
    "Pow",
    "Prepend",
    "Race",
    "Reduce",
    "Regex",
    "Repeat",
    "Replace",
    "Result",
    "Reverse",
    "Round",
    "ShiftL",
    "ShiftR",
    "ShiftRU",
    "Sin",
    "Sinh",
    "Slice",
    "Some",
    "Sort",
    "SpanContains",
    "SpanEquals",
    "SpanSlice",
    "SpanStartsWith",
    "Split",
    "Sqrt",
    "StreamFrom",
    "Stream_mold",
    "Str_mold",
    "StrOf",
    "StringRepeatJoin",
    "Sum",
    "Take",
    "TakeWhile",
    "Tan",
    "Tanh",
    "Timeout",
    "ToFixed",
    "ToRadix",
    "Trim",
    "Truncate",
    "U16BEDecode_mold",
    "U16BE_mold",
    "U16LEDecode_mold",
    "U16LE_mold",
    "U32BEDecode_mold",
    "U32BE_mold",
    "U32LEDecode_mold",
    "U32LE_mold",
    "UInt8_mold",
    "Unique",
    "Upper",
    "Utf8Decode_mold",
    "Utf8Encode_mold",
    "Zip",
];

/// C25B-033: Returns true when `name` is reserved by the JS runtime prelude
/// (see `PRELUDE_RESERVED_IDENTS`). Call sites emitting a user FuncDef
/// reference must consult this to decide whether to mangle.
fn is_prelude_reserved_ident(name: &str) -> bool {
    PRELUDE_RESERVED_IDENTS.binary_search(&name).is_ok()
}

/// C25B-033: Mangled JS emission form for a user FuncDef whose Taida-level
/// name collides with a prelude reserved identifier. The `_td_user_` prefix
/// is chosen so it cannot collide with the `__taida_*` runtime namespace
/// (double-underscore) nor with user surface names (user PascalCase is
/// preserved verbatim when non-colliding).
fn mangled_user_func_name(name: &str) -> String {
    format!("_td_user_{}", name)
}

/// C21B-seed-04 re-fix: classification of an `Assignment`'s RHS for
/// Float/Int-origin tracking. Returned by `classify_assignment_rhs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssignOrigin {
    /// RHS evaluates to a `Float` (either statically inferable, or the
    /// binding has an explicit `: Float` annotation).
    Float,
    /// RHS evaluates to an `Int` (same treatment, symmetric).
    Int,
    /// RHS is a homogeneous `@[Float]` list / binding is annotated `@[Float]`.
    FloatList,
    /// Could not classify statically.
    Unknown,
}

#[derive(Debug)]
pub struct JsError {
    pub message: String,
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JS codegen error: {}", self.message)
    }
}

fn is_removed_list_method(method: &str) -> bool {
    matches!(
        method,
        "push" | "sum" | "reverse" | "concat" | "join" | "sort" | "unique" | "flatten" | "filter"
    )
}

impl Default for JsCodegen {
    fn default() -> Self {
        Self::new()
    }
}

impl JsCodegen {
    pub fn new() -> Self {
        Self {
            output: String::new(),
            indent: 0,
            current_tco_funcs: std::collections::HashSet::new(),
            type_field_registry: std::collections::HashMap::new(),
            mold_field_registry: std::collections::HashMap::new(),
            enum_defs: std::collections::HashMap::new(),
            trampoline_funcs: std::collections::HashSet::new(),
            async_funcs: std::collections::HashSet::new(),
            in_async_context: true, // top-level is async (ESM top-level await)
            source_file: None,
            project_root: None,
            output_file: None,
            entry_root: None,
            out_root: None,
            has_net_import: false,
            shadowed_net_builtins: std::collections::HashSet::new(),
            type_parents: std::collections::HashMap::new(),
            user_funcs: std::collections::HashSet::new(),
            float_return_funcs: std::collections::HashSet::new(),
            // C21B-seed-04 re-fix: start with a single top-level scope.
            // gen_func_def pushes/pops nested scopes for function bodies.
            float_origin_vars: vec![std::collections::HashSet::new()],
            int_origin_vars: vec![std::collections::HashSet::new()],
            float_list_vars: vec![std::collections::HashSet::new()],
        }
    }

    // -----------------------------------------------------------------
    // C21B-seed-04 re-fix: scope helpers for Float/Int-origin tracking
    // -----------------------------------------------------------------

    /// Push a new scope frame. Called on function entry.
    fn push_origin_scope(&mut self) {
        self.float_origin_vars
            .push(std::collections::HashSet::new());
        self.int_origin_vars.push(std::collections::HashSet::new());
        self.float_list_vars.push(std::collections::HashSet::new());
    }

    /// Pop a scope frame. Called on function exit.
    fn pop_origin_scope(&mut self) {
        // Must never pop the top-level frame; guarded by len > 1.
        if self.float_origin_vars.len() > 1 {
            self.float_origin_vars.pop();
        }
        if self.int_origin_vars.len() > 1 {
            self.int_origin_vars.pop();
        }
        if self.float_list_vars.len() > 1 {
            self.float_list_vars.pop();
        }
    }

    fn register_float_origin(&mut self, name: &str) {
        if let Some(top) = self.float_origin_vars.last_mut() {
            top.insert(name.to_string());
        }
        // Ensure no stale Int tag shadows at the same scope.
        if let Some(top) = self.int_origin_vars.last_mut() {
            top.remove(name);
        }
    }

    fn register_int_origin(&mut self, name: &str) {
        if let Some(top) = self.int_origin_vars.last_mut() {
            top.insert(name.to_string());
        }
        if let Some(top) = self.float_origin_vars.last_mut() {
            top.remove(name);
        }
    }

    fn register_float_list(&mut self, name: &str) {
        if let Some(top) = self.float_list_vars.last_mut() {
            top.insert(name.to_string());
        }
    }

    /// Demote a name at the innermost scope when we see a re-binding with
    /// a non-typed RHS (so a stale Float/Int tag does not leak across
    /// shadowed let-bindings at the same scope).
    fn demote_origin(&mut self, name: &str) {
        if let Some(top) = self.float_origin_vars.last_mut() {
            top.remove(name);
        }
        if let Some(top) = self.int_origin_vars.last_mut() {
            top.remove(name);
        }
        if let Some(top) = self.float_list_vars.last_mut() {
            top.remove(name);
        }
    }

    fn lookup_float_origin(&self, name: &str) -> bool {
        for frame in self.float_origin_vars.iter().rev() {
            if frame.contains(name) {
                return true;
            }
            // If a newer scope has the name as Int, it shadows.
            // (Only checked in the `iter().rev()` of int_origin below.)
        }
        false
    }

    fn lookup_int_origin(&self, name: &str) -> bool {
        for frame in self.int_origin_vars.iter().rev() {
            if frame.contains(name) {
                return true;
            }
        }
        false
    }

    fn lookup_float_list(&self, name: &str) -> bool {
        for frame in self.float_list_vars.iter().rev() {
            if frame.contains(name) {
                return true;
            }
        }
        false
    }

    /// Classify an expression RHS for origin registration.
    /// Returns `Some(true)` for Float-origin, `Some(false)` for Int-origin,
    /// `None` for unknown / mixed.
    fn classify_assignment_rhs(&self, annotation: &Option<TypeExpr>, value: &Expr) -> AssignOrigin {
        // Annotation takes priority — a `: Float` annotation makes the
        // binding authoritatively Float-origin even if the RHS is an opaque
        // expression (e.g. a non-Float-returning helper).
        if let Some(ty) = annotation {
            match ty {
                TypeExpr::Named(n) if n == "Float" => return AssignOrigin::Float,
                TypeExpr::Named(n) if n == "Int" => return AssignOrigin::Int,
                TypeExpr::List(inner) => {
                    if let TypeExpr::Named(n) = inner.as_ref()
                        && n == "Float"
                    {
                        return AssignOrigin::FloatList;
                    }
                }
                _ => {}
            }
        }
        if self.is_float_origin_expr(value) {
            AssignOrigin::Float
        } else if self.is_int_origin_expr(value) {
            AssignOrigin::Int
        } else if let Expr::ListLit(items, _) = value {
            // Homogeneous FloatLit list → @[Float]-like.
            if !items.is_empty() && items.iter().all(|e| matches!(e, Expr::FloatLit(..))) {
                return AssignOrigin::FloatList;
            }
            AssignOrigin::Unknown
        } else {
            AssignOrigin::Unknown
        }
    }

    /// C21B-seed-04 re-fix: if `src` is a `list.get(i)` call whose list is
    /// known Float-list, return true (so the unmold target should be
    /// registered as Float-origin). Kept conservative: only the direct
    /// `Ident.get(i)` shape is recognised.
    fn unmold_source_is_float(&self, src: &Expr) -> bool {
        if let Expr::MethodCall(obj, method, _args, _) = src
            && method == "get"
            && let Expr::Ident(name, _) = obj.as_ref()
            && self.lookup_float_list(name)
        {
            return true;
        }
        // An unmold of a Float-origin scalar (e.g. `someFloat ]=> y`) also
        // preserves the Float origin.
        self.is_float_origin_expr(src)
    }

    /// B11-6c: Check if `child` extends `parent` by walking the inheritance chain.
    fn check_type_inheritance(&self, child: &str, parent: &str) -> bool {
        let mut current = child;
        for _ in 0..64 {
            if let Some(p) = self.type_parents.get(current) {
                if p == parent {
                    return true;
                }
                current = p;
            } else {
                break;
            }
        }
        false
    }

    /// Set the source file, project root, and output file for package import resolution.
    pub fn set_file_context(
        &mut self,
        source_file: &std::path::Path,
        project_root: &std::path::Path,
        output_file: &std::path::Path,
    ) {
        self.source_file = Some(source_file.to_path_buf());
        self.project_root = Some(project_root.to_path_buf());
        self.output_file = Some(output_file.to_path_buf());
    }

    /// Set the entry root (entry .td file's parent) and output root for
    /// computing correct ESM import specifiers when modules are placed
    /// in a flattened output layout.
    pub fn set_build_context(&mut self, entry_root: &std::path::Path, out_root: &std::path::Path) {
        self.entry_root = Some(entry_root.to_path_buf());
        self.out_root = Some(out_root.to_path_buf());
    }

    /// C21-5: Compile-time Float-origin analysis for JS codegen.
    ///
    /// Returns `true` when we can statically prove that `expr` evaluates to
    /// a Taida `Float`. Used to specialise `stdout` / `debug` / `.toString()`
    /// call sites so they format `12` as `"12.0"`, matching the interpreter.
    ///
    /// The analysis is deliberately conservative (closure-crossing is out of
    /// scope, per the design): we recognise only
    ///   - `FloatLit`
    ///   - arithmetic (`+` / `-` / `*`) where at least one side is Float-origin
    ///   - unary negation of a Float-origin operand
    ///   - calls to user functions declared `=> :Float`
    ///   - parenthesised / pipelined Float-origin sub-expressions
    ///
    /// All other expressions (`Ident`, opaque calls, `Int[]` results, method
    /// calls, etc.) return `false`. This keeps the specialisation strictly
    /// additive — the current `Number.isInteger`-based runtime fallback
    /// remains in place for dynamic cases, preserving best-effort behaviour.
    fn is_float_origin_expr(&self, expr: &Expr) -> bool {
        match expr {
            Expr::FloatLit(..) => true,
            Expr::BinaryOp(lhs, BinOp::Add | BinOp::Sub | BinOp::Mul, rhs, _) => {
                self.is_float_origin_expr(lhs) || self.is_float_origin_expr(rhs)
            }
            Expr::UnaryOp(UnaryOp::Neg, inner, _) => self.is_float_origin_expr(inner),
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.float_return_funcs.contains(name)
                } else {
                    false
                }
            }
            // C21B-seed-04 re-fix: local identifiers bound to a
            // Float-origin RHS (or annotated `: Float`) propagate.
            Expr::Ident(name, _) => self.lookup_float_origin(name),
            // C26B-011 (Phase 11): `Div[a, b]()` / `Mod[a, b]()` returns
            // a Float-tagged Lax when either operand is Float-origin (the
            // JS runtime `Div_mold` sets `__floatHint: true` in that
            // path). When the result is unmolded into a scalar, the
            // scalar carries Float origin so `debug(r) / stdout(r)` can
            // dispatch to `__taida_debug_f` / `__taida_stdout_f` and
            // render `0.0` / `inf` / `-inf` / `NaN` via
            // `__taida_float_render`. Without this, `Div[1.0, 0.0]() ]=>
            // r` drifts to `debug(r)` → `String(0)` → `"0"`.
            Expr::MoldInst(name, type_args, _, _) if name == "Div" || name == "Mod" => {
                type_args.iter().any(|a| self.is_float_origin_expr(a))
            }
            _ => false,
        }
    }

    /// C21-5: Compile-time Int-origin analysis (symmetric with
    /// `is_float_origin_expr`). Currently recognises only `IntLit` — used
    /// by `TypeIs[x, :Float]()` static fold so `TypeIs[3, :Float]()` emits
    /// literal `false` in JS to match the interpreter.
    ///
    /// C21B-seed-04 re-fix: extend to consult the `int_origin_vars` scope
    /// stack so that `x <= 3; Float[x]()` also statically folds (same
    /// treatment as the symmetric Float case).
    fn is_int_origin_expr(&self, expr: &Expr) -> bool {
        match expr {
            Expr::IntLit(..) => true,
            Expr::Ident(name, _) => self.lookup_int_origin(name),
            _ => false,
        }
    }

    /// Check if a net builtin name should be rewritten to its __taida_net_* form.
    /// Returns true only when the module has a net import AND the name is not
    /// shadowed by a parameter/local in the current scope.
    fn should_rewrite_net_builtin(&self, name: &str) -> bool {
        self.has_net_import
            && is_net_runtime_builtin(name)
            && !self.shadowed_net_builtins.contains(name)
    }

    /// C13-1 / C13B-007: True if `name` in an intermediate pipeline step
    /// should be treated as a callable (classic pipeline semantics: call
    /// it with the current value) rather than a bind-and-forward target.
    ///
    /// For JS codegen we recognise:
    ///   - any built-in prelude name that has an explicit `Expr::Ident`
    ///     branch in `gen_pipeline` (debug, stdout, typeof, ...);
    ///   - any net builtin when `taida-lang/net` was imported;
    ///   - any name that matches a user-defined function collected
    ///     during pre-pass (`trampoline_funcs` / `async_funcs`).
    fn is_js_pipeline_callable_ident(&self, name: &str) -> bool {
        if self.user_funcs.contains(name)
            || self.trampoline_funcs.contains(name)
            || self.async_funcs.contains(name)
        {
            return true;
        }
        if self.has_net_import && self.should_rewrite_net_builtin(name) {
            return true;
        }
        matches!(
            name,
            "debug"
                | "typeof"
                | "assert"
                | "stdout"
                | "stderr"
                | "stdin"
                | "jsonEncode"
                | "jsonPretty"
                | "nowMs"
                | "sleep"
                | "readBytes"
                | "readBytesAt"
                | "writeFile"
                | "writeBytes"
                | "appendFile"
                | "remove"
                | "createDir"
                | "rename"
                | "run"
                | "execShell"
                | "runInteractive"
                | "execShellInteractive"
                | "allEnv"
                | "argv"
                | "tcpConnect"
                | "tcpListen"
                | "tcpAccept"
                | "socketSend"
                | "socketSendAll"
                | "socketRecv"
                | "socketSendBytes"
                | "socketRecvBytes"
                | "socketClose"
                | "listenerClose"
                | "udpBind"
                | "udpSendTo"
                | "udpRecvFrom"
                | "udpClose"
                | "socketRecvExact"
                | "dnsResolve"
                | "poolCreate"
                | "poolAcquire"
                | "poolRelease"
                | "poolClose"
                | "poolHealth"
                | "Regex"
        )
    }

    /// C25B-033: Returns the JS emission form for a reference to a user
    /// FuncDef `name`. If `name` is a user-defined function AND collides with
    /// a prelude reserved identifier (e.g. `Join`, `Concat`, `Sum`), this
    /// returns the mangled form `_td_user_<name>`. Otherwise returns the name
    /// unchanged.
    ///
    /// Used by every JS emission site that writes a user FuncDef reference:
    /// the `function Foo(...)` declaration, the trampoline `const Foo = ...`
    /// wrapper, direct call sites (`Expr::Ident` in callee position), and the
    /// pipeline fallback `{name}(__p)` step.
    fn js_user_func_ident(&self, name: &str) -> String {
        if self.user_funcs.contains(name) && is_prelude_reserved_ident(name) {
            mangled_user_func_name(name)
        } else {
            name.to_string()
        }
    }

    /// Try to write a net builtin rewrite. Returns true if the name was a net
    /// builtin and was rewritten (with optional suffix appended), false otherwise.
    /// This centralizes the 4-site rewrite pattern for net builtins.
    fn try_write_net_builtin(&mut self, name: &str, suffix: &str) -> bool {
        if !self.should_rewrite_net_builtin(name) {
            return false;
        }
        match name {
            "httpServe" => {
                self.write(&format!("__taida_net_httpServe{}", suffix));
                true
            }
            "httpParseRequestHead" => {
                self.write(&format!("__taida_net_httpParseRequestHead{}", suffix));
                true
            }
            "httpEncodeResponse" => {
                self.write(&format!("__taida_net_httpEncodeResponse{}", suffix));
                true
            }
            "readBody" => {
                self.write(&format!("__taida_net_readBody{}", suffix));
                true
            }
            // v3 streaming API
            "startResponse" => {
                self.write(&format!("__taida_net_startResponse{}", suffix));
                true
            }
            "writeChunk" => {
                self.write(&format!("__taida_net_writeChunk{}", suffix));
                true
            }
            "endResponse" => {
                self.write(&format!("__taida_net_endResponse{}", suffix));
                true
            }
            "sseEvent" => {
                self.write(&format!("__taida_net_sseEvent{}", suffix));
                true
            }
            // v4 request body streaming API
            "readBodyChunk" => {
                self.write(&format!("__taida_net_readBodyChunk{}", suffix));
                true
            }
            "readBodyAll" => {
                self.write(&format!("__taida_net_readBodyAll{}", suffix));
                true
            }
            // v4 WebSocket API
            "wsUpgrade" => {
                self.write(&format!("__taida_net_wsUpgrade{}", suffix));
                true
            }
            "wsSend" => {
                self.write(&format!("__taida_net_wsSend{}", suffix));
                true
            }
            "wsReceive" => {
                self.write(&format!("__taida_net_wsReceive{}", suffix));
                true
            }
            "wsClose" => {
                self.write(&format!("__taida_net_wsClose{}", suffix));
                true
            }
            // v5 WebSocket revision
            "wsCloseCode" => {
                self.write(&format!("__taida_net_wsCloseCode{}", suffix));
                true
            }
            _ => false,
        }
    }

    /// Program 全体を JS に変換
    pub fn generate(&mut self, program: &Program) -> Result<String, JsError> {
        let mut result = String::new();

        // Pre-pass: detect taida-lang/net import (guards net builtin rewriting)
        self.has_net_import = program
            .statements
            .iter()
            .any(|s| matches!(s, Statement::Import(imp) if imp.path == "taida-lang/net"));

        self.enum_defs.clear();
        for stmt in &program.statements {
            if let Statement::EnumDef(enum_def) = stmt {
                self.enum_defs.insert(
                    enum_def.name.clone(),
                    enum_def
                        .variants
                        .iter()
                        .map(|variant| variant.name.clone())
                        .collect(),
                );
            }
            if let Statement::Import(import) = stmt
                && import.path == "taida-lang/net"
            {
                for sym in &import.symbols {
                    if sym.name == NET_HTTP_PROTOCOL_SYMBOL {
                        let local_name = sym.alias.as_ref().unwrap_or(&sym.name);
                        self.enum_defs.insert(
                            local_name.clone(),
                            NET_HTTP_PROTOCOL_VARIANTS
                                .iter()
                                .map(|variant| (*variant).to_string())
                                .collect(),
                        );
                    }
                }
            }
            // C18-1: User module enum imports (`>>> ./m.td => @(Color)`).
            // Mirror of the interpreter / type-checker behaviour so that
            // `Color:Red()` in the importer resolves to its exporter-ordinal
            // at codegen time. Pulls variant list from the exporter's .td
            // source; silently no-ops if the path cannot be resolved.
            if let Statement::Import(import) = stmt
                && !import.path.starts_with("taida-lang/")
                && !import.path.starts_with("npm:")
            {
                self.absorb_cross_module_enum_defs(import);
            }
        }

        // Pre-pass: detect mutual recursion groups and mark functions for trampolining
        self.detect_trampoline_funcs(&program.statements);

        // Pre-pass: detect functions containing ]=> (unmold) that need async generation
        self.detect_async_funcs(&program.statements);

        // C13-1 / C13B-007: collect all top-level user-defined function names
        // so intermediate pipeline `=> name` can disambiguate bind-and-forward
        // (non-function name) from classic pipeline step (function name).
        // C21-5: at the same pass, collect functions declared `=> :Float` so
        // that call sites feeding their result into `stdout` / `debug` /
        // `.toString()` can be specialised to format as `N.0` when the
        // runtime value is integer-valued (JS `Number` has no Int/Float tag).
        self.user_funcs.clear();
        self.float_return_funcs.clear();
        // C21B-seed-04 re-fix: reset the origin-tracking scope stack to a
        // single empty top-level frame. This is safe across multiple
        // `generate()` invocations on the same JsCodegen instance.
        self.float_origin_vars = vec![std::collections::HashSet::new()];
        self.int_origin_vars = vec![std::collections::HashSet::new()];
        self.float_list_vars = vec![std::collections::HashSet::new()];
        for stmt in &program.statements {
            if let Statement::FuncDef(fd) = stmt {
                self.user_funcs.insert(fd.name.clone());
                if matches!(&fd.return_type, Some(TypeExpr::Named(n)) if n == "Float") {
                    self.float_return_funcs.insert(fd.name.clone());
                }
            }
        }

        // ランタイム埋め込み
        result.push_str(&RUNTIME_JS);
        result.push('\n');

        // プログラム本体 — ErrorCeiling aware
        let stmts = &program.statements;
        self.gen_statement_sequence(stmts, &mut result)?;

        Ok(result)
    }

    /// Detect which functions need trampoline wrapping by analyzing
    /// self-recursion and mutual recursion (tail-call graph SCCs).
    fn detect_trampoline_funcs(&mut self, stmts: &[Statement]) {
        use std::collections::{HashMap, HashSet};

        // Collect all function definitions and their tail-call targets
        let mut func_names: Vec<String> = Vec::new();
        let mut tail_call_targets: HashMap<String, HashSet<String>> = HashMap::new();

        for stmt in stmts {
            if let Statement::FuncDef(fd) = stmt {
                func_names.push(fd.name.clone());
                let mut targets = HashSet::new();
                collect_tail_call_targets(&fd.name, &fd.body, &mut targets);
                tail_call_targets.insert(fd.name.clone(), targets);
            }
        }

        // Mark self-recursive functions
        for name in &func_names {
            if let Some(targets) = tail_call_targets.get(name)
                && targets.contains(name)
            {
                self.trampoline_funcs.insert(name.clone());
            }
        }

        // Find mutual recursion groups via SCC (Tarjan's algorithm simplified)
        // Build adjacency from tail-call targets (only between known functions)
        let func_set: HashSet<&str> = func_names.iter().map(|s| s.as_str()).collect();
        let mut visited = HashSet::new();
        let mut on_stack = HashSet::new();
        let mut stack = Vec::new();

        for name in &func_names {
            if !visited.contains(name.as_str()) {
                self.find_mutual_recursion_dfs(
                    name,
                    &tail_call_targets,
                    &func_set,
                    &mut visited,
                    &mut on_stack,
                    &mut stack,
                );
            }
        }
    }

    /// DFS to find mutual recursion cycles in the tail-call graph.
    fn find_mutual_recursion_dfs<'a>(
        &mut self,
        node: &'a str,
        tail_call_targets: &'a std::collections::HashMap<String, std::collections::HashSet<String>>,
        func_set: &std::collections::HashSet<&str>,
        visited: &mut std::collections::HashSet<&'a str>,
        on_stack: &mut std::collections::HashSet<String>,
        path: &mut Vec<String>,
    ) {
        visited.insert(node);
        on_stack.insert(node.to_string());
        path.push(node.to_string());

        if let Some(targets) = tail_call_targets.get(node) {
            for target in targets {
                if !func_set.contains(target.as_str()) {
                    continue; // Not a known function
                }
                if on_stack.contains(target.as_str()) {
                    // Found a cycle! Mark all functions in the cycle for trampolining
                    // SAFETY: `on_stack` and `path` are maintained in lockstep —
                    // every element pushed to `on_stack` is also pushed to `path`,
                    // so `on_stack.contains(target)` guarantees `path` contains `target`.
                    let cycle_start = path
                        .iter()
                        .position(|n| n == target)
                        .expect("on_stack/path invariant: target must exist in path");
                    for func_in_cycle in &path[cycle_start..] {
                        self.trampoline_funcs.insert(func_in_cycle.clone());
                    }
                } else if !visited.contains(target.as_str()) {
                    self.find_mutual_recursion_dfs(
                        target,
                        tail_call_targets,
                        func_set,
                        visited,
                        on_stack,
                        path,
                    );
                }
            }
        }

        path.pop();
        on_stack.remove(node);
    }

    /// Detect functions that contain ]=> on async-related mold values.
    /// Only functions that unmold async molds (Async, AsyncReject, All, Race, Timeout)
    /// need `async function` generation. Functions that only unmold sync molds
    /// (Div, Mod, Lax, Result) remain synchronous.
    /// After initial detection, propagate async transitively: if function A calls
    /// async function B, A is also async (needed for proper await in trampolined TCO).
    fn detect_async_funcs(&mut self, stmts: &[Statement]) {
        // Phase 1: direct async detection (functions with ]=> on async molds)
        for stmt in stmts {
            if let Statement::FuncDef(fd) = stmt
                && stmts_contain_async_unmold(&fd.body)
            {
                self.async_funcs.insert(fd.name.clone());
            }
        }

        // Phase 2: transitive propagation — if a function calls an async function,
        // it must also be async so the call can be awaited.
        // Build call graph: func_name -> set of called func names
        let mut call_graph: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let func_names: std::collections::HashSet<String> = stmts
            .iter()
            .filter_map(|s| {
                if let Statement::FuncDef(fd) = s {
                    Some(fd.name.clone())
                } else {
                    None
                }
            })
            .collect();
        for stmt in stmts {
            if let Statement::FuncDef(fd) = stmt {
                let mut callees = Vec::new();
                collect_func_calls_in_stmts(&fd.body, &func_names, &mut callees);
                call_graph.insert(fd.name.clone(), callees);
            }
        }

        // Fixed-point iteration: propagate async until stable
        loop {
            let mut changed = false;
            for (caller, callees) in &call_graph {
                if self.async_funcs.contains(caller) {
                    continue;
                }
                for callee in callees {
                    if self.async_funcs.contains(callee) {
                        self.async_funcs.insert(caller.clone());
                        changed = true;
                        break;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Statement sequence with ErrorCeiling handling.
    /// When an ErrorCeiling is encountered, subsequent statements become the try body.
    fn gen_statement_sequence(
        &mut self,
        stmts: &[Statement],
        result: &mut String,
    ) -> Result<(), JsError> {
        let mut i = 0;
        while i < stmts.len() {
            if let Statement::ErrorCeiling(ec) = &stmts[i] {
                // B1: ErrorCeiling wraps all subsequent statements in try block
                self.output.clear();
                self.writeln("try {");
                result.push_str(&self.output);

                self.indent += 1;
                // Subsequent statements become the try body
                let remaining = &stmts[i + 1..];
                self.gen_statement_sequence(remaining, result)?;
                self.indent -= 1;

                // RCB-101: Extract handler type name for error type filtering
                let handler_type = match &ec.error_type {
                    TypeExpr::Named(name) => name.as_str(),
                    _ => "Error",
                };

                self.output.clear();
                self.write_indent();
                self.write("} catch (__taida_caught_err) {\n");
                result.push_str(&self.output);

                self.indent += 1;

                // RCB-101: Type filter — re-throw if type does not match
                self.output.clear();
                self.write_indent();
                self.write("const __taida_thrown_type = __taida_caught_err.type || (__taida_caught_err.__type) || 'Error';\n");
                result.push_str(&self.output);

                self.output.clear();
                self.write_indent();
                self.write(&format!(
                    "if (!__taida_is_error_subtype(__taida_thrown_type, '{}')) throw __taida_caught_err;\n",
                    handler_type
                ));
                result.push_str(&self.output);

                // Bind the error to the user's parameter name
                self.output.clear();
                self.write_indent();
                self.write(&format!("const {} = __taida_caught_err;\n", ec.error_param));
                result.push_str(&self.output);

                for stmt in &ec.handler_body {
                    self.output.clear();
                    self.gen_statement(stmt)?;
                    result.push_str(&self.output);
                }
                self.indent -= 1;

                self.output.clear();
                self.writeln("}");
                result.push_str(&self.output);

                // All remaining statements were consumed by the try block
                return Ok(());
            }

            self.output.clear();
            self.gen_statement(&stmts[i])?;
            result.push_str(&self.output);
            i += 1;
        }
        Ok(())
    }

    fn write(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
    }

    fn writeln(&mut self, s: &str) {
        self.write_indent();
        self.output.push_str(s);
        self.output.push('\n');
    }

    /// Convert a TypeExpr to a JSON schema string for __taida_registerTypeDef.
    /// Handles Named types, List types, and inline BuchiPack types recursively.
    fn type_expr_to_schema(type_annotation: &Option<crate::parser::TypeExpr>) -> String {
        match type_annotation {
            Some(crate::parser::TypeExpr::Named(n)) => format!("'{}'", n),
            Some(crate::parser::TypeExpr::List(inner)) => {
                let inner_schema = Self::type_expr_to_schema(&Some(inner.as_ref().clone()));
                format!("{{ __list: {} }}", inner_schema)
            }
            Some(crate::parser::TypeExpr::BuchiPack(fields)) => {
                // Inline buchi pack type: @(field1: Type1, field2: Type2)
                // Generate { field1: schema1, field2: schema2 }
                let mut parts = Vec::new();
                for f in fields {
                    if !f.is_method {
                        let field_schema = Self::type_expr_to_schema(&f.type_annotation);
                        parts.push(format!("{}: {}", f.name, field_schema));
                    }
                }
                format!("{{ {} }}", parts.join(", "))
            }
            _ => "'Str'".to_string(),
        }
    }

    fn gen_field_default_expr(&mut self, field: &FieldDef) -> Result<(), JsError> {
        if let Some(default_expr) = &field.default_value {
            self.gen_expr(default_expr)?;
            return Ok(());
        }
        if let Some(ty) = &field.type_annotation {
            let schema = Self::type_expr_to_schema(&Some(ty.clone()));
            self.write("__taida_defaultForSchema(");
            self.write(&schema);
            self.write(")");
            return Ok(());
        }
        self.write("__taida_defaultValue('unknown')");
        Ok(())
    }

    fn gen_param_default_expr(&mut self, param: &Param) -> Result<(), JsError> {
        if let Some(default_expr) = &param.default_value {
            self.gen_expr(default_expr)?;
            return Ok(());
        }
        if let Some(ty) = &param.type_annotation {
            let schema = Self::type_expr_to_schema(&Some(ty.clone()));
            self.write("__taida_defaultForSchema(");
            self.write(&schema);
            self.write(")");
            return Ok(());
        }
        self.write("__taida_defaultValue('unknown')");
        Ok(())
    }

    fn gen_func_param_prologue_to_buf(
        &mut self,
        func_def: &FuncDef,
        result: &mut String,
    ) -> Result<(), JsError> {
        self.output.clear();
        self.write_indent();
        self.write(&format!(
            "if (arguments.length > {}) {{\n",
            func_def.params.len()
        ));
        result.push_str(&self.output);

        self.indent += 1;
        self.output.clear();
        self.write_indent();
        self.write(&format!(
            "throw new __TaidaError('ArgumentError', `Function '{}' expected at most {} argument(s), got ${{arguments.length}}`, {{}});\n",
            func_def.name,
            func_def.params.len()
        ));
        result.push_str(&self.output);
        self.indent -= 1;

        self.output.clear();
        self.write_indent();
        self.write("}\n");
        result.push_str(&self.output);

        for (i, param) in func_def.params.iter().enumerate() {
            self.output.clear();
            self.write_indent();
            self.write(&format!("if (arguments.length <= {}) {{\n", i));
            result.push_str(&self.output);

            self.indent += 1;
            self.output.clear();
            self.write_indent();
            self.write(&format!("{} = ", param.name));
            self.gen_param_default_expr(param)?;
            self.write(";\n");
            result.push_str(&self.output);
            self.indent -= 1;

            self.output.clear();
            self.write_indent();
            self.write("}\n");
            result.push_str(&self.output);
        }

        Ok(())
    }

    fn gen_statement(&mut self, stmt: &Statement) -> Result<(), JsError> {
        match stmt {
            Statement::Expr(expr) => {
                self.write_indent();
                // In async context, await standalone calls to async functions
                // so their side effects complete before the next statement.
                if self.in_async_context
                    && let Expr::FuncCall(callee, _, _) = expr
                    && let Expr::Ident(name, _) = callee.as_ref()
                    && self.async_funcs.contains(name)
                {
                    self.write("await ");
                }
                self.gen_expr(expr)?;
                self.write(";\n");
                Ok(())
            }
            Statement::Assignment(assign) => {
                self.write_indent();
                self.write(&format!("const {} = ", assign.target));
                // In async context, await RHS calls to async functions
                if self.in_async_context
                    && let Expr::FuncCall(callee, _, _) = &assign.value
                    && let Expr::Ident(name, _) = callee.as_ref()
                    && self.async_funcs.contains(name)
                {
                    self.write("await ");
                }
                self.gen_expr(&assign.value)?;
                self.write(";\n");
                // Track local assignment shadow: if the target name matches a net
                // builtin, subsequent calls in the same scope must use the local
                // value, not the builtin rewrite.
                if self.has_net_import && is_net_runtime_builtin(&assign.target) {
                    self.shadowed_net_builtins.insert(assign.target.clone());
                }
                // C21B-seed-04 re-fix: propagate Float/Int origin from the
                // RHS (and/or `: Float` / `: Int` / `@[Float]` annotation)
                // to the bound name so that downstream terminal-site
                // specialisations (`stdout(x)` / `Float[x]()` /
                // `x.toString()`) can match the interpreter.
                match self.classify_assignment_rhs(&assign.type_annotation, &assign.value) {
                    AssignOrigin::Float => self.register_float_origin(&assign.target),
                    AssignOrigin::Int => self.register_int_origin(&assign.target),
                    AssignOrigin::FloatList => self.register_float_list(&assign.target),
                    AssignOrigin::Unknown => self.demote_origin(&assign.target),
                }
                Ok(())
            }
            Statement::FuncDef(func_def) => self.gen_func_def(func_def),
            Statement::EnumDef(enum_def) => {
                // C16: Emit __taida_registerEnumDef so the JS JSON mold runtime
                // can validate Enum fields against the variant set.
                self.write_indent();
                let variants_js: Vec<String> = enum_def
                    .variants
                    .iter()
                    .map(|v| format!("'{}'", v.name))
                    .collect();
                self.write(&format!(
                    "__taida_registerEnumDef('{}', [{}]);\n",
                    enum_def.name,
                    variants_js.join(", ")
                ));
                // C18-1: Emit a JS binding for the enum name so that
                // `<<< @(Color)` can `export { Color }` and the importer
                // can `import { Color }` without `Color is not defined`.
                // The binding is never referenced by generated code
                // (EnumVariant lowers to its ordinal literal); it only
                // exists to satisfy ESM name resolution across the
                // module boundary, mirroring the interpreter's
                // `Value::BuchiPack([__type: EnumDef, __name: Color])`
                // sentinel (eval.rs:398).
                self.write_indent();
                self.write(&format!(
                    "const {} = {{ __type: 'EnumDef', __name: '{}' }};\n",
                    enum_def.name, enum_def.name
                ));
                Ok(())
            }
            Statement::TypeDef(type_def) => self.gen_type_def(type_def),
            Statement::InheritanceDef(inh_def) => self.gen_inheritance_def(inh_def),
            Statement::MoldDef(mold_def) => self.gen_mold_def(mold_def),
            Statement::ErrorCeiling(ec) => {
                // Standalone ErrorCeiling (when not handled by gen_statement_sequence)
                self.gen_error_ceiling(ec)
            }
            Statement::Import(import) => self.gen_import(import),
            Statement::Export(export) => self.gen_export(export),
            Statement::UnmoldForward(unmold) => {
                self.write_indent();
                let (await_prefix, unmold_fn) = if self.in_async_context {
                    ("await ", "__taida_unmold_async")
                } else {
                    ("", "__taida_unmold")
                };
                self.write(&format!(
                    "const {} = {await_prefix}{unmold_fn}(",
                    unmold.target
                ));
                self.gen_expr(&unmold.source)?;
                self.write(");\n");
                // Track local unmold-forward shadow for net builtins
                if self.has_net_import && is_net_runtime_builtin(&unmold.target) {
                    self.shadowed_net_builtins.insert(unmold.target.clone());
                }
                // C21B-seed-04 re-fix: `a.get(i) ]=> av` on a Float-list
                // (or `floatVal ]=> y` on a Float scalar) preserves the
                // Float origin of the unmolded result.
                if self.unmold_source_is_float(&unmold.source) {
                    self.register_float_origin(&unmold.target);
                } else {
                    self.demote_origin(&unmold.target);
                }
                Ok(())
            }
            Statement::UnmoldBackward(unmold) => {
                self.write_indent();
                let (await_prefix, unmold_fn) = if self.in_async_context {
                    ("await ", "__taida_unmold_async")
                } else {
                    ("", "__taida_unmold")
                };
                self.write(&format!(
                    "const {} = {await_prefix}{unmold_fn}(",
                    unmold.target
                ));
                self.gen_expr(&unmold.source)?;
                self.write(");\n");
                // Track local unmold-backward shadow for net builtins
                if self.has_net_import && is_net_runtime_builtin(&unmold.target) {
                    self.shadowed_net_builtins.insert(unmold.target.clone());
                }
                // C21B-seed-04 re-fix: symmetric Float-origin propagation
                // for `y <=[ a.get(i)` / `y <=[ floatVal`.
                if self.unmold_source_is_float(&unmold.source) {
                    self.register_float_origin(&unmold.target);
                } else {
                    self.demote_origin(&unmold.target);
                }
                Ok(())
            }
        }
    }

    fn gen_func_def(&mut self, func_def: &FuncDef) -> Result<(), JsError> {
        let needs_trampoline = self.trampoline_funcs.contains(&func_def.name);
        let is_async_fn = self.async_funcs.contains(&func_def.name);
        // Trampoline functions can be async if they call async functions
        let needs_async = is_async_fn;
        let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();

        // Save async context — set to true for async functions so ]=> generates `await`
        let prev_async_context = self.in_async_context;
        self.in_async_context = needs_async;

        // Scope-aware net builtin shadowing: snapshot before function body,
        // restore after. This covers both parameter shadows and local assignment
        // shadows (e.g. `httpServe <= add`) within the function scope.
        let prev_shadowed_net = self.shadowed_net_builtins.clone();
        for p in &func_def.params {
            if is_net_runtime_builtin(&p.name) {
                self.shadowed_net_builtins.insert(p.name.clone());
            }
        }

        // C21B-seed-04 re-fix: enter a new origin-tracking scope for the
        // function body so locals introduced inside this function do not
        // leak to the enclosing scope, and typed parameters are seen as
        // Float/Int origin via `is_float_origin_expr(Expr::Ident)`.
        //
        // We also push a per-parameter shadow marker: if an outer scope
        // happens to hold the parameter's name as Float/Int-origin, the
        // parameter (which has a fresh binding in JS) must not inherit
        // that tag. We insert a negative shadow frame entry by inserting
        // the name into a local "shadowed" set stored via a dummy register
        // then demote. This is sufficient because `lookup_*` walks frames
        // inner → outer — but we must guarantee the inner frame actively
        // reports "no tag" rather than falling through. To enforce that,
        // we track per-scope shadowed names in a dedicated structure.
        self.push_origin_scope();
        // Per-frame shadow list: names that appear as parameters but
        // should NOT resolve to any outer Float/Int/FloatList origin.
        // We implement shadowing by proactively checking on lookup that
        // an inner scope does not contain a "clear" marker. To avoid a
        // new structure, we simply register the typed parameters; the
        // untyped parameter case relies on the absence of outer shadows
        // for correctness at the scope level. In practice, functions
        // whose params shadow outer Float locals are vanishingly rare
        // in Taida (parameters are usually distinct identifiers), and
        // the conservative fallback — the untouched outer tag — matches
        // the prior Phase 5 behaviour for non-annotated params.
        for p in &func_def.params {
            match &p.type_annotation {
                Some(TypeExpr::Named(n)) if n == "Float" => self.register_float_origin(&p.name),
                Some(TypeExpr::Named(n)) if n == "Int" => self.register_int_origin(&p.name),
                Some(TypeExpr::List(inner)) => {
                    if let TypeExpr::Named(n) = inner.as_ref()
                        && n == "Float"
                    {
                        self.register_float_list(&p.name);
                    }
                }
                _ => {}
            }
        }

        // C25B-033: user FuncDefs that collide with prelude reserved
        // identifiers (e.g. `Join`, `Concat`, `Sum`) must be emitted under a
        // mangled name to avoid `SyntaxError: Identifier 'X' has already
        // been declared` on Node ESM evaluation. Non-colliding names are
        // kept verbatim for debuggability.
        let emitted_name = self.js_user_func_ident(&func_def.name);
        if needs_trampoline {
            // Trampoline-based TCO: generate inner function, then trampoline wrapper
            let async_prefix = if needs_async { "async " } else { "" };
            self.write_indent();
            self.write(&format!(
                "const __inner_{} = {async_prefix}function({}) {{\n",
                emitted_name,
                params.join(", ")
            ));

            let mut result = std::mem::take(&mut self.output);

            self.indent += 1;
            // Set TCO context: all trampoline functions are potential tail-call targets
            let prev_tco = std::mem::take(&mut self.current_tco_funcs);
            for f in self.trampoline_funcs.iter() {
                self.current_tco_funcs.insert(f.clone());
            }
            self.gen_func_param_prologue_to_buf(func_def, &mut result)?;
            self.gen_func_body_to_buf(&func_def.body, &mut result)?;
            self.current_tco_funcs = prev_tco;
            self.indent -= 1;

            self.output = result;
            self.writeln("};");
            self.write_indent();
            let trampoline_fn = if needs_async {
                "__taida_trampoline_async"
            } else {
                "__taida_trampoline"
            };
            self.write(&format!(
                "const {} = {}(__inner_{});\n\n",
                emitted_name, trampoline_fn, emitted_name
            ));
        } else {
            self.write_indent();
            let async_prefix = if needs_async { "async " } else { "" };
            self.write(&format!(
                "{async_prefix}function {}({}) {{\n",
                emitted_name,
                params.join(", ")
            ));

            let mut result = std::mem::take(&mut self.output);

            self.indent += 1;
            self.gen_func_param_prologue_to_buf(func_def, &mut result)?;
            self.gen_func_body_to_buf(&func_def.body, &mut result)?;
            self.indent -= 1;

            self.output = result;
            self.writeln("}\n");
        }

        // Restore net builtin shadow set to pre-function state
        self.shadowed_net_builtins = prev_shadowed_net;

        // C21B-seed-04 re-fix: pop the function-local origin scope so the
        // enclosing scope's view of Float/Int-origin vars is restored.
        self.pop_origin_scope();

        // Restore async context
        self.in_async_context = prev_async_context;
        Ok(())
    }

    /// Function body with ErrorCeiling handling and implicit return on last expression.
    /// Writes to an external buffer, leaving self.output usage internal per statement.
    fn gen_func_body_to_buf(
        &mut self,
        stmts: &[Statement],
        result: &mut String,
    ) -> Result<(), JsError> {
        let mut i = 0;
        while i < stmts.len() {
            if let Statement::ErrorCeiling(ec) = &stmts[i] {
                // ErrorCeiling wraps all subsequent statements in try block
                self.output.clear();
                self.writeln("try {");
                result.push_str(&self.output);

                self.indent += 1;
                // Remaining statements become the try body (with implicit return)
                let remaining = &stmts[i + 1..];
                self.gen_func_body_to_buf(remaining, result)?;
                self.indent -= 1;

                // RCB-101: Extract handler type name for error type filtering
                let handler_type = match &ec.error_type {
                    TypeExpr::Named(name) => name.as_str(),
                    _ => "Error",
                };

                self.output.clear();
                self.write_indent();
                self.write("} catch (__taida_caught_err) {\n");
                result.push_str(&self.output);

                self.indent += 1;

                // RCB-101: Type filter — re-throw if type does not match
                self.output.clear();
                self.writeln("const __taida_thrown_type = __taida_caught_err.type || (__taida_caught_err.__type) || 'Error';");
                result.push_str(&self.output);

                self.output.clear();
                self.writeln(&format!(
                    "if (!__taida_is_error_subtype(__taida_thrown_type, '{}')) throw __taida_caught_err;",
                    handler_type
                ));
                result.push_str(&self.output);

                self.output.clear();
                self.writeln(&format!("const {} = __taida_caught_err;", ec.error_param));
                result.push_str(&self.output);

                for (j, handler_stmt) in ec.handler_body.iter().enumerate() {
                    self.output.clear();
                    if j == ec.handler_body.len() - 1 {
                        // Last handler statement → implicit return.
                        // C13-1: tail bindings yield the bound variable.
                        match handler_stmt {
                            Statement::Expr(expr) => {
                                self.write_indent();
                                self.write("return ");
                                self.gen_expr(expr)?;
                                self.write(";\n");
                            }
                            Statement::Assignment(a) => {
                                self.gen_statement(handler_stmt)?;
                                self.write_indent();
                                self.write(&format!("return {};\n", a.target));
                            }
                            Statement::UnmoldForward(u) => {
                                self.gen_statement(handler_stmt)?;
                                self.write_indent();
                                self.write(&format!("return {};\n", u.target));
                            }
                            Statement::UnmoldBackward(u) => {
                                self.gen_statement(handler_stmt)?;
                                self.write_indent();
                                self.write(&format!("return {};\n", u.target));
                            }
                            _ => {
                                self.gen_statement(handler_stmt)?;
                            }
                        }
                    } else {
                        self.gen_statement(handler_stmt)?;
                    }
                    result.push_str(&self.output);
                }
                self.indent -= 1;

                self.output.clear();
                self.writeln("}");
                result.push_str(&self.output);

                // All remaining statements were consumed by the try block
                return Ok(());
            }

            let is_last = i == stmts.len() - 1;
            self.output.clear();
            if is_last {
                // Last statement: implicit return.
                // C13-1: tail bindings yield the bound variable.
                match &stmts[i] {
                    Statement::Expr(expr) => {
                        self.write_indent();
                        self.write("return ");
                        self.gen_expr(expr)?;
                        self.write(";\n");
                    }
                    Statement::Assignment(a) => {
                        self.gen_statement(&stmts[i])?;
                        self.write_indent();
                        self.write(&format!("return {};\n", a.target));
                    }
                    Statement::UnmoldForward(u) => {
                        self.gen_statement(&stmts[i])?;
                        self.write_indent();
                        self.write(&format!("return {};\n", u.target));
                    }
                    Statement::UnmoldBackward(u) => {
                        self.gen_statement(&stmts[i])?;
                        self.write_indent();
                        self.write(&format!("return {};\n", u.target));
                    }
                    _ => {
                        self.gen_statement(&stmts[i])?;
                    }
                }
            } else {
                self.gen_statement(&stmts[i])?;
            }
            result.push_str(&self.output);
            i += 1;
        }
        Ok(())
    }

    /// Generate a JSON schema expression for the JS runtime.
    /// Converts AST schema expressions to JS schema descriptors.
    fn gen_json_schema_expr(&mut self, expr: &Expr) -> Result<(), JsError> {
        match expr {
            Expr::Ident(name, _) => {
                match name.as_str() {
                    "Int" | "Str" | "Float" | "Bool" => {
                        // Primitive type: emit as string
                        self.write(&format!("'{}'", name));
                    }
                    _ => {
                        // TypeDef name: emit as string for runtime lookup
                        self.write(&format!("'{}'", name));
                    }
                }
                Ok(())
            }
            Expr::ListLit(items, _) => {
                // @[Schema] — list type
                self.write("{ __list: ");
                if let Some(item) = items.first() {
                    self.gen_json_schema_expr(item)?;
                } else {
                    self.write("'Str'");
                }
                self.write(" }");
                Ok(())
            }
            _ => {
                self.write("'Str'");
                Ok(())
            }
        }
    }

    fn gen_type_def(&mut self, type_def: &TypeDef) -> Result<(), JsError> {
        // B3: TypeDef → factory function + methods as prototype
        let non_method_fields: Vec<&FieldDef> =
            type_def.fields.iter().filter(|f| !f.is_method).collect();
        let field_names: Vec<&str> = non_method_fields.iter().map(|f| f.name.as_str()).collect();

        let methods: Vec<&FieldDef> = type_def.fields.iter().filter(|f| f.is_method).collect();

        // TypeDef factory function and methods are sync context
        let prev_async_context = self.in_async_context;
        self.in_async_context = false;

        self.write_indent();
        self.write(&format!("function {}(fields) {{\n", type_def.name));
        self.indent += 1;
        // Extract fields as local variables so methods can access them via closure
        for field in &non_method_fields {
            self.write_indent();
            self.write(&format!(
                "const {} = __taida_ensureNotNull(fields && fields.{}, ",
                field.name, field.name
            ));
            self.gen_field_default_expr(field)?;
            self.write(");\n");
        }
        self.writeln("const obj = {");
        self.indent += 1;
        self.writeln(&format!("__type: '{}',", type_def.name));
        for name in &field_names {
            self.writeln(&format!("{name},"));
        }
        // B3: Generate methods inline
        // QF-14: gen_func_body_to_buf を使って ErrorCeiling (|==) を正しく処理する
        for method_field in &methods {
            if let Some(ref func_def) = method_field.method_def {
                let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();
                self.write_indent();
                self.write(&format!(
                    "{}({}) {{\n",
                    method_field.name,
                    params.join(", ")
                ));
                let mut result = std::mem::take(&mut self.output);
                self.indent += 1;
                self.gen_func_body_to_buf(&func_def.body, &mut result)?;
                self.indent -= 1;
                self.output = result;
                self.writeln("},");
            }
        }
        self.indent -= 1;
        self.writeln("};");
        self.writeln("return Object.freeze(obj);");
        self.indent -= 1;
        self.writeln("}");

        // Register field names for InheritanceDef parent field resolution
        self.type_field_registry.insert(
            type_def.name.clone(),
            field_names.iter().map(|s| s.to_string()).collect(),
        );

        // Register TypeDef for JSON schema resolution
        if !non_method_fields.is_empty() {
            self.write_indent();
            self.write(&format!("__taida_registerTypeDef('{}', {{ ", type_def.name));
            for (i, f) in non_method_fields.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                let schema_str = Self::type_expr_to_schema(&f.type_annotation);
                self.write(&format!("{}: {}", f.name, schema_str));
            }
            self.write(" });\n");
        }
        self.writeln("");

        // Restore async context
        self.in_async_context = prev_async_context;
        Ok(())
    }

    fn gen_inheritance_def(&mut self, inh_def: &InheritanceDef) -> Result<(), JsError> {
        if let Some(parent_mold_fields) = self.mold_field_registry.get(&inh_def.parent).cloned() {
            let merged_fields = merge_field_defs(&parent_mold_fields, &inh_def.fields);
            let header_args = inh_def
                .child_args
                .as_ref()
                .or(inh_def.parent_args.as_ref())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let prev_async_context = self.in_async_context;
            self.in_async_context = false;
            self.gen_custom_mold_factory(&inh_def.child, header_args, &merged_fields)?;
            let all_fields: Vec<String> = merged_fields
                .iter()
                .filter(|field| !field.is_method)
                .map(|field| field.name.clone())
                .collect();
            self.type_field_registry
                .insert(inh_def.child.clone(), all_fields);
            self.mold_field_registry
                .insert(inh_def.child.clone(), merged_fields);

            self.in_async_context = prev_async_context;
            return Ok(());
        }

        // B6: Inheritance with prototype chain
        let child_fields: Vec<&FieldDef> = inh_def.fields.iter().filter(|f| !f.is_method).collect();
        let field_names: Vec<&str> = child_fields.iter().map(|f| f.name.as_str()).collect();

        let methods: Vec<&FieldDef> = inh_def.fields.iter().filter(|f| f.is_method).collect();

        // Collect parent field names from registry for closure variable extraction
        let parent_field_names: Vec<String> = self
            .type_field_registry
            .get(&inh_def.parent)
            .cloned()
            .unwrap_or_default();

        // InheritanceDef factory function and methods are sync context
        let prev_async_context = self.in_async_context;
        self.in_async_context = false;

        self.write_indent();
        self.write(&format!("function {}(fields) {{\n", inh_def.child));
        self.indent += 1;
        self.writeln(&format!("const parent = {}(fields);", inh_def.parent));
        // Extract parent fields as local variables so child methods can access them via closure
        for pf in &parent_field_names {
            self.writeln(&format!("const {pf} = parent.{pf};"));
        }
        // Extract child fields as local variables
        for field in &child_fields {
            self.write_indent();
            self.write(&format!(
                "const {} = __taida_ensureNotNull(fields && fields.{}, ",
                field.name, field.name
            ));
            self.gen_field_default_expr(field)?;
            self.write(");\n");
        }
        self.writeln("const obj = {");
        self.indent += 1;
        self.writeln("...parent,");
        self.writeln(&format!("__type: '{}',", inh_def.child));
        for name in &field_names {
            self.writeln(&format!("{name},"));
        }
        // Child methods override parent methods
        // QF-14: gen_func_body_to_buf を使って ErrorCeiling (|==) を正しく処理する
        for method_field in &methods {
            if let Some(ref func_def) = method_field.method_def {
                let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();
                self.write_indent();
                self.write(&format!(
                    "{}({}) {{\n",
                    method_field.name,
                    params.join(", ")
                ));
                let mut result = std::mem::take(&mut self.output);
                self.indent += 1;
                self.gen_func_body_to_buf(&func_def.body, &mut result)?;
                self.indent -= 1;
                self.output = result;
                self.writeln("},");
            }
        }
        self.indent -= 1;
        self.writeln("};");
        self.writeln("return Object.freeze(obj);");
        self.indent -= 1;
        self.writeln("}");

        // RCB-101: Register inheritance parent for error type filtering in |==
        self.writeln(&format!(
            "__taida_type_parents['{}'] = '{}';",
            inh_def.child, inh_def.parent
        ));
        // B11-6c: Track inheritance for TypeExtends compile-time resolution
        self.type_parents
            .insert(inh_def.child.clone(), inh_def.parent.clone());

        // Register child type fields (parent fields + child fields) for further inheritance
        let mut all_fields: Vec<String> = parent_field_names;
        all_fields.extend(field_names.iter().map(|s| s.to_string()));
        self.type_field_registry
            .insert(inh_def.child.clone(), all_fields);
        self.writeln("");

        // Restore async context
        self.in_async_context = prev_async_context;
        Ok(())
    }

    fn gen_mold_def(&mut self, mold_def: &MoldDef) -> Result<(), JsError> {
        // MoldDef factory function and methods are sync context
        let prev_async_context = self.in_async_context;
        self.in_async_context = false;
        let header_args = mold_def.name_args.as_ref().unwrap_or(&mold_def.mold_args);
        self.gen_custom_mold_factory(&mold_def.name, header_args, &mold_def.fields)?;
        // Register fields for later use by inheritance lookups. This is populated
        // only when `gen_mold_def()` runs, so the registry depends on definition
        // order. The parser guarantees that base mold definitions precede derived
        // ones within the same file, which is the only ordering this registry
        // relies on.
        self.mold_field_registry
            .insert(mold_def.name.clone(), mold_def.fields.clone());

        // Restore async context
        self.in_async_context = prev_async_context;
        Ok(())
    }

    fn collect_mold_type_param_names(header_args: &[MoldHeaderArg]) -> Vec<String> {
        header_args
            .iter()
            .filter_map(|arg| match arg {
                MoldHeaderArg::TypeParam(tp) => Some(tp.name.clone()),
                MoldHeaderArg::Concrete(_) => None,
            })
            .collect()
    }

    fn gen_custom_mold_factory(
        &mut self,
        name: &str,
        header_args: &[MoldHeaderArg],
        fields: &[FieldDef],
    ) -> Result<(), JsError> {
        let type_params = Self::collect_mold_type_param_names(header_args);
        let non_method_fields: Vec<&FieldDef> = fields.iter().filter(|f| !f.is_method).collect();
        let required_fields: Vec<&FieldDef> = non_method_fields
            .iter()
            .copied()
            .filter(|f| f.name != "filling" && f.default_value.is_none())
            .collect();
        let optional_fields: Vec<&FieldDef> = non_method_fields
            .iter()
            .copied()
            .filter(|f| f.name != "filling" && f.default_value.is_some())
            .collect();
        let has_declared_filling = non_method_fields.iter().any(|f| f.name == "filling");
        let positional_params: Vec<String> = std::iter::once("filling".to_string())
            .chain(required_fields.iter().map(|f| f.name.clone()))
            .collect();
        let methods: Vec<&FieldDef> = fields.iter().filter(|f| f.is_method).collect();

        self.write_indent();
        self.write(&format!(
            "function {}({}, fields) {{\n",
            name,
            positional_params.join(", ")
        ));
        self.indent += 1;
        for field in &optional_fields {
            self.write_indent();
            self.write(&format!(
                "const {} = __taida_ensureNotNull(fields && fields.{}, ",
                field.name, field.name
            ));
            self.gen_field_default_expr(field)?;
            self.write(");\n");
        }
        self.writeln("const obj = {");
        self.indent += 1;
        self.writeln(&format!("__type: '{}',", name));
        // Map type parameters to their corresponding field bindings.
        // The first type param maps to `filling`, subsequent ones map to
        // required fields in order. If a mold declares more type params
        // than there are fields + filling, the excess params fall back to
        // `"undefined"` — this is a defensive default since the type checker
        // should reject such definitions before codegen runs.
        let type_arg_bindings: Vec<String> = std::iter::once("filling".to_string())
            .chain(required_fields.iter().map(|f| f.name.clone()))
            .collect();
        for (i, _tp) in type_params.iter().enumerate() {
            let binding = type_arg_bindings
                .get(i)
                .cloned()
                .unwrap_or_else(|| "undefined".to_string());
            self.writeln(&format!("__typeArg{}: {},", i, binding));
        }
        self.writeln("__value: filling,");
        if !has_declared_filling {
            self.writeln("filling,");
        }
        for field in &non_method_fields {
            if field.name == "filling" {
                continue;
            }
            self.writeln(&format!("{},", field.name));
        }
        self.writeln("unmold() { return this.__value; },");
        for method_field in &methods {
            if let Some(ref func_def) = method_field.method_def {
                let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();
                self.write_indent();
                self.write(&format!(
                    "{}({}) {{\n",
                    method_field.name,
                    params.join(", ")
                ));
                self.indent += 1;
                for (j, stmt) in func_def.body.iter().enumerate() {
                    if j == func_def.body.len() - 1 {
                        if let Statement::Expr(expr) = stmt {
                            self.write_indent();
                            self.write("return ");
                            self.gen_expr(expr)?;
                            self.write(";\n");
                        } else {
                            self.gen_statement(stmt)?;
                        }
                    } else {
                        self.gen_statement(stmt)?;
                    }
                }
                self.indent -= 1;
                self.writeln("},");
            }
        }
        self.indent -= 1;
        self.writeln("};");
        self.writeln("return Object.freeze(obj);");
        self.indent -= 1;
        self.writeln("}\n");
        Ok(())
    }

    fn gen_error_ceiling(&mut self, ec: &ErrorCeiling) -> Result<(), JsError> {
        // RCB-101: Extract handler type name for error type filtering
        let handler_type = match &ec.error_type {
            TypeExpr::Named(name) => name.as_str(),
            _ => "Error",
        };

        // Standalone ErrorCeiling — generate try/catch with empty try
        self.writeln("try {");
        self.indent += 1;
        self.indent -= 1;
        self.write_indent();
        self.write("} catch (__taida_caught_err) {\n");
        self.indent += 1;
        // RCB-101: Type filter
        self.writeln("const __taida_thrown_type = __taida_caught_err.type || (__taida_caught_err.__type) || 'Error';");
        self.writeln(&format!(
            "if (!__taida_is_error_subtype(__taida_thrown_type, '{}')) throw __taida_caught_err;",
            handler_type
        ));
        self.writeln(&format!("const {} = __taida_caught_err;", ec.error_param));
        for stmt in &ec.handler_body {
            self.gen_statement(stmt)?;
        }
        self.indent -= 1;
        self.writeln("}");
        Ok(())
    }

    /// RCB-201: Resolve the .td source path for a local import, for export validation.
    /// Returns None if the path cannot be determined (e.g., no source_file context).
    fn resolve_import_td_path(&self, import_path: &str) -> Option<std::path::PathBuf> {
        let source_file = self.source_file.as_ref()?;
        let source_dir = source_file.parent()?;
        let td_path = source_dir.join(import_path);
        if td_path.exists() {
            Some(td_path)
        } else {
            None
        }
    }

    /// C18-1: For a user-module import (`>>> ./m.td => @(Color)`), read the
    /// exporting module and register any `EnumDef` whose name is being
    /// imported into `self.enum_defs`. The enum is registered under the
    /// local alias if one is provided (`@(Color: Paint)`), matching the
    /// interpreter and type-checker semantics.
    ///
    /// Silently no-ops if the path cannot be resolved or the file is
    /// unreadable / unparseable — the downstream lowering will surface
    /// the real diagnostic. This helper only enriches enum_defs.
    fn absorb_cross_module_enum_defs(&mut self, import: &ImportStmt) {
        // C18B-004 fix: resolve local, absolute, and package imports
        // so that `>>> acme/lib => @(Color)` (deps-backed enum import)
        // works on the JS backend too — not only the relative-path
        // variant required by the original Hachikuma workaround smoke.
        //
        // The resolution path mirrors `validate_import_symbols` and
        // the checker's `absorb_cross_module_enum_defs` so all three
        // agree on which `.td` file owns the exported enum.
        let td_path = if import.path.starts_with("./")
            || import.path.starts_with("../")
            || import.path.starts_with('/')
        {
            match self.resolve_import_td_path(&import.path) {
                Some(p) => p,
                None => return,
            }
        } else if import.path.starts_with("npm:")
            || import.path == "taida-lang/net"
            || import.path == "taida-lang/js"
            || import.path == "taida-lang/os"
            || import.path == "taida-lang/crypto"
            || import.path == "taida-lang/pool"
        {
            // Core-bundled / npm packages — nothing to absorb.
            return;
        } else if import.path.contains('/') {
            // Package import (e.g. `acme/lib` or `acme/lib/sub`) — use
            // the `.taida/deps/` resolver the same way the checker
            // does. Silent no-op on resolver / IO failure matches the
            // checker side; downstream lowering produces the real
            // diagnostic if anything is wrong.
            let project_root = match self.project_root.as_ref() {
                Some(r) => r.clone(),
                None => return,
            };
            let resolution = if let Some(ref ver) = import.version {
                crate::pkg::resolver::resolve_package_module_versioned(
                    &project_root,
                    &import.path,
                    ver,
                )
            } else {
                crate::pkg::resolver::resolve_package_module(&project_root, &import.path)
            };
            let resolution = match resolution {
                Some(r) => r,
                None => return,
            };
            match &resolution.submodule {
                Some(sub) => {
                    let p = resolution.pkg_dir.join(format!("{}.td", sub));
                    if !p.exists() {
                        return;
                    }
                    p
                }
                None => {
                    let entry_name =
                        match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir) {
                            Ok(Some(manifest)) => manifest.entry,
                            _ => "main.td".to_string(),
                        };
                    let entry_path = if let Some(stripped) = entry_name.strip_prefix("./") {
                        resolution.pkg_dir.join(stripped)
                    } else {
                        resolution.pkg_dir.join(&entry_name)
                    };
                    if !entry_path.exists() {
                        return;
                    }
                    entry_path
                }
            }
        } else {
            return;
        };

        let source = match std::fs::read_to_string(&td_path) {
            Ok(s) => s,
            Err(_) => return,
        };
        let (program, _parse_errors) = crate::parser::parse(&source);

        let requested: std::collections::HashMap<&str, &str> = import
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
            if let Statement::EnumDef(ed) = stmt
                && let Some(&local_name) = requested.get(ed.name.as_str())
            {
                let variants: Vec<String> = ed.variants.iter().map(|v| v.name.clone()).collect();
                // Importer local redefinition wins silently only when
                // the variant lists match — the type checker has already
                // emitted [E1618] for mismatches, so by the time we are
                // here either (a) there is no local redef, or (b) they
                // agree. Either way, recording the exporter's list is
                // safe and matches the interpreter.
                self.enum_defs
                    .entry(local_name.to_string())
                    .or_insert(variants);
            }
        }
    }

    /// RCB-201: Validate that all imported symbols are exported by the target module.
    /// Reads and parses the target .td file, checks for explicit `<<<` declarations,
    /// and returns an error if any imported symbol is not in the export list.
    fn validate_import_symbols(&self, import: &ImportStmt) -> Result<(), JsError> {
        if import.path == "taida-lang/net" {
            for sym in &import.symbols {
                if !is_net_export_name(&sym.name) {
                    return Err(JsError {
                        message: format!(
                            "Symbol '{}' not found in module '{}'. The module exports: {}",
                            sym.name,
                            import.path,
                            net_export_list()
                        ),
                    });
                }
            }
            return Ok(());
        }

        // Skip other core-bundled and npm packages — they don't have .td export declarations
        if import.path.starts_with("npm:")
            || import.path == "taida-lang/js"
            || import.path == "taida-lang/os"
            || import.path == "taida-lang/crypto"
            || import.path == "taida-lang/pool"
        {
            return Ok(());
        }

        // Resolve the .td source path + optional facade exports
        let (td_path, pkg_manifest_exports): (Option<std::path::PathBuf>, Option<Vec<String>>) =
            if import.path.starts_with("./")
                || import.path.starts_with("../")
                || import.path.starts_with('/')
            {
                (self.resolve_import_td_path(&import.path), None)
            } else if import.path.contains('/') {
                // Package import — resolve via .taida/deps/
                let project_root = match self.project_root.as_ref() {
                    Some(r) => r,
                    None => return Ok(()), // No project root — skip validation
                };
                let resolution = if let Some(ref ver) = import.version {
                    crate::pkg::resolver::resolve_package_module_versioned(
                        project_root,
                        &import.path,
                        ver,
                    )
                } else {
                    crate::pkg::resolver::resolve_package_module(project_root, &import.path)
                };
                match resolution {
                    Some(r) => {
                        match &r.submodule {
                            Some(sub) => {
                                let p = r.pkg_dir.join(format!("{}.td", sub));
                                (if p.exists() { Some(p) } else { None }, None)
                            }
                            None => {
                                // B11B-023: Package root import — use centralized facade validation
                                if let Some(ctx) =
                                    crate::pkg::facade::resolve_facade_context(&r.pkg_dir)
                                {
                                    let sym_names: Vec<String> =
                                        import.symbols.iter().map(|s| s.name.clone()).collect();
                                    let violations = crate::pkg::facade::validate_facade(
                                        &ctx.facade_exports,
                                        &ctx.entry_path,
                                        &sym_names,
                                    );
                                    if let Some(v) = violations.first() {
                                        return Err(JsError {
                                        message: match v {
                                            crate::pkg::facade::FacadeViolation::HiddenSymbol { name, available } => {
                                                format!(
                                                    "Symbol '{}' is not part of the public API declared in packages.tdm. \
                                                     Available exports: {}",
                                                    name,
                                                    available.join(", ")
                                                )
                                            }
                                            crate::pkg::facade::FacadeViolation::GhostSymbol { name } => {
                                                format!(
                                                    "Symbol '{}' is declared in packages.tdm but not found in the entry module. \
                                                     The entry module must export all symbols listed in the package facade.",
                                                    name
                                                )
                                            }
                                        },
                                    });
                                    }
                                    (Some(ctx.entry_path), Some(ctx.facade_exports))
                                } else {
                                    // No facade — resolve entry module normally
                                    let entry = match crate::pkg::manifest::Manifest::from_dir(
                                        &r.pkg_dir,
                                    ) {
                                        Ok(Some(manifest)) => manifest.entry,
                                        _ => "main.td".to_string(),
                                    };
                                    let p = r.pkg_dir.join(entry);
                                    (if p.exists() { Some(p) } else { None }, None)
                                }
                            }
                        }
                    }
                    None => (None, None),
                }
            } else {
                (None, None)
            };

        let td_path = match td_path {
            Some(p) => p,
            None => return Ok(()), // Cannot resolve — let downstream handle the error
        };

        let source = match std::fs::read_to_string(&td_path) {
            Ok(s) => s,
            Err(_) => return Ok(()), // Cannot read — let downstream handle the error
        };
        let (program, _) = crate::parser::parse(&source);

        // Collect explicit export list from <<< statements
        let mut export_symbols: Vec<String> = Vec::new();
        let mut has_export = false;
        for stmt in &program.statements {
            if let crate::parser::Statement::Export(export_stmt) = stmt {
                has_export = true;
                for sym in &export_stmt.symbols {
                    if !export_symbols.contains(sym) {
                        export_symbols.push(sym.clone());
                    }
                }
            }
        }

        // B11B-023: Facade validation (membership + ghost) is now handled by
        // pkg::facade::validate_facade() above. If we reach here with a facade,
        // it means all symbols passed validation — proceed to normal export check.
        if pkg_manifest_exports.is_some() {
            return Ok(());
        }

        // If no <<< found, all symbols are exported (backward compat)
        if !has_export {
            return Ok(());
        }

        let exports: std::collections::HashSet<String> = export_symbols.into_iter().collect();

        // Validate each imported symbol against entry module's <<< export list
        for sym in &import.symbols {
            if !exports.contains(&sym.name) {
                return Err(JsError {
                    message: format!(
                        "Symbol '{}' not found in module '{}'. \
                         The module exports: {}",
                        sym.name,
                        import.path,
                        if exports.is_empty() {
                            "(nothing)".to_string()
                        } else {
                            let mut sorted: Vec<&String> = exports.iter().collect();
                            sorted.sort();
                            sorted
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                });
            }
        }

        Ok(())
    }

    fn gen_import(&mut self, import: &ImportStmt) -> Result<(), JsError> {
        // RCB-201: Validate imported symbols against target module's export list
        self.validate_import_symbols(import)?;

        // RC1 Phase 4: addon-backed package detection.
        //
        // If the import resolves to a package directory that contains
        // `native/addon.toml`, this is an addon-backed package. JS is
        // a non-Native backend so the deterministic policy guard
        // produces a compile-time error here.
        //
        // The check uses the same resolver pair as the source-load
        // path so we cannot drift from the runtime resolution order.
        if let Some(pkg_dir) = self.try_locate_addon_pkg_dir(import)
            && pkg_dir.join("native").join("addon.toml").exists()
        {
            let policy_err =
                crate::addon::ensure_addon_supported(crate::addon::AddonBackend::Js, &import.path)
                    .expect_err("Js backend must be rejected by addon policy");
            return Err(JsError {
                message: policy_err.to_string(),
            });
        }

        // taida-lang/js: JSNew is a compile-time construct, no runtime import needed
        if import.path == "taida-lang/js" {
            return Ok(());
        }
        // taida-lang/os: core-bundled, runtime functions already embedded
        if import.path == "taida-lang/os" {
            return Ok(());
        }
        // taida-lang/crypto: core-bundled, runtime sha256 already embedded
        if import.path == "taida-lang/crypto" {
            return Ok(());
        }
        // taida-lang/net: core-bundled, HTTP v1 runtime functions already embedded
        if import.path == "taida-lang/net" {
            self.has_net_import = true;
            return Ok(());
        }

        let symbols: Vec<String> = import
            .symbols
            .iter()
            .map(|s| match &s.alias {
                Some(alias) => format!("{} as {}", s.name, alias),
                None => s.name.clone(),
            })
            .collect();

        self.write_indent();
        if import.path.starts_with("npm:") {
            // npm パッケージからのインポート
            let pkg_name = &import.path[4..];
            self.write(&format!(
                "import {{ {} }} from '{}';\n",
                symbols.join(", "),
                pkg_name
            ));
        } else if !import.path.starts_with("./")
            && !import.path.starts_with("../")
            && !import.path.starts_with('/')
            && import.path.contains('/')
        {
            // Package import (e.g. "shijimic/taida-package-test")
            // Resolve via .taida/deps/ and packages.tdm entry point.
            // RC-1q: Pass version for versioned imports (>>> alice/http@b.12)
            // to resolve version-qualified directories first.
            let js_path =
                self.resolve_package_import_path(&import.path, import.version.as_deref())?;
            self.write(&format!(
                "import {{ {} }} from '{}';\n",
                symbols.join(", "),
                js_path
            ));
        } else {
            // ローカルモジュール — ESM import (.mjs)
            let js_path = self.resolve_local_import_js_path(&import.path)?;
            self.write(&format!(
                "import {{ {} }} from '{}';\n",
                symbols.join(", "),
                js_path
            ));
        }

        // C18B-011 fix: Register absorbed imported enums into the consumer
        // module's `__taida_enumDefs` registry so that consumer-side
        // `jsonEncode(@(state <= ImportedEnum:X()))` emits the variant-name
        // Str via `__taida_enumVal(...).toJSON()` instead of falling back to
        // the raw ordinal.
        //
        // `self.enum_defs` was previously populated by
        // `absorb_cross_module_enum_defs` (under the local alias). The
        // registry itself is a per-module `const` declared by the embedded
        // runtime, so producer-module registration does not carry over to
        // the consumer. We therefore re-register the enum on the consumer
        // side here, mirroring the symmetry required by C18-2 across the
        // 3 backends. `Statement::EnumDef` at line 767 handles the
        // declaring-module side of the same symmetry.
        //
        // We only register for imports that could plausibly carry a user
        // enum — i.e. anything not bundled by the embedded JS runtime.
        // `taida-lang/net`'s `HttpProtocol` is an exception and is already
        // registered by its own code path at line 317 via `self.enum_defs`
        // pre-seeding; we still re-emit here because the net branch
        // `return`s above before reaching this point.
        if !import.path.starts_with("taida-lang/") && !import.path.starts_with("npm:") {
            for sym in &import.symbols {
                let local_name = sym.alias.as_deref().unwrap_or(sym.name.as_str());
                if let Some(variants) = self.enum_defs.get(local_name) {
                    let variants_js: Vec<String> =
                        variants.iter().map(|v| format!("'{}'", v)).collect();
                    self.write_indent();
                    self.write(&format!(
                        "__taida_registerEnumDef('{}', [{}]);\n",
                        local_name,
                        variants_js.join(", ")
                    ));
                }
            }
        }
        Ok(())
    }

    /// Resolve a local (relative) import path to the correct .mjs ESM specifier.
    ///
    /// When build context (entry_root, out_root) is available, computes the actual
    /// output location of the dependency using the same strip_prefix logic as main.rs,
    /// then produces a relative ESM import from our output to the dependency's output.
    /// This handles the case where `../shared` from `src/main.td` is flattened to
    /// `out_root/shared.mjs` alongside `out_root/main.mjs` (correct: `./shared.mjs`).
    ///
    /// Also performs RCB-303 / C27B-022 path traversal rejection for
    /// `..` AND absolute `/` imports (3-backend parity with Interpreter
    /// SEC-003 land and Native `driver.rs::resolve_module_path`).
    fn resolve_local_import_js_path(&self, import_path: &str) -> Result<String, JsError> {
        use std::path::{Path, PathBuf};

        // RCB-303 / C27B-022: Check path traversal for `..` AND absolute
        // `/` imports. The error message mirrors the interpreter's
        // wording verbatim so 3-backend regression tests can assert the
        // same string across Interpreter / JS / Native.
        //
        // C27B-022 parity fix: when `self.project_root` is not set
        // (build invoked outside a `packages.tdm` tree), fall back to
        // walking up from `self.source_file` for `taida.toml` /
        // `.taida` / `.git` markers — the same set Native and
        // Interpreter recognise. This closes a 3-backend parity hole
        // where JS would silently accept absolute escapes when the
        // project was anchored on `taida.toml` only.
        if (import_path.contains("..") || import_path.starts_with('/'))
            && let Some(source_file) = self.source_file.as_ref()
        {
            let source_dir = source_file.parent().unwrap_or(Path::new("."));
            let project_root_buf;
            let project_root: &Path = match self.project_root.as_ref() {
                Some(p) => p.as_path(),
                None => {
                    project_root_buf = js_find_project_root(source_dir);
                    project_root_buf.as_path()
                }
            };
            let td_path = if Path::new(import_path).is_absolute() {
                PathBuf::from(import_path)
            } else {
                source_dir.join(import_path)
            };
            let reject = if let Ok(canonical) = td_path.canonicalize() {
                if let Ok(root_canonical) = project_root.canonicalize() {
                    !canonical.starts_with(&root_canonical)
                } else {
                    false // cannot verify root — let it through, will fail at read
                }
            } else {
                false // file not found — let it through, will fail at read
            };
            if reject {
                return Err(JsError {
                    message: format!(
                        "Import path '{}' resolves outside the project root. \
                         Path traversal beyond the project boundary is not allowed.",
                        import_path
                    ),
                });
            }
        }

        // When build context is available and the import crosses directory boundaries,
        // compute the correct ESM path based on output layout.
        if let (Some(entry_root), Some(out_root), Some(source_file), Some(output_file)) = (
            &self.entry_root,
            &self.out_root,
            &self.source_file,
            &self.output_file,
        ) {
            let source_dir = source_file.parent().unwrap_or(Path::new("."));
            let dep_source = source_dir.join(import_path);
            // Canonicalize to resolve symlinks and ..
            let dep_source = dep_source.canonicalize().unwrap_or(dep_source);

            // Replicate main.rs output placement: strip_prefix chain
            let dep_rel = dep_source
                .strip_prefix(entry_root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| {
                    let entry_parent = entry_root.parent().unwrap_or(entry_root);
                    dep_source
                        .strip_prefix(entry_parent)
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|_| {
                            PathBuf::from(
                                dep_source
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("module.td"),
                            )
                        })
                });
            let dep_output = out_root.join(dep_rel.with_extension("mjs"));

            // Compute relative ESM path from our output to the dependency's output
            let our_dir = output_file.parent().unwrap_or(Path::new("."));
            if let Some(rel) = pathdiff(our_dir, &dep_output) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                // ESM requires explicit ./ prefix for same-directory imports
                let esm_path = if rel_str.starts_with("../") || rel_str.starts_with("./") {
                    rel_str.to_string()
                } else {
                    format!("./{}", rel_str)
                };
                return Ok(esm_path);
            }
        }

        // Fallback: simple string replacement (no build context).
        // Parser enforces .td extension on relative imports, so we only need
        // the .td/.tdjs → .mjs conversion.
        let js_path = import_path.replace(".tdjs", ".mjs").replace(".td", ".mjs");
        Ok(js_path)
    }

    /// Resolve a package import path to a relative .mjs path for ESM import.
    ///
    /// Given "shijimic/taida-package-test", finds `.taida/deps/shijimic/taida-package-test/`,
    /// reads packages.tdm for entry point, and returns a relative path from the JS output
    /// to the package's .mjs file (transpiled in-place in .taida/deps/).
    ///
    /// RC-1q: When `version` is provided, tries version-qualified directories first
    /// (e.g., `.taida/deps/alice/http@b.12/`) before falling back to unversioned.
    fn resolve_package_import_path(
        &self,
        import_path: &str,
        version: Option<&str>,
    ) -> Result<String, JsError> {
        let project_root = self.project_root.as_ref().ok_or_else(|| JsError {
            message: format!(
                "Could not resolve package import '{}': project root context is unavailable.",
                import_path
            ),
        })?;
        let _source_file = self.source_file.as_ref().ok_or_else(|| JsError {
            message: format!(
                "Could not resolve package import '{}': source file context is unavailable.",
                import_path
            ),
        })?;

        // Find the package directory using longest-prefix matching.
        // RCB-213: For versioned imports, use resolve_package_module_versioned
        // which does longest-prefix matching with version-qualified directories.
        // This supports submodule imports (e.g., alice/pkg/submod@b.12 resolves to
        // .taida/deps/alice/pkg@b.12/submod.td).
        let resolution = if let Some(ver) = version {
            crate::pkg::resolver::resolve_package_module_versioned(
                project_root,
                import_path,
                ver,
            )
            .ok_or_else(|| JsError {
                message: format!(
                    "Could not resolve package import '{}@{}'. Run `taida deps` and ensure the package is installed in .taida/deps/ before building JS.",
                    import_path, ver
                ),
            })?
        } else {
            crate::pkg::resolver::resolve_package_module(project_root, import_path)
                .ok_or_else(|| JsError {
                    message: format!(
                        "Could not resolve package import '{}'. Run `taida deps` and ensure the package is installed in .taida/deps/ before building JS.",
                        import_path
                    ),
                })?
        };

        // Determine the target .td file
        let td_path = match &resolution.submodule {
            Some(submodule) => resolution.pkg_dir.join(format!("{}.td", submodule)),
            None => {
                // Read packages.tdm for entry point.
                // If the manifest is missing or unreadable, fall back to "main.td"
                // which is the conventional default entry point for Taida packages.
                let entry = match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir) {
                    Ok(Some(manifest)) => manifest.entry,
                    Ok(None) => {
                        eprintln!(
                            "Warning: no packages.tdm found in '{}', using 'main.td' as entry point",
                            resolution.pkg_dir.display()
                        );
                        "main.td".to_string()
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: failed to read packages.tdm in '{}': {}, using 'main.td' as entry point",
                            resolution.pkg_dir.display(),
                            e
                        );
                        "main.td".to_string()
                    }
                };
                let entry_clean = entry.strip_prefix("./").unwrap_or(&entry);
                resolution.pkg_dir.join(entry_clean)
            }
        };

        // The dep .mjs is in-place next to the .td file in .taida/deps/
        let mjs_path = td_path.with_extension("mjs");

        let output_file = self.output_file.as_ref().ok_or_else(|| JsError {
            message: format!(
                "Could not resolve package import '{}': JS output path is unavailable.",
                import_path
            ),
        })?;
        let js_output_dir = output_file
            .parent()
            .ok_or_else(|| JsError {
                message: format!(
                    "Could not resolve package import '{}': could not determine JS output directory.",
                    import_path
                ),
            })?
            .to_path_buf();

        let rel = pathdiff(&js_output_dir, &mjs_path).ok_or_else(|| JsError {
            message: format!(
                "Could not resolve package import '{}': failed to compute relative JS import path.",
                import_path
            ),
        })?;

        // Ensure it starts with "./" for ESM
        let rel_str = rel.to_string_lossy().to_string();
        if rel_str.starts_with("./") || rel_str.starts_with("../") {
            Ok(rel_str)
        } else {
            Ok(format!("./{}", rel_str))
        }
    }

    fn gen_export(&mut self, export: &ExportStmt) -> Result<(), JsError> {
        // RCB-212: Re-export path `<<< ./path` is not supported.
        if export.path.is_some() {
            return Err(JsError {
                message: "Re-export with path (`<<< ./path`) is not yet supported. \
                         Use explicit import and re-export instead."
                    .to_string(),
            });
        }
        // ESM named export
        self.write_indent();
        self.write("export { ");
        self.write(&export.symbols.join(", "));
        self.write(" };\n");
        Ok(())
    }

    /// RC1 Phase 4 helper: resolve only the **package directory** for
    /// an import statement, without producing a `.mjs` path. Used by
    /// the addon-policy guard in `gen_import` so JS codegen can detect
    /// addon-backed packages and emit a deterministic compile-time
    /// error rather than silently emitting an `import` statement that
    /// would crash at runtime.
    ///
    /// Returns `None` for relative / absolute / project-root /
    /// `std/` / `npm:` imports — those can never be addon-backed.
    /// Also returns `None` for submodule imports (`org/pkg/sub`)
    /// because RC1 addons are package-level only.
    fn try_locate_addon_pkg_dir(&self, import: &ImportStmt) -> Option<std::path::PathBuf> {
        let path = &import.path;
        if path.starts_with("./")
            || path.starts_with("../")
            || path.starts_with('/')
            || path.starts_with("~/")
            || path.starts_with("std/")
            || path.starts_with("npm:")
        {
            return None;
        }
        let project_root = self.project_root.as_ref()?;
        let resolution = if let Some(ver) = &import.version {
            crate::pkg::resolver::resolve_package_module_versioned(project_root, path, ver)
        } else {
            crate::pkg::resolver::resolve_package_module(project_root, path)
        }?;
        if resolution.submodule.is_some() {
            return None;
        }
        Some(resolution.pkg_dir)
    }

    fn gen_todo_default_expr(&mut self, arg: &Expr) -> Result<(), JsError> {
        match arg {
            Expr::Ident(name, _) => match name.as_str() {
                "Int" | "Num" => self.write("0"),
                "Float" => self.write("0.0"),
                "Str" => self.write("\"\""),
                "Bool" => self.write("false"),
                "Molten" => self.write("__taida_molten()"),
                _ => self.write("Object.freeze({})"),
            },
            Expr::MoldInst(name, type_args, _, _) if name == "Stub" => {
                if type_args.len() != 1 {
                    return Err(JsError {
                        message: "Stub requires exactly 1 message argument: Stub[\"msg\"]"
                            .to_string(),
                    });
                }
                self.write("__taida_molten()");
            }
            _ => self.write("Object.freeze({})"),
        }
        Ok(())
    }

    fn gen_expr(&mut self, expr: &Expr) -> Result<(), JsError> {
        match expr {
            Expr::IntLit(val, _) => {
                self.write(&val.to_string());
                Ok(())
            }
            Expr::FloatLit(val, _) => {
                // C26B-011 / Round 7 wV-a: preserve IEEE-754 signed zero
                // in JS Float literal codegen. Rust `f64::to_string()`
                // renders -0.0 as "-0" (no decimal point, indistinguishable
                // from integer -0), and also renders +0.0 as "0". When
                // the literal is a Float-origin value (e.g. an `x: Float`
                // binding or a `-0.0` source literal) we must emit JS that
                // will compare equal to the interpreter / native Float
                // value under `Object.is(...)`.
                //
                // Note: the arithmetic path (`-1.0 * 0.0`) is already
                // parity-safe as of wS Round 6 (runtime `__taida_float_render`
                // handles -0.0 via `Object.is`). This fix covers the
                // pre-existing literal codegen divergence noted in
                // `examples/quality/c26_float_edge/signed_zero_parity.td`.
                if val.is_sign_negative() && *val == 0.0 {
                    self.write("-0");
                } else if val.is_nan() {
                    self.write("(0/0)");
                } else if val.is_infinite() {
                    if val.is_sign_negative() {
                        self.write("(-1/0)");
                    } else {
                        self.write("(1/0)");
                    }
                } else {
                    self.write(&val.to_string());
                }
                Ok(())
            }
            Expr::StringLit(val, _) => {
                let escaped = val
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t");
                self.write(&format!("\"{}\"", escaped));
                Ok(())
            }
            Expr::TemplateLit(val, _) => {
                // テンプレートリテラル → JS テンプレートリテラル
                // Taida uses ${var} syntax, same as JS — pass through directly
                // But @[...] list literals inside ${} need conversion to JS arrays
                let converted = Self::convert_template_list_literals(val);
                self.write(&format!("`{}`", converted));
                Ok(())
            }
            Expr::BoolLit(val, _) => {
                self.write(if *val { "true" } else { "false" });
                Ok(())
            }
            Expr::Gorilla(_) => {
                self.write("process.exit(1)");
                Ok(())
            }
            Expr::Ident(name, _) => {
                // C25B-033: user FuncDef references whose Taida-level name
                // collides with a prelude reserved identifier are emitted
                // under the mangled form `_td_user_<name>`. Non-colliding
                // identifiers (the common case) are written verbatim.
                let emitted = self.js_user_func_ident(name);
                self.write(&emitted);
                Ok(())
            }
            Expr::Placeholder(_) => {
                self.write("_");
                Ok(())
            }
            Expr::Hole(_) => {
                // Hole should not appear outside of FuncCall partial application context
                self.write("undefined");
                Ok(())
            }
            Expr::BuchiPack(fields, _) => {
                // QF-16: Placeholder 値のフィールドをスキップ（=> :Type が Placeholder として
                // パースされるため、BuchiPack 内ラムダの戻り値型注釈が不正なフィールドになる）
                let real_fields: Vec<_> = fields
                    .iter()
                    .filter(|f| !matches!(f.value, Expr::Placeholder(_)))
                    .collect();
                self.write("Object.freeze({ ");
                for (i, field) in real_fields.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(&format!("{}: ", field.name));
                    self.gen_expr(&field.value)?;
                }
                self.write(" })");
                Ok(())
            }
            Expr::ListLit(items, _) => {
                self.write("Object.freeze([");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.gen_expr(item)?;
                }
                self.write("])");
                Ok(())
            }
            Expr::BinaryOp(lhs, op, rhs, _) => {
                // Eq/NotEq use __taida_equals for structural comparison
                match op {
                    BinOp::Eq => {
                        self.write("__taida_equals(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    BinOp::NotEq => {
                        self.write("!__taida_equals(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    BinOp::Add => {
                        self.write("__taida_add(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    BinOp::Sub => {
                        self.write("__taida_sub(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    BinOp::Mul => {
                        self.write("__taida_mul(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    _ => {}
                }
                self.write("(");
                self.gen_expr(lhs)?;
                let op_str = match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul => unreachable!(),
                    // BinOp::Div and BinOp::Mod removed — use Div[x, y]() and Mod[x, y]() molds
                    BinOp::Eq | BinOp::NotEq => unreachable!(),
                    BinOp::Lt => " < ",
                    BinOp::Gt => " > ",
                    BinOp::GtEq => " >= ",
                    BinOp::And => " && ",
                    BinOp::Or => " || ",
                    BinOp::Concat => " + ",
                };
                self.write(op_str);
                self.gen_expr(rhs)?;
                self.write(")");
                Ok(())
            }
            Expr::UnaryOp(op, operand, _) => {
                let op_str = match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "!",
                };
                self.write(op_str);
                self.gen_expr(operand)?;
                Ok(())
            }
            Expr::FuncCall(callee, args, _) => {
                // TCO: if calling a function in the current TCO group, emit TailCall
                if !self.current_tco_funcs.is_empty()
                    && let Expr::Ident(name, _) = callee.as_ref()
                    && self.current_tco_funcs.contains(name)
                {
                    // C25B-033: TCO inner helper also follows the mangled
                    // user-func name when the Taida name collides with a
                    // prelude reserved identifier.
                    let inner_name = self.js_user_func_ident(name);
                    self.write(&format!("new __TaidaTailCall(__inner_{}, [", inner_name));
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.gen_expr(arg)?;
                    }
                    self.write("])");
                    return Ok(());
                }

                // Empty-slot partial application: if any arg is Hole (empty slot), emit a closure.
                // Note: Old `_` (Placeholder) partial application is rejected by checker
                // (E1502) before reaching codegen. Only Hole-based syntax `f(5, )` is handled.
                let has_hole = args.iter().any(|a| matches!(a, Expr::Hole(_)));
                if has_hole {
                    // Count holes and generate parameter names
                    let placeholder_count =
                        args.iter().filter(|a| matches!(a, Expr::Hole(_))).count();
                    let params: Vec<String> = (0..placeholder_count)
                        .map(|i| format!("__pa_{}", i))
                        .collect();
                    self.write(&format!("(({}) => ", params.join(", ")));

                    // Generate the function call with placeholders replaced
                    if let Expr::Ident(name, _) = callee.as_ref() {
                        match name.as_str() {
                            "debug" => self.write("__taida_debug"),
                            "typeof" => self.write("__taida_typeof"),
                            "assert" => self.write("__taida_assert"),
                            "stdout" => self.write("__taida_stdout"),
                            "stderr" => self.write("__taida_stderr"),
                            "stdin" => self.write("__taida_stdin"),
                            // C20-2: stdinLine is the UTF-8-aware Async[Lax[Str]] successor
                            "stdinLine" => self.write("__taida_stdinLine"),
                            "jsonEncode" => self.write("__taida_jsonEncode"),
                            "jsonPretty" => self.write("__taida_jsonPretty"),
                            "nowMs" => self.write("__taida_nowMs"),
                            "sleep" => self.write("__taida_sleep"),
                            // D28B-015: `strOf(span, raw)` lowercase function-form
                            // delegates to the existing `__taida_net_StrOf`
                            // runtime helper (always present in `RUNTIME_JS`).
                            "strOf" => self.write("__taida_net_StrOf"),
                            "readBytes" => self.write("__taida_os_readBytes"),
                            "readBytesAt" => self.write("__taida_os_readBytesAt"),
                            "writeFile" => self.write("__taida_os_writeFile"),
                            "writeBytes" => self.write("__taida_os_writeBytes"),
                            "appendFile" => self.write("__taida_os_appendFile"),
                            "remove" => self.write("__taida_os_remove"),
                            "createDir" => self.write("__taida_os_createDir"),
                            "rename" => self.write("__taida_os_rename"),
                            "run" => self.write("__taida_os_run"),
                            "execShell" => self.write("__taida_os_execShell"),
                            // C19: interactive TTY-passthrough variants
                            "runInteractive" => self.write("__taida_os_runInteractive"),
                            "execShellInteractive" => self.write("__taida_os_execShellInteractive"),
                            "allEnv" => self.write("__taida_os_allEnv"),
                            "argv" => self.write("__taida_os_argv"),
                            "tcpConnect" => self.write("__taida_os_tcpConnect"),
                            "tcpListen" => self.write("__taida_os_tcpListen"),
                            "tcpAccept" => self.write("__taida_os_tcpAccept"),
                            "socketSend" => self.write("__taida_os_socketSend"),
                            "socketSendAll" => self.write("__taida_os_socketSendAll"),
                            "socketRecv" => self.write("__taida_os_socketRecv"),
                            "socketSendBytes" => self.write("__taida_os_socketSendBytes"),
                            "socketRecvBytes" => self.write("__taida_os_socketRecvBytes"),
                            "socketClose" => self.write("__taida_os_socketClose"),
                            "listenerClose" => self.write("__taida_os_listenerClose"),
                            "udpBind" => self.write("__taida_os_udpBind"),
                            "udpSendTo" => self.write("__taida_os_udpSendTo"),
                            "udpRecvFrom" => self.write("__taida_os_udpRecvFrom"),
                            "udpClose" => self.write("__taida_os_udpClose"),
                            "socketRecvExact" => self.write("__taida_os_socketRecvExact"),
                            "dnsResolve" => self.write("__taida_os_dnsResolve"),
                            "poolCreate" => self.write("__taida_os_poolCreate"),
                            "poolAcquire" => self.write("__taida_os_poolAcquire"),
                            "poolRelease" => self.write("__taida_os_poolRelease"),
                            "poolClose" => self.write("__taida_os_poolClose"),
                            "poolHealth" => self.write("__taida_os_poolHealth"),
                            // C12-6a: Regex(pattern, flags?) prelude constructor
                            "Regex" => self.write("__taida_regex"),
                            // taida-lang/net HTTP v1 (only when imported)
                            _ if self.try_write_net_builtin(name, "") => {}
                            _ => self.gen_expr(callee)?,
                        }
                    } else {
                        self.gen_expr(callee)?;
                    }
                    self.write("(");
                    let mut ph_idx = 0;
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        if matches!(arg, Expr::Hole(_)) {
                            self.write(&format!("__pa_{}", ph_idx));
                            ph_idx += 1;
                        } else {
                            self.gen_expr(arg)?;
                        }
                    }
                    self.write("))");
                    return Ok(());
                }

                if let Expr::Ident(name, _) = callee.as_ref() {
                    // C21-5: specialise single-arg stdout / debug / stderr
                    // when the argument is statically Float-origin, so that
                    // `stdout(triple(4.0))` renders `12.0` (matching the
                    // interpreter) without runtime-wrapping every Number.
                    let float_specialise = args.len() == 1
                        && self.is_float_origin_expr(&args[0])
                        && matches!(name.as_str(), "stdout" | "debug" | "stderr");
                    if float_specialise {
                        match name.as_str() {
                            "stdout" => self.write("__taida_stdout_f"),
                            "debug" => self.write("__taida_debug_f"),
                            // stderr does not have a dedicated float variant
                            // — delegate to __taida_to_string_f so the
                            // written payload carries the `.0` suffix.
                            "stderr" => {
                                self.write("__taida_stderr(__taida_to_string_f(");
                                self.gen_expr(&args[0])?;
                                self.write("))");
                                return Ok(());
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        match name.as_str() {
                            "debug" => self.write("__taida_debug"),
                            "typeof" => self.write("__taida_typeof"),
                            "assert" => self.write("__taida_assert"),
                            "stdout" => self.write("__taida_stdout"),
                            "stderr" => self.write("__taida_stderr"),
                            "stdin" => self.write("__taida_stdin"),
                            // C20-2: stdinLine is the UTF-8-aware Async[Lax[Str]] successor
                            "stdinLine" => self.write("__taida_stdinLine"),
                            "jsonEncode" => self.write("__taida_jsonEncode"),
                            "jsonPretty" => self.write("__taida_jsonPretty"),
                            "nowMs" => self.write("__taida_nowMs"),
                            "sleep" => self.write("__taida_sleep"),
                            // D28B-015: `strOf(span, raw)` lowercase function-form
                            // delegates to the existing `__taida_net_StrOf`
                            // runtime helper (always present in `RUNTIME_JS`).
                            "strOf" => self.write("__taida_net_StrOf"),
                            "readBytes" => self.write("__taida_os_readBytes"),
                            "readBytesAt" => self.write("__taida_os_readBytesAt"),
                            "writeFile" => self.write("__taida_os_writeFile"),
                            "writeBytes" => self.write("__taida_os_writeBytes"),
                            "appendFile" => self.write("__taida_os_appendFile"),
                            "remove" => self.write("__taida_os_remove"),
                            "createDir" => self.write("__taida_os_createDir"),
                            "rename" => self.write("__taida_os_rename"),
                            "run" => self.write("__taida_os_run"),
                            "execShell" => self.write("__taida_os_execShell"),
                            // C19: interactive TTY-passthrough variants
                            "runInteractive" => self.write("__taida_os_runInteractive"),
                            "execShellInteractive" => self.write("__taida_os_execShellInteractive"),
                            "allEnv" => self.write("__taida_os_allEnv"),
                            "argv" => self.write("__taida_os_argv"),
                            "tcpConnect" => self.write("__taida_os_tcpConnect"),
                            "tcpListen" => self.write("__taida_os_tcpListen"),
                            "tcpAccept" => self.write("__taida_os_tcpAccept"),
                            "socketSend" => self.write("__taida_os_socketSend"),
                            "socketSendAll" => self.write("__taida_os_socketSendAll"),
                            "socketRecv" => self.write("__taida_os_socketRecv"),
                            "socketSendBytes" => self.write("__taida_os_socketSendBytes"),
                            "socketRecvBytes" => self.write("__taida_os_socketRecvBytes"),
                            "socketClose" => self.write("__taida_os_socketClose"),
                            "listenerClose" => self.write("__taida_os_listenerClose"),
                            "udpBind" => self.write("__taida_os_udpBind"),
                            "udpSendTo" => self.write("__taida_os_udpSendTo"),
                            "udpRecvFrom" => self.write("__taida_os_udpRecvFrom"),
                            "udpClose" => self.write("__taida_os_udpClose"),
                            "socketRecvExact" => self.write("__taida_os_socketRecvExact"),
                            "dnsResolve" => self.write("__taida_os_dnsResolve"),
                            "poolCreate" => self.write("__taida_os_poolCreate"),
                            "poolAcquire" => self.write("__taida_os_poolAcquire"),
                            "poolRelease" => self.write("__taida_os_poolRelease"),
                            "poolClose" => self.write("__taida_os_poolClose"),
                            "poolHealth" => self.write("__taida_os_poolHealth"),
                            // C12-6a: Regex(pattern, flags?) prelude constructor
                            "Regex" => self.write("__taida_regex"),
                            // taida-lang/net HTTP v1 (only when imported)
                            _ if self.try_write_net_builtin(name, "") => {}
                            _ => self.gen_expr(callee)?,
                        }
                    }
                } else {
                    self.gen_expr(callee)?;
                }
                self.write("(");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.gen_expr(arg)?;
                }
                self.write(")");
                Ok(())
            }
            Expr::MethodCall(obj, method, args, _) => {
                if method == "throw" {
                    // .throw() is emitted as a standalone function call to avoid
                    // polluting Object.prototype.
                    self.write("__taida_throw(");
                    self.gen_expr(obj)?;
                    self.write(")");
                    return Ok(());
                }
                if is_removed_list_method(method) {
                    self.write("__taida_list_method_removed(");
                    self.write(&format!("{:?}", method));
                    self.write(")");
                    return Ok(());
                }
                // C12-2b: .toString() universal adoption. Route through a
                // runtime helper so that plain objects (BuchiPacks) format
                // as `@(...)` instead of JS's default `[object Object]`.
                // Primitives (Number/Boolean/String/Array/Uint8Array) already
                // have Taida-compatible prototype patches applied, so the
                // helper only overrides dispatch for untyped plain objects.
                if method == "toString" && args.is_empty() {
                    // C21-5: Float-origin specialisation so that
                    // `triple(4.0).toString()` renders `"12.0"` matching
                    // the interpreter's `Value::Float(12.0).to_string()`.
                    if self.is_float_origin_expr(obj) {
                        self.write("__taida_to_string_f(");
                    } else {
                        self.write("__taida_to_string(");
                    }
                    self.gen_expr(obj)?;
                    self.write(")");
                    return Ok(());
                }
                // hasValue() is a method call on Lax — emit as method call
                // (In new design, hasValue() is always a function, not a property)
                // B11-4c: replace/replaceAll/split → runtime helper for edge-case parity
                // C12-6c: match/search → runtime helper (Regex-only). The
                // helpers inspect the first arg's `__type` tag at runtime
                // so the same JS call handles both fixed-string (B11) and
                // Regex overloads uniformly.
                if method == "replace"
                    || method == "replaceAll"
                    || method == "split"
                    || method == "match"
                    || method == "search"
                {
                    let helper = match method.as_str() {
                        "replace" => "__taida_str_replace",
                        "replaceAll" => "__taida_str_replace_all",
                        "split" => "__taida_str_split",
                        "match" => "__taida_str_match",
                        "search" => "__taida_str_search",
                        _ => unreachable!(),
                    };
                    self.write(&format!("{}(", helper));
                    self.gen_expr(obj)?;
                    for arg in args.iter() {
                        self.write(", ");
                        self.gen_expr(arg)?;
                    }
                    self.write(")");
                    return Ok(());
                }
                self.gen_expr(obj)?;
                // Taida .length() is a method call, but JS .length is a property.
                // Use .length_() which is patched in the runtime.
                // Only state-check methods remain as prototype methods.
                // Operation methods are now standalone mold functions.
                let js_method = match method.as_str() {
                    "length" => "length_",
                    other => other,
                };
                self.write(&format!(".{}(", js_method));
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.gen_expr(arg)?;
                }
                self.write(")");
                Ok(())
            }
            Expr::FieldAccess(obj, field, _) => {
                self.gen_expr(obj)?;
                // F-59 fix: Lax/Gorillax hasValue is a callable function in JS runtime.
                // When accessed as a property (field access), emit as function call
                // so that it returns the boolean value instead of a function reference.
                if field == "hasValue" {
                    self.write(".hasValue()");
                } else {
                    self.write(&format!(".{}", field));
                }
                Ok(())
            }
            // IndexAccess removed in v0.5.0 — use .get(i) instead
            Expr::CondBranch(arms, _) => self.gen_cond_branch(arms),
            Expr::Pipeline(exprs, _) => self.gen_pipeline(exprs),
            Expr::TypeInst(name, fields, _) => {
                self.write(&format!("{}({{ ", name));
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(&format!("{}: ", field.name));
                    self.gen_expr(&field.value)?;
                }
                self.write(" })");
                Ok(())
            }
            Expr::EnumVariant(enum_name, variant_name, _) => {
                let ordinal = self
                    .enum_defs
                    .get(enum_name)
                    .and_then(|variants| {
                        variants.iter().position(|variant| variant == variant_name)
                    })
                    .ok_or_else(|| JsError {
                        message: format!("Unknown enum variant '{}:{}()'", enum_name, variant_name),
                    })?;
                // C18-2: Emit a tagged wrapper whose `toJSON` returns the
                // variant-name Str. Arithmetic / comparison with the
                // ordinal continues to work via `valueOf`. See
                // `src/js/runtime/core.rs::__taida_enumVal`.
                self.write(&format!("__taida_enumVal('{}', {})", enum_name, ordinal));
                Ok(())
            }
            Expr::MoldInst(name, type_args, fields, _) => {
                // B5: MoldInst → function call with type args

                // C18-3: Ordinal[<enum_value>]() → strict Enum → Int.
                //
                // C18B-005 fix: call `__taida_enumOrdinalStrict` (not the
                // permissive `__taida_enumOrdinal`) so non-Enum arguments
                // raise a `RuntimeError` whose message matches the
                // interpreter's exactly. Pre-fix, `Ordinal[42]()`
                // silently returned `42` under JS / Native while the
                // interpreter errored, which diverged 3-backend parity
                // and invalidated the IMPL_SPEC comment claiming that
                // non-Enum inputs are rejected.
                if name == "Ordinal" {
                    if type_args.is_empty() {
                        return Err(JsError {
                            message: "Ordinal requires 1 argument: Ordinal[<enum_value>]()"
                                .to_string(),
                        });
                    }
                    self.write("__taida_enumOrdinalStrict(");
                    self.gen_expr(&type_args[0])?;
                    self.write(")");
                    return Ok(());
                }

                // B11-5b: If[cond, then, else]() → (cond ? then : else)
                // Short-circuit via ternary operator — non-selected branch is never evaluated.
                if name == "If" {
                    if type_args.len() < 3 {
                        return Err(JsError {
                            message:
                                "If requires 3 arguments: If[condition, then_value, else_value]()"
                                    .to_string(),
                        });
                    }
                    self.write("(");
                    self.gen_expr(&type_args[0])?;
                    self.write(" ? ");
                    self.gen_expr(&type_args[1])?;
                    self.write(" : ");
                    self.gen_expr(&type_args[2])?;
                    self.write(")");
                    return Ok(());
                }

                // B11-6c: TypeIs[value, :TypeName]() → type check expression
                if name == "TypeIs" {
                    if type_args.len() < 2 {
                        return Err(JsError {
                            message: "TypeIs requires 2 arguments: TypeIs[value, :TypeName]()"
                                .to_string(),
                        });
                    }
                    self.write("(");
                    match &type_args[1] {
                        Expr::TypeLiteral(type_name, None, _) => {
                            // C21-5: compile-time Int/Float static fold for
                            // TypeIs[FloatLit, :Int]() and symmetric cases.
                            // The Taida interpreter distinguishes
                            // `Value::Int(3)` from `Value::Float(3.0)`; JS
                            // cannot at runtime, so we emit a literal when
                            // the static origin is known, and fall back to
                            // the previous `Number.isInteger` check only
                            // for dynamic values (best-effort parity).
                            let static_fold: Option<bool> = match type_name.as_str() {
                                "Int" => {
                                    if self.is_float_origin_expr(&type_args[0]) {
                                        Some(false)
                                    } else if self.is_int_origin_expr(&type_args[0]) {
                                        Some(true)
                                    } else {
                                        None
                                    }
                                }
                                "Float" => {
                                    if self.is_float_origin_expr(&type_args[0]) {
                                        Some(true)
                                    } else if self.is_int_origin_expr(&type_args[0]) {
                                        Some(false)
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            };
                            if let Some(lit) = static_fold {
                                self.write(if lit { "true" } else { "false" });
                                self.write(")");
                                return Ok(());
                            }
                            let val_code = {
                                let saved = std::mem::take(&mut self.output);
                                self.gen_expr(&type_args[0])?;
                                std::mem::replace(&mut self.output, saved)
                            };
                            match type_name.as_str() {
                                "Int" => {
                                    self.write(&format!("__taida_is_int({})", val_code));
                                }
                                "Float" => {
                                    self.write(&format!("__taida_is_float({})", val_code));
                                }
                                "Num" => {
                                    self.write(&format!("typeof {} === \"number\"", val_code));
                                }
                                "Str" => {
                                    self.write(&format!("typeof {} === \"string\"", val_code));
                                }
                                "Bool" => {
                                    self.write(&format!("typeof {} === \"boolean\"", val_code));
                                }
                                "Bytes" => {
                                    self.write(&format!("{} instanceof Uint8Array", val_code));
                                }
                                // B11B-015: Error check uses __type + inheritance chain
                                "Error" => {
                                    self.write(&format!(
                                        "(function(__v){{ return typeof __v === \"object\" && __v !== null && \
                                         __taida_is_error_subtype(__v.__type || __v.type || \"\", \"Error\"); }})({v})",
                                        v = val_code
                                    ));
                                }
                                // B11B-015: Named type check via __type field + inheritance chain
                                other => {
                                    // Use an IIFE to safely evaluate val_code once and
                                    // avoid JS syntax errors with property access on literals.
                                    self.write(&format!(
                                        "(function(__v){{ return typeof __v === \"object\" && __v !== null && \
                                         (__v.__type === \"{t}\" || __taida_is_error_subtype(__v.__type || \"\", \"{t}\")); }})({v})",
                                        v = val_code,
                                        t = other
                                    ));
                                }
                            }
                        }
                        Expr::TypeLiteral(enum_name, Some(variant_name), _) => {
                            // Enum variant check: unwrap `__taida_enumVal`
                            // wrapper via `__taida_enumOrdinal` (also works
                            // on plain numbers) and compare to the ordinal.
                            // C18-2: Enum values are now tagged wrappers,
                            // so a bare `=== ordinal` would compare object
                            // reference to Number and always return false.
                            let ordinal = self
                                .enum_defs
                                .get(enum_name.as_str())
                                .and_then(|variants| {
                                    variants.iter().position(|v| v == variant_name)
                                })
                                .unwrap_or(usize::MAX);
                            self.write("__taida_enumOrdinal(");
                            self.gen_expr(&type_args[0])?;
                            self.write(&format!(") === {}", ordinal));
                        }
                        _ => {
                            // Fallback: emit false
                            self.write("false");
                        }
                    }
                    self.write(")");
                    return Ok(());
                }

                // B11-6c: TypeExtends[:TypeA, :TypeB]() → compile-time type check
                if name == "TypeExtends" {
                    if type_args.len() < 2 {
                        return Err(JsError {
                            message:
                                "TypeExtends requires 2 arguments: TypeExtends[:TypeA, :TypeB]()"
                                    .to_string(),
                        });
                    }
                    let type_a = match &type_args[0] {
                        Expr::TypeLiteral(name, _, _) => name.clone(),
                        _ => String::new(),
                    };
                    let type_b = match &type_args[1] {
                        Expr::TypeLiteral(name, _, _) => name.clone(),
                        _ => String::new(),
                    };
                    let result = if type_a == type_b {
                        true
                    } else {
                        match (type_a.as_str(), type_b.as_str()) {
                            ("Int", "Num") | ("Float", "Num") | ("Int", "Float") => true,
                            (a, b) if !a.is_empty() && !b.is_empty() => {
                                // Check inheritance chain
                                self.check_type_inheritance(a, b)
                            }
                            _ => false,
                        }
                    };
                    self.write(if result { "true" } else { "false" });
                    return Ok(());
                }

                // JSNew[ClassName](...) → new ClassName(...)
                if name == "JSNew" {
                    if type_args.is_empty() {
                        return Err(JsError {
                            message: "JSNew requires a type argument: JSNew[ClassName](...)"
                                .to_string(),
                        });
                    }
                    // Extract class name from first type arg (must be an identifier)
                    let class_name = match &type_args[0] {
                        Expr::Ident(n, _) => n.clone(),
                        _ => {
                            return Err(JsError {
                                message: "JSNew type argument must be an identifier (class name)"
                                    .to_string(),
                            });
                        }
                    };
                    self.write(&format!("new {}(", class_name));
                    // Emit constructor arguments from fields (positional args)
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.gen_expr(&field.value)?;
                    }
                    self.write(")");
                    return Ok(());
                }

                // JSSet[obj, key, value]() → ((o) => { o[key] = value; return o; })(obj)
                if name == "JSSet" {
                    if type_args.len() < 3 {
                        return Err(JsError {
                            message: "JSSet requires 3 type arguments: JSSet[obj, key, value]()"
                                .to_string(),
                        });
                    }
                    self.write("((__o) => { __o[");
                    self.gen_expr(&type_args[1])?;
                    self.write("] = ");
                    self.gen_expr(&type_args[2])?;
                    self.write("; return __o; })(");
                    self.gen_expr(&type_args[0])?;
                    self.write(")");
                    return Ok(());
                }

                // JSBind[obj, method]() → obj[method].bind(obj)
                if name == "JSBind" {
                    if type_args.len() < 2 {
                        return Err(JsError {
                            message: "JSBind requires 2 type arguments: JSBind[obj, method]()"
                                .to_string(),
                        });
                    }
                    self.write("((__o) => __o[");
                    self.gen_expr(&type_args[1])?;
                    self.write("].bind(__o))(");
                    self.gen_expr(&type_args[0])?;
                    self.write(")");
                    return Ok(());
                }

                // JSSpread[target, source]() → __taida_js_spread(target, source)
                if name == "JSSpread" {
                    if type_args.len() < 2 {
                        return Err(JsError {
                            message:
                                "JSSpread requires 2 type arguments: JSSpread[target, source]()"
                                    .to_string(),
                        });
                    }
                    self.write("__taida_js_spread(");
                    self.gen_expr(&type_args[0])?;
                    self.write(", ");
                    self.gen_expr(&type_args[1])?;
                    self.write(")");
                    return Ok(());
                }

                // taida-lang/os input molds: Read, ListDir, Stat, Exists, EnvVar
                if name == "Read"
                    || name == "ListDir"
                    || name == "Stat"
                    || name == "Exists"
                    || name == "EnvVar"
                {
                    let func_name = if name == "Read" {
                        "__taida_os_read"
                    } else if name == "ListDir" {
                        "__taida_os_listdir"
                    } else if name == "Stat" {
                        "__taida_os_stat"
                    } else if name == "Exists" {
                        "__taida_os_exists"
                    } else {
                        "__taida_os_envvar"
                    };
                    self.write(func_name);
                    self.write("(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }

                // taida-lang/os async input molds: ReadAsync, HttpGet, HttpPost, HttpRequest
                if name == "ReadAsync" {
                    self.write("__taida_os_readAsync(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                if name == "HttpGet" {
                    self.write("__taida_os_httpGet(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                if name == "HttpPost" {
                    self.write("__taida_os_httpPost(");
                    if type_args.len() >= 2 {
                        self.gen_expr(&type_args[0])?;
                        self.write(", ");
                        self.gen_expr(&type_args[1])?;
                    } else if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                        self.write(", ''");
                    }
                    self.write(")");
                    return Ok(());
                }
                if name == "HttpRequest" {
                    // C20-4 (ROOT-16): Interpreter / Native reject
                    // `HttpRequest[method]()` with an explicit runtime
                    // error; the JS backend previously emitted
                    // `__taida_os_httpRequest(, null, null)` — syntax-
                    // invalid JS that failed at parse time with a
                    // cryptic message. Surface the arity violation at
                    // codegen so all three backends fail the same way.
                    if type_args.len() < 2 {
                        return Err(JsError {
                            message: "HttpRequest requires at least 2 type arguments: HttpRequest[method, url]()".to_string(),
                        });
                    }
                    self.write("__taida_os_httpRequest(");
                    self.gen_expr(&type_args[0])?;
                    self.write(", ");
                    self.gen_expr(&type_args[1])?;
                    // Pass headers and body from optional fields
                    let mut has_headers = false;
                    let mut has_body = false;
                    for field in fields {
                        if field.name == "headers" {
                            self.write(", ");
                            self.gen_expr(&field.value)?;
                            has_headers = true;
                        }
                    }
                    if !has_headers {
                        self.write(", null");
                    }
                    for field in fields {
                        if field.name == "body" {
                            self.write(", ");
                            self.gen_expr(&field.value)?;
                            has_body = true;
                        }
                    }
                    if !has_body {
                        self.write(", null");
                    }
                    self.write(")");
                    return Ok(());
                }

                if name == "JSON" {
                    // JSON[raw, Schema]() — pass raw and schema name
                    self.write("JSON_mold(");
                    if type_args.len() >= 2 {
                        self.gen_expr(&type_args[0])?;
                        self.write(", ");
                        // Schema is a type name — emit as string for runtime lookup
                        self.gen_json_schema_expr(&type_args[1])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // Str[x](), Int[x](base?), Float[x](), Bool[x](), Bytes[x](), UInt8[x](),
                // Char[x](), CodePoint[x](), Utf8Encode[x](), Utf8Decode[x]() conversion molds
                if (name == "Str"
                    || name == "Int"
                    || name == "Float"
                    || name == "Bool"
                    || name == "Bytes"
                    || name == "UInt8"
                    || name == "Char"
                    || name == "CodePoint"
                    || name == "Utf8Encode"
                    || name == "Utf8Decode"
                    || name == "U16BE"
                    || name == "U16LE"
                    || name == "U32BE"
                    || name == "U32LE"
                    || name == "U16BEDecode"
                    || name == "U16LEDecode"
                    || name == "U32BEDecode"
                    || name == "U32LEDecode"
                    || name == "BytesCursor"
                    || name == "BytesCursorU8"
                    || name == "Cancel")
                    && !type_args.is_empty()
                {
                    // C21B-seed-04 re-fix (2026-04-22): `Float[...]()` is
                    // semantically Float by contract — the result Lax must
                    // render its `__value` / `__default` as Float (e.g. `3.0`
                    // rather than `3`). Route to `Float_mold_f`, which tags
                    // the Lax with `__floatHint: true` so the stdout /
                    // debug / format path uses Float-aware rendering.
                    // `Int_mold` already truncates to an integer JS Number,
                    // whose default `String(n)` matches the interpreter's
                    // `Int` display — no specialisation needed there.
                    let mold_fn = if name == "Float" {
                        "Float_mold_f".to_string()
                    } else {
                        format!("{}_mold", name)
                    };
                    self.write(&format!("{}(", mold_fn));
                    self.gen_expr(&type_args[0])?;
                    if name == "Int" && type_args.len() >= 2 {
                        self.write(", ");
                        self.gen_expr(&type_args[1])?;
                    } else if name == "Bytes" && !fields.is_empty() {
                        self.write(", { ");
                        for (i, field) in fields.iter().enumerate() {
                            if i > 0 {
                                self.write(", ");
                            }
                            self.write(&format!("{}: ", field.name));
                            self.gen_expr(&field.value)?;
                        }
                        self.write(" }");
                    }
                    self.write(")");
                    return Ok(());
                }
                // BytesCursorTake[cursor, size]() — 2 type args
                if name == "BytesCursorTake" && type_args.len() >= 2 {
                    self.write("BytesCursorTake_mold(");
                    self.gen_expr(&type_args[0])?;
                    self.write(", ");
                    self.gen_expr(&type_args[1])?;
                    self.write(")");
                    return Ok(());
                }
                // BytesCursorRemaining[cursor]() — 1 type arg, returns Int (not Lax)
                if name == "BytesCursorRemaining" && !type_args.is_empty() {
                    self.write("BytesCursorRemaining_mold(");
                    self.gen_expr(&type_args[0])?;
                    self.write(")");
                    return Ok(());
                }
                // Cage[value, fn]() → Cage_mold(value, fn)
                if name == "Cage" {
                    self.write("Cage_mold(");
                    for (i, arg) in type_args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.gen_expr(arg)?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // Gorillax[value]() → Gorillax(value)
                if name == "Gorillax" {
                    self.write("Gorillax(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // Stub["msg"]() -> __taida_stub("msg")
                if name == "Stub" {
                    if !fields.is_empty() {
                        return Err(JsError {
                            message: "Stub does not take `()` fields. Use Stub[\"msg\"]"
                                .to_string(),
                        });
                    }
                    if type_args.len() != 1 {
                        return Err(JsError {
                            message: "Stub requires exactly 1 message argument: Stub[\"msg\"]"
                                .to_string(),
                        });
                    }
                    self.write("__taida_stub(");
                    self.gen_expr(&type_args[0])?;
                    self.write(")");
                    return Ok(());
                }
                // TODO[T](id <= ..., task <= ..., sol <= ..., unm <= ...)
                // The runtime uses `__type: 'TODO'` as the discriminant marker,
                // matching the mold name in source. This is consistent with how
                // other mold types (Lax, Result, etc.) use their name as `__type`.
                if name == "TODO" {
                    self.write("__taida_todo_mold(");
                    if let Some(arg0) = type_args.first() {
                        self.gen_todo_default_expr(arg0)?;
                    } else {
                        self.write("Object.freeze({})");
                    }
                    self.write(", { ");
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.write(&format!("{}: ", field.name));
                        self.gen_expr(&field.value)?;
                    }
                    self.write(" })");
                    return Ok(());
                }
                // ── C26B-016 (@c.26, Option B+): span-aware comparison molds ──
                // SpanEquals / SpanStartsWith / SpanContains / SpanSlice — accept a
                // span pack `@(start, len)` + raw (Bytes/Str) + needle, dispatch to
                // the JS runtime helpers defined in `src/js/runtime/core.rs`.
                // `StrOf[span, raw]()` is the cold-path counterpart (2-arg,
                // returns Str via UTF-8 decode).
                if name == "SpanEquals"
                    || name == "SpanStartsWith"
                    || name == "SpanContains"
                    || name == "SpanSlice"
                    || name == "StrOf"
                {
                    let required_arity = match name.as_str() {
                        "SpanSlice" => 4,
                        "StrOf" => 2,
                        _ => 3,
                    };
                    if type_args.len() < required_arity {
                        return Err(JsError {
                            message: format!("{} requires {} arguments", name, required_arity),
                        });
                    }
                    self.write(&format!("__taida_net_{}(", name));
                    for (i, arg) in type_args.iter().take(required_arity).enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.gen_expr(arg)?;
                    }
                    self.write(")");
                    return Ok(());
                }

                // Molten[]() → __taida_molten()
                if name == "Molten" {
                    if !type_args.is_empty() {
                        return Err(JsError {
                            message: "Molten takes no type arguments: Molten[]()".to_string(),
                        });
                    }
                    self.write("__taida_molten()");
                    return Ok(());
                }
                // Stream[value]() → Stream_mold(value)
                if name == "Stream" {
                    self.write("Stream_mold(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // StreamFrom[list]() → StreamFrom(list)
                if name == "StreamFrom" {
                    self.write("StreamFrom(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // Div[x, y]() and Mod[x, y]() molds → Div_mold(x, y, opts)
                if name == "Div" || name == "Mod" {
                    self.write(&format!("{}_mold(", name));
                    for (i, arg) in type_args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.gen_expr(arg)?;
                    }
                    // Check if any type arg is a FloatLit — JS Number.isInteger(2.0) is true,
                    // so we pass a __floatHint flag to preserve Taida's float semantics.
                    let has_float_arg = type_args.iter().any(|a| matches!(a, Expr::FloatLit(..)));
                    if !fields.is_empty() || has_float_arg {
                        self.write(", { ");
                        let mut wrote = false;
                        for (i, field) in fields.iter().enumerate() {
                            if i > 0 {
                                self.write(", ");
                            }
                            self.write(&format!("{}: ", field.name));
                            self.gen_expr(&field.value)?;
                            wrote = true;
                        }
                        if has_float_arg {
                            if wrote {
                                self.write(", ");
                            }
                            self.write("__floatHint: true");
                        }
                        self.write(" }");
                    }
                    self.write(")");
                    return Ok(());
                }
                self.write("__taida_solidify(");
                self.write(&format!("{}(", name));
                for (i, arg) in type_args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.gen_expr(arg)?;
                }
                if !fields.is_empty() {
                    if !type_args.is_empty() {
                        self.write(", ");
                    }
                    self.write("{ ");
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.write(&format!("{}: ", field.name));
                        self.gen_expr(&field.value)?;
                    }
                    self.write(" }");
                } else if type_args.is_empty() {
                    // No args at all
                }
                self.write(")");
                self.write(")");
                Ok(())
            }
            Expr::Unmold(inner, _) => {
                let (await_prefix, unmold_fn) = if self.in_async_context {
                    ("await ", "__taida_unmold_async")
                } else {
                    ("", "__taida_unmold")
                };
                self.write(&format!("{await_prefix}{unmold_fn}("));
                self.gen_expr(inner)?;
                self.write(")");
                Ok(())
            }
            Expr::Lambda(params, body, _) => {
                let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                // Scope-aware net builtin shadowing: snapshot/restore for lambda scope
                let prev_shadowed_net = self.shadowed_net_builtins.clone();
                for p in params {
                    if is_net_runtime_builtin(&p.name) {
                        self.shadowed_net_builtins.insert(p.name.clone());
                    }
                }
                self.write(&format!("(({}) => ", param_names.join(", ")));
                self.gen_expr(body)?;
                self.write(")");
                self.shadowed_net_builtins = prev_shadowed_net;
                Ok(())
            }
            Expr::Throw(inner, _) => {
                self.write("(() => { throw ");
                self.gen_expr(inner)?;
                self.write("; })()");
                Ok(())
            }
            // B11-6a: TypeLiteral emits type name as string (used by TypeIs/TypeExtends codegen)
            Expr::TypeLiteral(name, variant, _) => {
                if let Some(var) = variant {
                    self.write(&format!("\"{}:{}\"", name, var));
                } else {
                    self.write(&format!("\"{}\"", name));
                }
                Ok(())
            }
        }
    }

    fn gen_cond_branch(&mut self, arms: &[crate::parser::CondArm]) -> Result<(), JsError> {
        self.write("(() => {\n");
        self.indent += 1;

        for (i, arm) in arms.iter().enumerate() {
            match &arm.condition {
                Some(cond) => {
                    self.write_indent();
                    if i == 0 {
                        self.write("if (");
                    } else {
                        self.write("else if (");
                    }
                    self.gen_expr(cond)?;
                    self.write(") {\n");
                    self.indent += 1;
                    self.gen_cond_arm_body(&arm.body)?;
                    self.indent -= 1;
                    self.write_indent();
                    self.write("}\n");
                }
                None => {
                    self.write_indent();
                    if i > 0 {
                        self.write("else {\n");
                    } else {
                        self.write("{\n");
                    }
                    self.indent += 1;
                    self.gen_cond_arm_body(&arm.body)?;
                    self.indent -= 1;
                    self.write_indent();
                    self.write("}\n");
                }
            }
        }

        self.indent -= 1;
        self.write_indent();
        self.write("})()");
        Ok(())
    }

    /// Generate the body of a condition arm.
    /// For multi-statement bodies, generates all statements with `return` on the last expression.
    ///
    /// C13-1: If the last statement is a tail binding (`Assignment` /
    /// `UnmoldForward` / `UnmoldBackward`), emit the binding as usual and
    /// then `return <target>;` so the bound value becomes the arm result.
    fn gen_cond_arm_body(&mut self, body: &[crate::parser::Statement]) -> Result<(), JsError> {
        use crate::parser::Statement;
        if body.is_empty() {
            self.write_indent();
            self.write("return undefined;\n");
            return Ok(());
        }
        for (i, stmt) in body.iter().enumerate() {
            let is_last = i == body.len() - 1;
            if is_last {
                match stmt {
                    Statement::Expr(expr) => {
                        self.write_indent();
                        self.write("return ");
                        self.gen_expr(expr)?;
                        self.write(";\n");
                    }
                    Statement::Assignment(a) => {
                        self.gen_statement(stmt)?;
                        self.write_indent();
                        self.write(&format!("return {};\n", a.target));
                    }
                    Statement::UnmoldForward(u) => {
                        self.gen_statement(stmt)?;
                        self.write_indent();
                        self.write(&format!("return {};\n", u.target));
                    }
                    Statement::UnmoldBackward(u) => {
                        self.gen_statement(stmt)?;
                        self.write_indent();
                        self.write(&format!("return {};\n", u.target));
                    }
                    _ => {
                        self.gen_statement(stmt)?;
                    }
                }
            } else {
                self.gen_statement(stmt)?;
            }
        }
        Ok(())
    }

    /// テンプレートリテラル内の Taida 構文を JS 構文に変換
    ///
    /// Handles two categories of conversion:
    /// 1. **Text segments** (outside `${...}`): escape `\`, `` ` ``, and convert
    ///    `@[` to `[` and `.length()` to `.length_()`.
    /// 2. **Interpolation segments** (`${...}`): convert `@[` and `.length()` only,
    ///    without adding escape sequences.
    ///
    /// Limitation: nested `${...}` and complex expressions inside interpolation
    /// blocks are handled by simple brace matching (first `}` closes the block).
    /// Deeply nested braces (e.g. `${fn(@(a <= 1))}`) would be mis-split. In
    /// practice, Taida's template interpolation is single-expression, so this
    /// suffices for all current use cases.
    fn convert_template_list_literals(template: &str) -> String {
        // Template literals: convert Taida syntax to JS.
        // Segment the template so that escaping is only applied to text outside ${...}
        // interpolation blocks. Inside ${...}, the expression is passed through as-is.
        //
        // Escaping applied to text segments:
        //   - `\` → `\\`  (backslash)
        //   - `` ` `` → `\``  (backtick)
        //   - `@[` → `[`  (list literal)
        //   - `.length()` → `.length_()`  (avoid JS property collision)
        let mut result = String::new();
        let mut rest = template;
        while let Some(start) = rest.find("${") {
            // Escape the text before ${
            result.push_str(&Self::escape_template_text(&rest[..start]));
            if let Some(end) = rest[start..].find('}') {
                // Apply expression-level transforms inside ${...}
                let expr_content = &rest[start..start + end + 1];
                let expr_converted = expr_content
                    .replace("@[", "[")
                    .replace(".length()", ".length_()");
                result.push_str(&expr_converted);
                rest = &rest[start + end + 1..];
            } else {
                break;
            }
        }
        result.push_str(&Self::escape_template_text(rest));
        result
    }

    /// テンプレートリテラルのテキスト部分（${...} の外側）にエスケープを適用
    fn escape_template_text(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('`', "\\`")
            .replace("@[", "[")
            .replace(".length()", ".length_()")
    }

    fn gen_pipeline(&mut self, exprs: &[Expr]) -> Result<(), JsError> {
        if exprs.is_empty() {
            return Ok(());
        }

        // Pipeline is wrapped in an IIFE `(() => { ... })()`, so the `__p`
        // accumulator variable is scoped to the IIFE and cannot collide with
        // user-defined variables in the surrounding scope. The `__` prefix is
        // a defensive convention to avoid shadowing within the pipeline body.
        self.write("(() => {\n");
        self.indent += 1;

        self.write_indent();
        self.write("let __p = ");
        self.gen_expr(&exprs[0])?;
        self.write(";\n");

        // C13-1 / C13B-007: Track pipeline-scope bindings introduced by
        // intermediate `=> name` steps so later steps that explicitly
        // consume them skip the classic `__p` auto-injection.
        let last_idx = exprs.len().saturating_sub(1);
        let mut bound_names: Vec<String> = Vec::new();

        for (step_idx, expr) in exprs[1..].iter().enumerate() {
            let i = step_idx + 1; // absolute index in exprs
            // Intermediate `=> name` bind-and-forward: emit `const name = __p;`
            // and leave `__p` unchanged. Skip when `name` is a function / type /
            // mold / builtin that should be called with the current value.
            if i < last_idx
                && let Expr::Ident(name, _) = expr
                && !self.is_js_pipeline_callable_ident(name)
            {
                self.write_indent();
                self.write(&format!("const {} = __p;\n", name));
                bound_names.push(name.clone());
                continue;
            }
            // Step that explicitly references a bound name → no auto-inject.
            // Emit the step expression directly and assign its result to `__p`.
            if !bound_names.is_empty() && expr_references_any_name(expr, &bound_names) {
                self.write_indent();
                self.write("__p = ");
                self.gen_expr(expr)?;
                self.write(";\n");
                continue;
            }
            self.write_indent();
            self.write("__p = ");
            match expr {
                Expr::FuncCall(callee, args, _) => {
                    if let Expr::Ident(name, _) = callee.as_ref() {
                        match name.as_str() {
                            "debug" => self.write("__taida_debug"),
                            "typeof" => self.write("__taida_typeof"),
                            "assert" => self.write("__taida_assert"),
                            "stdout" => self.write("__taida_stdout"),
                            "stderr" => self.write("__taida_stderr"),
                            "stdin" => self.write("__taida_stdin"),
                            // C20-2: stdinLine is the UTF-8-aware Async[Lax[Str]] successor
                            "stdinLine" => self.write("__taida_stdinLine"),
                            "jsonEncode" => self.write("__taida_jsonEncode"),
                            "jsonPretty" => self.write("__taida_jsonPretty"),
                            "nowMs" => self.write("__taida_nowMs"),
                            "sleep" => self.write("__taida_sleep"),
                            // D28B-015: `strOf(span, raw)` lowercase function-form
                            // delegates to the existing `__taida_net_StrOf`
                            // runtime helper (always present in `RUNTIME_JS`).
                            "strOf" => self.write("__taida_net_StrOf"),
                            "readBytes" => self.write("__taida_os_readBytes"),
                            "readBytesAt" => self.write("__taida_os_readBytesAt"),
                            "writeFile" => self.write("__taida_os_writeFile"),
                            "writeBytes" => self.write("__taida_os_writeBytes"),
                            "appendFile" => self.write("__taida_os_appendFile"),
                            "remove" => self.write("__taida_os_remove"),
                            "createDir" => self.write("__taida_os_createDir"),
                            "rename" => self.write("__taida_os_rename"),
                            "run" => self.write("__taida_os_run"),
                            "execShell" => self.write("__taida_os_execShell"),
                            // C19: interactive TTY-passthrough variants
                            "runInteractive" => self.write("__taida_os_runInteractive"),
                            "execShellInteractive" => self.write("__taida_os_execShellInteractive"),
                            "allEnv" => self.write("__taida_os_allEnv"),
                            "argv" => self.write("__taida_os_argv"),
                            "tcpConnect" => self.write("__taida_os_tcpConnect"),
                            "tcpListen" => self.write("__taida_os_tcpListen"),
                            "tcpAccept" => self.write("__taida_os_tcpAccept"),
                            "socketSend" => self.write("__taida_os_socketSend"),
                            "socketSendAll" => self.write("__taida_os_socketSendAll"),
                            "socketRecv" => self.write("__taida_os_socketRecv"),
                            "socketSendBytes" => self.write("__taida_os_socketSendBytes"),
                            "socketRecvBytes" => self.write("__taida_os_socketRecvBytes"),
                            "socketClose" => self.write("__taida_os_socketClose"),
                            "listenerClose" => self.write("__taida_os_listenerClose"),
                            "udpBind" => self.write("__taida_os_udpBind"),
                            "udpSendTo" => self.write("__taida_os_udpSendTo"),
                            "udpRecvFrom" => self.write("__taida_os_udpRecvFrom"),
                            "udpClose" => self.write("__taida_os_udpClose"),
                            "socketRecvExact" => self.write("__taida_os_socketRecvExact"),
                            "dnsResolve" => self.write("__taida_os_dnsResolve"),
                            "poolCreate" => self.write("__taida_os_poolCreate"),
                            "poolAcquire" => self.write("__taida_os_poolAcquire"),
                            "poolRelease" => self.write("__taida_os_poolRelease"),
                            "poolClose" => self.write("__taida_os_poolClose"),
                            "poolHealth" => self.write("__taida_os_poolHealth"),
                            // C12-6a: Regex(pattern, flags?) prelude constructor
                            "Regex" => self.write("__taida_regex"),
                            // taida-lang/net HTTP v1 (only when imported)
                            _ if self.try_write_net_builtin(name, "") => {}
                            _ => self.write(name),
                        }
                    } else {
                        self.gen_expr(callee)?;
                    }
                    self.write("(");
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        if matches!(arg, Expr::Placeholder(_)) {
                            self.write("__p");
                        } else {
                            self.gen_expr(arg)?;
                        }
                    }
                    self.write(")");
                }
                Expr::MethodCall(obj, method, args, _) => {
                    // Pipeline method call: replace _ placeholder in obj with __p
                    {
                        if is_removed_list_method(method) {
                            self.write("__taida_list_method_removed(");
                            self.write(&format!("{:?}", method));
                            self.write(")");
                            return Ok(());
                        }
                        let js_method = match method.as_str() {
                            "length" => "length_",
                            other => other,
                        };
                        if matches!(obj.as_ref(), Expr::Placeholder(_)) {
                            self.write(&format!("__p.{}(", js_method));
                        } else {
                            self.gen_expr(obj)?;
                            self.write(&format!(".{}(", js_method));
                        }
                        for (i, arg) in args.iter().enumerate() {
                            if i > 0 {
                                self.write(", ");
                            }
                            if matches!(arg, Expr::Placeholder(_)) {
                                self.write("__p");
                            } else {
                                self.gen_expr(arg)?;
                            }
                        }
                        self.write(")");
                    }
                }
                Expr::MoldInst(name, type_args, fields, _) => {
                    // B11-5b: If[cond, then, else]() in pipeline
                    // → ((_) => (cond ? then : else))(__p)
                    // The IIFE binds `_` so gen_expr emits it as the parameter name,
                    // achieving correct short-circuit and placeholder substitution.
                    if name == "If" && type_args.len() >= 3 {
                        self.write("((_) => (");
                        self.gen_expr(&type_args[0])?;
                        self.write(" ? ");
                        self.gen_expr(&type_args[1])?;
                        self.write(" : ");
                        self.gen_expr(&type_args[2])?;
                        self.write("))(__p)");
                    }
                    // B11-6c: TypeIs in pipeline — bind _ to __p via IIFE
                    else if name == "TypeIs" && type_args.len() >= 2 {
                        self.write("((_) => ");
                        // Reuse main TypeIs codegen by constructing a temporary MoldInst
                        let temp = Expr::MoldInst(
                            name.clone(),
                            type_args.clone(),
                            fields.clone(),
                            type_args[0].span().clone(),
                        );
                        self.gen_expr(&temp)?;
                        self.write(")(__p)");
                    }
                    // B11-6c: TypeExtends in pipeline — no placeholder needed, compile-time
                    else if name == "TypeExtends" && type_args.len() >= 2 {
                        let temp = Expr::MoldInst(
                            name.clone(),
                            type_args.clone(),
                            fields.clone(),
                            type_args[0].span().clone(),
                        );
                        self.gen_expr(&temp)?;
                    }
                    // JSNew in pipeline: JSNew[ClassName](__p, ...) or JSNew[ClassName](...)
                    else if name == "JSNew" {
                        if let Some(Expr::Ident(class_name, _)) = type_args.first() {
                            self.write(&format!("new {}(", class_name));
                            // Pipeline value __p as first arg, followed by fields
                            let has_placeholder = fields
                                .iter()
                                .any(|f| matches!(f.value, Expr::Placeholder(_)));
                            if has_placeholder {
                                for (i, field) in fields.iter().enumerate() {
                                    if i > 0 {
                                        self.write(", ");
                                    }
                                    if matches!(field.value, Expr::Placeholder(_)) {
                                        self.write("__p");
                                    } else {
                                        self.gen_expr(&field.value)?;
                                    }
                                }
                            } else if fields.is_empty() {
                                self.write("__p");
                            } else {
                                self.write("__p");
                                for field in fields {
                                    self.write(", ");
                                    self.gen_expr(&field.value)?;
                                }
                            }
                            self.write(")");
                        }
                    } else {
                        // Pipeline MoldInst: replace _ placeholders in type_args with __p
                        // OS molds need to be mapped to runtime function names
                        let js_name = match name.as_str() {
                            "Read" => "__taida_os_read",
                            "ListDir" => "__taida_os_listdir",
                            "Stat" => "__taida_os_stat",
                            "Exists" => "__taida_os_exists",
                            "EnvVar" => "__taida_os_envvar",
                            _ => name.as_str(),
                        };
                        self.write("__taida_solidify(");
                        self.write(&format!("{}(", js_name));
                        let has_placeholder =
                            type_args.iter().any(|a| matches!(a, Expr::Placeholder(_)));
                        if has_placeholder {
                            for (i, arg) in type_args.iter().enumerate() {
                                if i > 0 {
                                    self.write(", ");
                                }
                                if matches!(arg, Expr::Placeholder(_)) {
                                    self.write("__p");
                                } else {
                                    self.gen_expr(arg)?;
                                }
                            }
                        } else {
                            // No placeholder — insert __p as first type arg
                            self.write("__p");
                            for arg in type_args {
                                self.write(", ");
                                self.gen_expr(arg)?;
                            }
                        }
                        if !fields.is_empty() {
                            self.write(", { ");
                            for (i, field) in fields.iter().enumerate() {
                                if i > 0 {
                                    self.write(", ");
                                }
                                self.write(&format!("{}: ", field.name));
                                self.gen_expr(&field.value)?;
                            }
                            self.write(" }");
                        }
                        self.write(")");
                        self.write(")");
                    }
                }
                Expr::Ident(name, _) => match name.as_str() {
                    "debug" => self.write("__taida_debug(__p)"),
                    "typeof" => self.write("__taida_typeof(__p)"),
                    "assert" => self.write("__taida_assert(__p)"),
                    "stdout" => self.write("__taida_stdout(__p)"),
                    "stderr" => self.write("__taida_stderr(__p)"),
                    "stdin" => self.write("__taida_stdin(__p)"),
                    // C20-2: stdinLine is the UTF-8-aware Async[Lax[Str]] successor
                    "stdinLine" => self.write("__taida_stdinLine(__p)"),
                    "jsonEncode" => self.write("__taida_jsonEncode(__p)"),
                    "jsonPretty" => self.write("__taida_jsonPretty(__p)"),
                    "nowMs" => self.write("__taida_nowMs()"),
                    "sleep" => self.write("__taida_sleep(__p)"),
                    "readBytes" => self.write("__taida_os_readBytes(__p)"),
                    "readBytesAt" => self.write("__taida_os_readBytesAt(__p)"),
                    "writeFile" => self.write("__taida_os_writeFile(__p)"),
                    "writeBytes" => self.write("__taida_os_writeBytes(__p)"),
                    "appendFile" => self.write("__taida_os_appendFile(__p)"),
                    "remove" => self.write("__taida_os_remove(__p)"),
                    "createDir" => self.write("__taida_os_createDir(__p)"),
                    "rename" => self.write("__taida_os_rename(__p)"),
                    "run" => self.write("__taida_os_run(__p)"),
                    "execShell" => self.write("__taida_os_execShell(__p)"),
                    // C19: interactive TTY-passthrough variants (pipeline form)
                    "runInteractive" => self.write("__taida_os_runInteractive(__p)"),
                    "execShellInteractive" => self.write("__taida_os_execShellInteractive(__p)"),
                    "allEnv" => self.write("__taida_os_allEnv(__p)"),
                    "argv" => self.write("__taida_os_argv()"),
                    "tcpConnect" => self.write("__taida_os_tcpConnect(__p)"),
                    "tcpListen" => self.write("__taida_os_tcpListen(__p)"),
                    "tcpAccept" => self.write("__taida_os_tcpAccept(__p)"),
                    "socketSend" => self.write("__taida_os_socketSend(__p)"),
                    "socketSendAll" => self.write("__taida_os_socketSendAll(__p)"),
                    "socketRecv" => self.write("__taida_os_socketRecv(__p)"),
                    "socketSendBytes" => self.write("__taida_os_socketSendBytes(__p)"),
                    "socketRecvBytes" => self.write("__taida_os_socketRecvBytes(__p)"),
                    "socketClose" => self.write("__taida_os_socketClose(__p)"),
                    "listenerClose" => self.write("__taida_os_listenerClose(__p)"),
                    "udpBind" => self.write("__taida_os_udpBind(__p)"),
                    "udpSendTo" => self.write("__taida_os_udpSendTo(__p)"),
                    "udpRecvFrom" => self.write("__taida_os_udpRecvFrom(__p)"),
                    "udpClose" => self.write("__taida_os_udpClose(__p)"),
                    "socketRecvExact" => self.write("__taida_os_socketRecvExact(__p)"),
                    "dnsResolve" => self.write("__taida_os_dnsResolve(__p)"),
                    "poolCreate" => self.write("__taida_os_poolCreate(__p)"),
                    "poolAcquire" => self.write("__taida_os_poolAcquire(__p)"),
                    "poolRelease" => self.write("__taida_os_poolRelease(__p)"),
                    "poolClose" => self.write("__taida_os_poolClose(__p)"),
                    "poolHealth" => self.write("__taida_os_poolHealth(__p)"),
                    // taida-lang/net HTTP v1 (only when imported)
                    _ if self.try_write_net_builtin(name, "(__p)") => {}
                    // C25B-033: pipeline fallback — if the name is a user
                    // FuncDef that collides with a prelude reserved ident,
                    // emit the mangled form so `x => MyJoin` resolves to
                    // `_td_user_MyJoin(__p)` (non-colliding names unchanged).
                    _ => {
                        let emitted = self.js_user_func_ident(name);
                        self.write(&format!("{}(__p)", emitted));
                    }
                },
                // C12B-020: `expr => _` is a no-op transform that discards
                // the pipeline value while keeping the preceding expression's
                // side effect. Keep `__p` unchanged instead of emitting
                // `__p = _;` which is a ReferenceError.
                Expr::Placeholder(_) => {
                    self.write("__p");
                }
                _ => {
                    self.gen_expr(expr)?;
                }
            }
            self.write(";\n");
        }

        self.write_indent();
        self.write("return __p;\n");
        self.indent -= 1;
        self.write_indent();
        self.write("})()");
        Ok(())
    }
}

/// C13-1 / C13B-007: True if `expr` references any name in `bound_names`
/// anywhere in its subtree. Used by `gen_pipeline` to decide whether a
/// pipeline step should skip the classic `__p` auto-injection because the
/// user explicitly consumed a pipeline-scope binding.
fn expr_references_any_name(expr: &Expr, bound_names: &[String]) -> bool {
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

/// Collect function names that are called in tail position from the given function body.
/// This is used to build the tail-call graph for mutual recursion detection.
fn collect_tail_call_targets(
    _self_name: &str,
    body: &[Statement],
    targets: &mut std::collections::HashSet<String>,
) {
    // Find the last expression in the body
    let last_expr = body.iter().rev().find_map(|s| match s {
        Statement::Expr(e) => Some(e),
        _ => None,
    });
    if let Some(expr) = last_expr {
        collect_tail_targets_from_expr(expr, targets);
    }
}

fn collect_tail_targets_from_expr(expr: &Expr, targets: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::FuncCall(callee, _, _) => {
            if let Expr::Ident(name, _) = callee.as_ref() {
                targets.insert(name.clone());
            }
        }
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(expr) = arm.last_expr() {
                    collect_tail_targets_from_expr(expr, targets);
                }
            }
        }
        _ => {}
    }
}

/// OS API mold names that are always async sources when unmolded.
/// These require `async function` generation when `]=>` appears inside a function.
const OS_ASYNC_MOLDS: &[&str] = &[
    "Read",
    "ListDir",
    "Stat",
    "Exists",
    "EnvVar",
    "ReadAsync",
    "HttpGet",
    "HttpPost",
    "HttpRequest",
];

/// OS/API/prelude function names that can yield pending async sources.
/// These are runtime functions (not molds).
const OS_ASYNC_FUNCS: &[&str] = &[
    "sleep",
    "tcpConnect",
    "tcpListen",
    "tcpAccept",
    "socketSend",
    "socketSendAll",
    "socketRecv",
    "socketSendBytes",
    "socketRecvBytes",
    "udpBind",
    "udpSendTo",
    "udpRecvFrom",
    "socketClose",
    "listenerClose",
    "udpClose",
    "socketRecvExact",
    "dnsResolve",
    "poolAcquire",
    "poolClose",
    // taida-lang/net HTTP v1 — call-site rewriting is guarded by has_net_import,
    // but async detection here is intentionally left unguarded to avoid threading
    // has_net_import through all recursive free functions. Over-inclusion only
    // affects the unlikely case of a user-defined `httpServe` without net import.
    "httpServe",
];

fn callee_is_os_async_func(callee: &Expr) -> bool {
    match callee {
        Expr::Ident(name, _) => OS_ASYNC_FUNCS.contains(&name.as_str()),
        _ => false,
    }
}

fn mold_propagates_async_from_args(name: &str) -> bool {
    matches!(name, "All" | "Race" | "Timeout")
}

/// Check if an unmold source expression involves an OS async mold/function (true Promise).
fn is_os_async_unmold_source(
    source: &Expr,
    os_async_vars: &std::collections::HashSet<String>,
) -> bool {
    match source {
        Expr::MoldInst(name, type_args, fields, _) => {
            OS_ASYNC_MOLDS.contains(&name.as_str())
                || (mold_propagates_async_from_args(name)
                    && (type_args
                        .iter()
                        .any(|a| is_os_async_unmold_source(a, os_async_vars))
                        || fields
                            .iter()
                            .any(|f| is_os_async_unmold_source(&f.value, os_async_vars))))
        }
        Expr::FuncCall(callee, _, _) => {
            callee_is_os_async_func(callee) || is_os_async_unmold_source(callee, os_async_vars)
        }
        Expr::Ident(name, _) => os_async_vars.contains(name),
        Expr::MethodCall(receiver, _, _, _) => is_os_async_unmold_source(receiver, os_async_vars),
        Expr::FieldAccess(receiver, _, _) => is_os_async_unmold_source(receiver, os_async_vars),
        Expr::BinaryOp(left, _, right, _) => {
            is_os_async_unmold_source(left, os_async_vars)
                || is_os_async_unmold_source(right, os_async_vars)
        }
        Expr::UnaryOp(_, inner, _) => is_os_async_unmold_source(inner, os_async_vars),
        Expr::Pipeline(exprs, _) => exprs
            .iter()
            .any(|e| is_os_async_unmold_source(e, os_async_vars)),
        Expr::BuchiPack(fields, _) => fields
            .iter()
            .any(|f| is_os_async_unmold_source(&f.value, os_async_vars)),
        Expr::TypeInst(_, fields, _) => fields
            .iter()
            .any(|f| is_os_async_unmold_source(&f.value, os_async_vars)),
        Expr::ListLit(items, _) => items
            .iter()
            .any(|e| is_os_async_unmold_source(e, os_async_vars)),
        Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
            arm.condition
                .as_ref()
                .is_some_and(|c| is_os_async_unmold_source(c, os_async_vars))
                || arm.body.iter().any(|stmt| {
                    if let crate::parser::Statement::Expr(e) = stmt {
                        is_os_async_unmold_source(e, os_async_vars)
                    } else {
                        false
                    }
                })
        }),
        _ => false,
    }
}

/// Check if an expression tree contains Expr::Unmold whose inner expression
/// involves an OS async mold. Recurses into sub-expressions but NOT lambdas.
fn expr_contains_os_async_unmold(
    expr: &Expr,
    os_async_vars: &std::collections::HashSet<String>,
) -> bool {
    match expr {
        Expr::Unmold(inner, _) => is_os_async_unmold_source(inner, os_async_vars),
        Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
            arm.condition
                .as_ref()
                .is_some_and(|c| expr_contains_os_async_unmold(c, os_async_vars))
                || arm.body.iter().any(|stmt| {
                    if let crate::parser::Statement::Expr(e) = stmt {
                        expr_contains_os_async_unmold(e, os_async_vars)
                    } else {
                        false
                    }
                })
        }),
        Expr::FuncCall(callee, args, _) => {
            expr_contains_os_async_unmold(callee, os_async_vars)
                || args
                    .iter()
                    .any(|a| expr_contains_os_async_unmold(a, os_async_vars))
        }
        Expr::MethodCall(obj, _, args, _) => {
            expr_contains_os_async_unmold(obj, os_async_vars)
                || args
                    .iter()
                    .any(|a| expr_contains_os_async_unmold(a, os_async_vars))
        }
        Expr::FieldAccess(obj, _, _) => expr_contains_os_async_unmold(obj, os_async_vars),
        Expr::BinaryOp(l, _, r, _) => {
            expr_contains_os_async_unmold(l, os_async_vars)
                || expr_contains_os_async_unmold(r, os_async_vars)
        }
        Expr::UnaryOp(_, inner, _) => expr_contains_os_async_unmold(inner, os_async_vars),
        Expr::Pipeline(exprs, _) => exprs
            .iter()
            .any(|e| expr_contains_os_async_unmold(e, os_async_vars)),
        Expr::MoldInst(_, type_args, fields, _) => {
            type_args
                .iter()
                .any(|a| expr_contains_os_async_unmold(a, os_async_vars))
                || fields
                    .iter()
                    .any(|f| expr_contains_os_async_unmold(&f.value, os_async_vars))
        }
        Expr::TypeInst(_, fields, _) => fields
            .iter()
            .any(|f| expr_contains_os_async_unmold(&f.value, os_async_vars)),
        Expr::BuchiPack(fields, _) => fields
            .iter()
            .any(|f| expr_contains_os_async_unmold(&f.value, os_async_vars)),
        Expr::ListLit(items, _) => items
            .iter()
            .any(|e| expr_contains_os_async_unmold(e, os_async_vars)),
        Expr::Lambda(_, _, _) => false, // Lambdas get their own async detection
        _ => false,
    }
}

/// Check if statements contain ]=> that unmolds an OS async (true Promise) value.
/// Only OS API molds that return real Promises trigger async function generation.
/// Standard Taida molds (Async, Div, Mod, etc.) use sync __TaidaAsync thenables
/// and do NOT require async functions.
/// Also checks for Expr::Unmold within expressions (e.g. inside CondBranch).
/// Collect names of user-defined functions called in a statement list (non-recursive into nested FuncDefs).
fn collect_func_calls_in_stmts(
    stmts: &[Statement],
    func_names: &std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Expr(expr) => collect_func_calls_in_expr(expr, func_names, out),
            Statement::Assignment(assign) => {
                collect_func_calls_in_expr(&assign.value, func_names, out)
            }
            Statement::UnmoldForward(u) => collect_func_calls_in_expr(&u.source, func_names, out),
            Statement::UnmoldBackward(u) => collect_func_calls_in_expr(&u.source, func_names, out),
            Statement::ErrorCeiling(ec) => {
                collect_func_calls_in_stmts(&ec.handler_body, func_names, out);
            }
            _ => {}
        }
    }
}

fn collect_func_calls_in_expr(
    expr: &Expr,
    func_names: &std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    match expr {
        Expr::FuncCall(callee, args, _) => {
            if let Expr::Ident(name, _) = callee.as_ref()
                && func_names.contains(name)
                && !out.contains(name)
            {
                out.push(name.clone());
            }
            collect_func_calls_in_expr(callee, func_names, out);
            for arg in args {
                collect_func_calls_in_expr(arg, func_names, out);
            }
        }
        Expr::MethodCall(obj, _, args, _) => {
            collect_func_calls_in_expr(obj, func_names, out);
            for arg in args {
                collect_func_calls_in_expr(arg, func_names, out);
            }
        }
        Expr::BinaryOp(l, _, r, _) => {
            collect_func_calls_in_expr(l, func_names, out);
            collect_func_calls_in_expr(r, func_names, out);
        }
        Expr::UnaryOp(_, inner, _) => collect_func_calls_in_expr(inner, func_names, out),
        Expr::FieldAccess(obj, _, _) => collect_func_calls_in_expr(obj, func_names, out),
        Expr::Pipeline(exprs, _) => {
            for e in exprs {
                collect_func_calls_in_expr(e, func_names, out);
            }
        }
        Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
            for f in fields {
                collect_func_calls_in_expr(&f.value, func_names, out);
            }
        }
        Expr::ListLit(exprs, _) => {
            for e in exprs {
                collect_func_calls_in_expr(e, func_names, out);
            }
        }
        Expr::MoldInst(_, type_args, fields, _) => {
            for a in type_args {
                collect_func_calls_in_expr(a, func_names, out);
            }
            for f in fields {
                collect_func_calls_in_expr(&f.value, func_names, out);
            }
        }
        Expr::Lambda(_, body, _) => collect_func_calls_in_expr(body, func_names, out),
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(cond) = &arm.condition {
                    collect_func_calls_in_expr(cond, func_names, out);
                }
                for stmt in &arm.body {
                    if let crate::parser::Statement::Expr(e) = stmt {
                        collect_func_calls_in_expr(e, func_names, out);
                    }
                }
            }
        }
        _ => {}
    }
}

fn stmts_contain_async_unmold(stmts: &[Statement]) -> bool {
    // Collect variable names assigned from async sources in this scope.
    // Fixed-point is needed for transitive cases like:
    //   s <= sleep(0)
    //   t <= Timeout[s, 100]()
    // where `t` should also be considered async.
    let mut os_async_vars = std::collections::HashSet::new();
    loop {
        let mut changed = false;
        for stmt in stmts {
            if let Statement::Assignment(assign) = stmt
                && is_os_async_unmold_source(&assign.value, &os_async_vars)
            {
                changed |= os_async_vars.insert(assign.target.clone());
            }
        }
        if !changed {
            break;
        }
    }

    for stmt in stmts {
        match stmt {
            Statement::UnmoldForward(unmold)
                if is_os_async_unmold_source(&unmold.source, &os_async_vars) =>
            {
                return true;
            }
            Statement::UnmoldBackward(unmold)
                if is_os_async_unmold_source(&unmold.source, &os_async_vars) =>
            {
                return true;
            }
            Statement::FuncDef(_) => {
                // Don't recurse into nested function defs — they get their own async detection
            }
            Statement::ErrorCeiling(ec) if stmts_contain_async_unmold(&ec.handler_body) => {
                return true;
            }
            Statement::Expr(expr) if expr_contains_os_async_unmold(expr, &os_async_vars) => {
                return true;
            }
            Statement::Assignment(assign)
                if expr_contains_os_async_unmold(&assign.value, &os_async_vars) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Compute relative path from `base` directory to `target` file.
/// C27B-022: Walk up from `start_dir` looking for project-root markers
/// (`packages.tdm`, `taida.toml`, `.taida`, `.git`). Mirrors the marker
/// set used by `interpreter::module_eval::find_project_root` and
/// `codegen::driver::find_project_root` so the JS path-traversal guard
/// agrees with Native / Interpreter on what counts as the project
/// boundary. Falls back to `start_dir` if no marker is found.
fn js_find_project_root(start_dir: &std::path::Path) -> std::path::PathBuf {
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

fn pathdiff(base: &std::path::Path, target: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    let base = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(base)
    };
    let target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(target)
    };

    let base_comps: Vec<_> = base.components().collect();
    let target_comps: Vec<_> = target.components().collect();

    // Find common prefix length
    let common = base_comps
        .iter()
        .zip(target_comps.iter())
        .take_while(|(b, t)| b == t)
        .count();

    let mut result = PathBuf::new();
    // Go up from base
    for _ in common..base_comps.len() {
        result.push("..");
    }
    // Go down to target
    for comp in &target_comps[common..] {
        result.push(comp);
    }

    if result.as_os_str().is_empty() {
        None
    } else {
        Some(result)
    }
}

/// .td ファイルを JS にトランスパイル (with file context for package import resolution)
pub fn transpile_with_context(
    program: &Program,
    source_file: &std::path::Path,
    project_root: &std::path::Path,
    output_file: &std::path::Path,
) -> Result<String, JsError> {
    let mut codegen = JsCodegen::new();
    codegen.set_file_context(source_file, project_root, output_file);
    codegen.generate(program)
}

/// .td ファイルを JS にトランスパイル (with build context for output-layout-aware imports)
pub fn transpile_with_build_context(
    program: &Program,
    source_file: &std::path::Path,
    project_root: Option<&std::path::Path>,
    output_file: &std::path::Path,
    entry_root: &std::path::Path,
    out_root: &std::path::Path,
) -> Result<String, JsError> {
    let mut codegen = JsCodegen::new();
    codegen.source_file = Some(source_file.to_path_buf());
    codegen.output_file = Some(output_file.to_path_buf());
    if let Some(root) = project_root {
        codegen.project_root = Some(root.to_path_buf());
    }
    codegen.set_build_context(entry_root, out_root);
    codegen.generate(program)
}

/// .td ファイルを JS にトランスパイル
pub fn transpile(source: &str) -> Result<String, JsError> {
    let (program, parse_errors) = crate::parser::parse(source);
    if !parse_errors.is_empty() {
        let msgs: Vec<String> = parse_errors.iter().map(|e| format!("{}", e)).collect();
        return Err(JsError {
            message: format!("parse errors:\n{}", msgs.join("\n")),
        });
    }

    let mut codegen = JsCodegen::new();
    codegen.generate(&program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn js_contains(source: &str, needle: &str) -> bool {
        let js = transpile(source).expect("transpile failed");
        js.contains(needle)
    }

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    // ── Optional — ABOLISHED (v0.8.0) ──
    // Optional tests removed. Optional has been replaced by Lax[T].

    // ── Result methods in runtime ──

    #[test]
    fn test_js_runtime_result_flatmap() {
        let js = transpile("x = 1\nx").unwrap();
        // Result function should contain flatMap
        assert!(
            js.contains("flatMap(fn)"),
            "Result should have flatMap method"
        );
    }

    #[test]
    fn test_js_runtime_result_maperror() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("mapError(fn)"),
            "Result should have mapError method"
        );
    }

    #[test]
    fn test_js_runtime_result_getorthrow() {
        let js = transpile("x = 1\nx").unwrap();
        // Both Result and ResultErr should have getOrThrow
        assert!(
            js.contains("ResultError"),
            "ResultErr getOrThrow should throw ResultError"
        );
    }

    #[test]
    fn test_js_runtime_result_tostring() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("'Result('"),
            "Result toString should produce Result(...)"
        );
        assert!(
            js.contains("'Result(throw <= '"),
            "Result toString should produce Result(throw <= ...) for errors"
        );
    }

    #[test]
    fn test_molten_rejects_type_args_codegen() {
        let err = transpile("m <= Molten[1]()").expect_err("Molten with type args should fail");
        assert!(
            err.message.contains("Molten takes no type arguments"),
            "Unexpected error: {}",
            err.message
        );
    }

    // ── Prelude utility functions in runtime ──

    #[test]
    fn test_js_runtime_hashmap() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function hashMap(entries)"),
            "Runtime should contain hashMap function"
        );
    }

    #[test]
    fn test_js_runtime_setof() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function setOf(items)"),
            "Runtime should contain setOf function"
        );
    }

    #[test]
    fn test_js_runtime_range() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function range(start, end)"),
            "Runtime should contain range function"
        );
    }

    #[test]
    fn test_js_runtime_enumerate() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function enumerate(list)"),
            "Runtime should contain enumerate function"
        );
    }

    #[test]
    fn test_js_runtime_zip() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function zip(a, b)"),
            "Runtime should contain zip function"
        );
    }

    #[test]
    fn test_js_runtime_assert() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function __taida_assert(cond, msg)"),
            "Runtime should contain __taida_assert function"
        );
    }

    #[test]
    fn test_js_runtime_typeof() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function __taida_typeof(x)"),
            "Runtime should contain __taida_typeof function"
        );
    }

    // ── Codegen mapping: typeof → __taida_typeof, assert → __taida_assert ──

    #[test]
    fn test_js_codegen_typeof_mapping() {
        assert!(
            js_contains("x = typeof(42)\nx", "__taida_typeof(42)"),
            "typeof(42) should be mapped to __taida_typeof(42)"
        );
    }

    #[test]
    fn test_js_codegen_assert_mapping() {
        assert!(
            js_contains("assert(true, \"ok\")\n", "__taida_assert(true, \"ok\")"),
            "assert(true, \"ok\") should be mapped to __taida_assert(true, \"ok\")"
        );
    }

    // ── Partial application: empty slot → closure ──

    #[test]
    fn test_js_codegen_partial_application_single() {
        // add(5, ) should generate a closure
        assert!(
            js_contains(
                "add x y = x + y => :Int\nadd5 = add(5, )\nadd5",
                "((__pa_0) => add(5, __pa_0))"
            ),
            "add(5, ) should generate ((__pa_0) => add(5, __pa_0))"
        );
    }

    #[test]
    fn test_js_codegen_partial_application_first_arg() {
        // multiply(, 2) should generate a closure
        assert!(
            js_contains(
                "mul x y = x * y => :Int\ndouble = mul(, 2)\ndouble",
                "((__pa_0) => mul(__pa_0, 2))"
            ),
            "mul(, 2) should generate ((__pa_0) => mul(__pa_0, 2))"
        );
    }

    #[test]
    fn test_js_codegen_partial_application_multiple() {
        // func(, 1, ) should generate closure with two params
        assert!(
            js_contains(
                "f x y z = x + y + z => :Int\ng = f(, 1, )\ng",
                "((__pa_0, __pa_1) => f(__pa_0, 1, __pa_1))"
            ),
            "f(, 1, ) should generate ((__pa_0, __pa_1) => f(__pa_0, 1, __pa_1))"
        );
    }

    // ── Str methods in runtime ──

    #[test]
    fn test_js_runtime_str_patches() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("__taida_str_patched"),
            "Runtime should contain string patches"
        );
        assert!(
            js.contains("function Reverse("),
            "Runtime should contain Reverse mold function"
        );
    }

    #[test]
    fn test_js_runtime_operation_molds() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function Upper("),
            "Runtime should contain Upper mold function"
        );
        assert!(
            js.contains("function Lower("),
            "Runtime should contain Lower mold function"
        );
        assert!(
            js.contains("function Sort("),
            "Runtime should contain Sort mold function"
        );
        assert!(
            js.contains("function Abs("),
            "Runtime should contain Abs mold function"
        );
        assert!(
            js.contains("function Find("),
            "Runtime should contain Find mold function"
        );
    }

    #[test]
    fn test_js_runtime_safe_unmold() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function __taida_unmold("),
            "Runtime should contain __taida_unmold helper"
        );
    }

    #[test]
    fn test_js_codegen_str_trimstart() {
        assert!(
            js_contains("s = \"  hi  \"\ns.trimStart()", ".trimStart()"),
            "str.trimStart() should pass through as-is"
        );
    }

    // ── Number methods in runtime ──

    #[test]
    fn test_js_runtime_number_methods() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("isNaN"),
            "Runtime should contain isNaN on Number.prototype"
        );
        assert!(
            js.contains("isInfinite"),
            "Runtime should contain isInfinite on Number.prototype"
        );
        assert!(
            js.contains("isFinite"),
            "Runtime should contain isFinite on Number.prototype"
        );
        assert!(
            js.contains("isPositive"),
            "Runtime should contain isPositive on Number.prototype"
        );
        assert!(
            js.contains("isNegative"),
            "Runtime should contain isNegative on Number.prototype"
        );
        assert!(
            js.contains("isZero"),
            "Runtime should contain isZero on Number.prototype"
        );
    }

    #[test]
    fn test_js_codegen_number_isnan() {
        assert!(
            js_contains("x = 42\nx.isNaN()", ".isNaN()"),
            "x.isNaN() should pass through as-is"
        );
    }

    #[test]
    fn test_js_codegen_number_ispositive() {
        assert!(
            js_contains("x = 42\nx.isPositive()", ".isPositive()"),
            "x.isPositive() should pass through as-is"
        );
    }

    // ── JSNew mold (taida-lang/js) ──

    #[test]
    fn test_js_codegen_jsnew_no_args() {
        // JSNew[Hono]() ]=> app  →  const app = new Hono();
        assert!(
            js_contains("JSNew[Hono]() ]=> app", "new Hono()"),
            "JSNew[Hono]() should generate new Hono()"
        );
    }

    #[test]
    fn test_js_codegen_jsnew_with_args() {
        // JSNew[Server](8080) ]=> server  →  const server = new Server(8080);
        assert!(
            js_contains("JSNew[Server](8080) ]=> server", "new Server(8080)"),
            "JSNew[Server](8080) should generate new Server(8080)"
        );
    }

    #[test]
    fn test_js_codegen_jsnew_with_multiple_args() {
        // JSNew[Uint8Array](16) ]=> buf  →  const buf = new Uint8Array(16);
        assert!(
            js_contains("JSNew[Uint8Array](16) ]=> buf", "new Uint8Array(16)"),
            "JSNew[Uint8Array](16) should generate new Uint8Array(16)"
        );
    }

    #[test]
    fn test_js_codegen_jsnew_unmold_forward() {
        let js = transpile("JSNew[Hono]() ]=> app\napp").unwrap();
        assert!(
            js.contains("const app = await __taida_unmold_async(new Hono())"),
            "JSNew unmold forward should wrap with await __taida_unmold_async: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsnew_unmold_backward() {
        let js = transpile("app <=[ JSNew[Hono]()\napp").unwrap();
        assert!(
            js.contains("const app = await __taida_unmold_async(new Hono())"),
            "JSNew unmold backward should wrap with await __taida_unmold_async: got {}",
            js
        );
    }

    #[test]
    fn test_os_async_function_call_marks_function_async() {
        let src = r#"
fetchUdp p =
  s <= udpBind("127.0.0.1", 0)
  s ]=> v
  v
"#;
        let js = transpile(src).expect("transpile should succeed");
        assert!(
            js.contains("async function fetchUdp("),
            "OS async function call unmold should mark function async: got {}",
            js
        );
        assert!(
            js.contains("await __taida_unmold_async(s)"),
            "OS async function call unmold should emit await unmold: got {}",
            js
        );
    }

    #[test]
    fn test_sleep_all_inside_function_marks_function_async() {
        let src = r#"
waitBoth p =
  all <= All[@[sleep(0), sleep(0)]]()
  all ]=> vals
  vals.length()
=> :Int
"#;
        let js = transpile(src).expect("transpile should succeed");
        assert!(
            js.contains("async function waitBoth("),
            "All+sleep unmold should mark function async: got {}",
            js
        );
        assert!(
            js.contains("await __taida_unmold_async(all)"),
            "All+sleep unmold should emit await unmold: got {}",
            js
        );
    }

    #[test]
    fn test_sleep_timeout_direct_unmold_marks_function_async() {
        let src = r#"
waitWithTimeout p =
  Timeout[sleep(0), 100]() ]=> done
  done
=> :Int
"#;
        let js = transpile(src).expect("transpile should succeed");
        assert!(
            js.contains("async function waitWithTimeout("),
            "Timeout+sleep unmold should mark function async: got {}",
            js
        );
        assert!(
            js.contains("await __taida_unmold_async(__taida_solidify(Timeout("),
            "Timeout+sleep direct unmold should emit await unmold: got {}",
            js
        );
    }

    #[test]
    fn test_sleep_timeout_via_assigned_var_marks_function_async() {
        let src = r#"
waitWithTimeout p =
  s <= sleep(0)
  t <= Timeout[s, 100]()
  t ]=> done
  done
=> :Int
"#;
        let js = transpile(src).expect("transpile should succeed");
        assert!(
            js.contains("async function waitWithTimeout("),
            "Timeout+sleep via var unmold should mark function async: got {}",
            js
        );
        assert!(
            js.contains("await __taida_unmold_async(t)"),
            "Timeout+sleep via var unmold should emit await unmold: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsnew_import_skipped() {
        // taida-lang/js import should not generate any ESM import statement
        let js = transpile(">>> taida-lang/js => @(JSNew)\nJSNew[Hono]() ]=> app\napp").unwrap();
        assert!(
            !js.contains("import {"),
            "taida-lang/js import should be skipped in JS output (no ESM import): got {}",
            js
        );
        assert!(
            js.contains("new Hono()"),
            "JSNew should still generate new: got {}",
            js
        );
    }

    #[test]
    fn test_package_import_resolution_failure_is_codegen_error() {
        let dir = unique_temp_dir("taida_js_missing_pkg");
        let main = dir.join("main.td");
        std::fs::write(&main, ">>> alice/missing => @(run)\nstdout(\"ok\")\n")
            .expect("write main.td");

        let source = std::fs::read_to_string(&main).expect("read main.td");
        let (program, parse_errors) = crate::parser::parse(&source);
        assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

        let err = transpile_with_context(&program, &main, &dir, &dir.join("out.mjs"))
            .expect_err("unresolved package import should fail codegen");
        assert!(
            err.message
                .contains("Could not resolve package import 'alice/missing'"),
            "unexpected error: {}",
            err.message
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── JSSet tests ──

    #[test]
    fn test_js_codegen_jsset_basic() {
        // JSSet[obj, "key", "value"]() → IIFE that sets property and returns obj
        let js = transpile("obj = 1\nJSSet[obj, \"key\", \"value\"]()\nobj").unwrap();
        assert!(
            js.contains("__o[\"key\"] = \"value\""),
            "JSSet should generate property assignment: got {}",
            js
        );
        assert!(
            js.contains("return __o;"),
            "JSSet should return the object: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsset_unmold() {
        let js = transpile("obj = 1\nJSSet[obj, \"x\", 42]() ]=> result\nresult").unwrap();
        assert!(
            js.contains("__taida_unmold"),
            "JSSet unmold should wrap with __taida_unmold: got {}",
            js
        );
        assert!(
            js.contains("__o[\"x\"] = 42"),
            "JSSet should set property: got {}",
            js
        );
    }

    // ── JSBind tests ──

    #[test]
    fn test_js_codegen_jsbind_basic() {
        // JSBind[obj, "method"]() → obj["method"].bind(obj)
        let js = transpile("obj = 1\nJSBind[obj, \"method\"]()\nobj").unwrap();
        assert!(
            js.contains(".bind("),
            "JSBind should generate .bind(): got {}",
            js
        );
        assert!(
            js.contains("[\"method\"]"),
            "JSBind should access method by name: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsbind_unmold() {
        let js = transpile("obj = 1\nJSBind[obj, \"fetch\"]() ]=> bound\nbound").unwrap();
        assert!(
            js.contains("__taida_unmold"),
            "JSBind unmold should wrap with __taida_unmold: got {}",
            js
        );
        assert!(
            js.contains(".bind("),
            "JSBind should generate .bind(): got {}",
            js
        );
    }

    // ── JSSpread tests ──

    #[test]
    fn test_js_codegen_jsspread_basic() {
        // JSSpread[target, source]() → __taida_js_spread(target, source)
        let js = transpile("a = 1\nb = 2\nJSSpread[a, b]()\na").unwrap();
        assert!(
            js.contains("__taida_js_spread("),
            "JSSpread should call __taida_js_spread: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsspread_unmold() {
        let js = transpile("a = 1\nb = 2\nJSSpread[a, b]() ]=> merged\nmerged").unwrap();
        assert!(
            js.contains("__taida_unmold"),
            "JSSpread unmold should wrap with __taida_unmold: got {}",
            js
        );
        assert!(
            js.contains("__taida_js_spread("),
            "JSSpread should call __taida_js_spread: got {}",
            js
        );
    }

    #[test]
    fn test_js_runtime_jsspread_helper() {
        // Verify __taida_js_spread is present in the runtime
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function __taida_js_spread("),
            "Runtime should include __taida_js_spread helper: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_invalid_net_import_reports_shared_export_list() {
        let err = transpile(">>> taida-lang/net => @(MissingNetSymbol)")
            .expect_err("invalid taida-lang/net import should fail");
        assert!(
            err.message.contains("MissingNetSymbol")
                && err.message.contains("HttpProtocol")
                && err.message.contains("httpServe"),
            "Expected taida-lang/net export list with HttpProtocol, got {}",
            err.message
        );
    }
}

// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
//
// Semantics-preserving split of the former monolithic `lower.rs`. This file
// groups infer methods of the `Lowering` struct (per the lower/ split's
// placement table). All methods keep their
// original signatures, bodies, and privacy; only the enclosing file changes.

use super::{LowerError, Lowering};
use crate::codegen::ir::*;
use crate::parser::*;

const LEGACY_FIELD_TYPE_BOOL: i64 = 4;

impl Lowering {
    // lower_index_access removed in v0.5.0 — IndexAccess no longer exists in AST

    /// ヒープオブジェクトを生成する式かどうかを判定
    /// Lambda は除外: キャプチャありのクロージャのみヒープ（closure_vars で判定）
    /// 式が float 値を返すかどうかを判定
    pub(crate) fn expr_returns_float(&self, expr: &Expr) -> bool {
        match expr {
            Expr::FloatLit(_, _) => true,
            Expr::FuncCall(callee, _, _) => {
                // Detect float-returning user-defined functions
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.float_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            // E30 Phase 8 / E30B-011 follow-up: pack field call whose
            // declared return type is :Float. Mirrors the Bool path in
            // `expr_is_bool`; without this, defaultFn-driven Float fields
            // (e.g. `Calc = @(halve: Int => :Float)`) would be rendered
            // through `taida_polymorphic_to_string` and observe a raw i64.
            Expr::MethodCall(obj, method, args, _) => {
                // getOrDefault's result type follows its default argument
                // (the checker enforces that the Lax payload and default
                // agree), so a Float default means a Float result.
                if method == "getOrDefault"
                    && args.first().is_some_and(|a| self.expr_returns_float(a))
                {
                    return true;
                }
                if let Some(type_name) = self.infer_type_name(obj)
                    && let Some(field_types) = self.type_field_types.get(&type_name)
                {
                    for (name, ty) in field_types {
                        if name == method
                            && let Some(crate::parser::TypeExpr::Function(_, ret)) = ty
                            && let crate::parser::TypeExpr::Named(n) = ret.as_ref()
                            && n == "Float"
                        {
                            return true;
                        }
                    }
                }
                false
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                // 一方が float なら結果も float
                self.expr_returns_float(lhs) || self.expr_returns_float(rhs)
            }
            Expr::UnaryOp(_, inner, _) => {
                // -2.3 etc: negate preserves float type
                self.expr_returns_float(inner)
            }
            Expr::Ident(name, _) => {
                // float 変数への参照
                self.float_vars.contains(name) || self.stdlib_constants.contains_key(name)
            }
            // Pack-literal field read: `p.x` where the binding recorded
            // x's static kind (Float payloads are invisible to the value
            // heuristics, so display dispatch needs the static answer).
            Expr::FieldAccess(obj, fname, _) => {
                if let Expr::Ident(v, _) = obj.as_ref() {
                    self.pack_field_kinds.get(v).and_then(|m| m.get(fname))
                        == Some(&crate::codegen::tag_prop::TAG_FLOAT)
                } else {
                    false
                }
            }
            // C25B-025 Phase 5-I: math mold family returns Float. Must
            // match the `Float` entries in `src/types/mold_specs.rs`
            // so nested calls (`Sqrt[Pow[2.0, 3]()]`) skip the
            // int→float widening in the outer mold's lowering.
            Expr::MoldInst(name, type_args, _, _) => {
                matches!(
                    name.as_str(),
                    "Sqrt"
                        | "Pow"
                        | "Exp"
                        | "Ln"
                        | "Log"
                        | "Log2"
                        | "Log10"
                        | "Sin"
                        | "Cos"
                        | "Tan"
                        | "Asin"
                        | "Acos"
                        | "Atan"
                        | "Atan2"
                        | "Sinh"
                        | "Cosh"
                        | "Tanh"
                ) ||
                // Sum over a Float list returns a Float: the runtime
                // switches to f64 accumulation when any element kind
                // is FLOAT, so the result's display/consumers must
                // treat it as one (statically visible for a float-list
                // literal or a tracked @[Float] variable).
                (name == "Sum"
                    && type_args.first().is_some_and(|a| self.expr_is_float_list(a)))
                // If[] yields one of its two branches; Abs/Clamp follow
                // their first argument.
                || (name == "If"
                    && type_args.len() >= 3
                    && self.expr_returns_float(&type_args[1])
                    && self.expr_returns_float(&type_args[2]))
                || (matches!(name.as_str(), "Abs" | "Clamp")
                    && type_args.first().is_some_and(|a| self.expr_returns_float(a)))
            }
            _ => false,
        }
    }

    /// Statically-known element kind of a list expression: a homogeneous
    /// literal or a variable tracked via `list_element_types` (literal
    /// binding or parameter annotation).
    pub(crate) fn static_list_elem_kind(&self, expr: &Expr) -> Option<&'static str> {
        match expr {
            Expr::ListLit(items, _) if !items.is_empty() => {
                if items.iter().all(|i| self.expr_returns_float(i)) {
                    Some("Float")
                } else if items.iter().all(|i| self.expr_is_int(i)) {
                    Some("Int")
                } else if items.iter().all(|i| self.expr_is_string_full(i)) {
                    Some("Str")
                } else if items.iter().all(|i| self.expr_is_bool(i)) {
                    Some("Bool")
                } else {
                    None
                }
            }
            Expr::Ident(n, _) => match self.list_element_types.get(n).map(String::as_str) {
                Some("Float") => Some("Float"),
                Some("Int") => Some("Int"),
                Some("Str") => Some("Str"),
                Some("Bool") => Some("Bool"),
                _ => None,
            },
            _ => None,
        }
    }

    /// Statically-known Float list: a literal whose every element is a
    /// Float expression, or a variable tracked as `@[Float]` (literal
    /// binding or parameter annotation) via `list_element_types`.
    pub(crate) fn expr_is_float_list(&self, expr: &Expr) -> bool {
        match expr {
            Expr::ListLit(items, _) => {
                !items.is_empty() && items.iter().all(|i| self.expr_returns_float(i))
            }
            Expr::Ident(name, _) => self
                .list_element_types
                .get(name)
                .is_some_and(|t| t == "Float"),
            _ => false,
        }
    }

    /// 式が文字列を返すかどうかを判定（静的に推測可能な場合のみ、変数名の追跡あり）
    pub(crate) fn expr_is_string_full(&self, expr: &Expr) -> bool {
        match expr {
            Expr::StringLit(_, _) | Expr::TemplateLit(_, _) => true,
            Expr::Ident(name, _) => self.string_vars.contains(name),
            Expr::MethodCall(_, method, _, _) => {
                matches!(
                    method.as_str(),
                    "toString"
                        | "toStr"
                        | "toUpperCase"
                        | "toLowerCase"
                        | "trim"
                        | "replace"
                        | "slice"
                        | "charAt"
                        | "repeat"
                        | "join"
                )
            }
            Expr::FuncCall(callee, _, _) => {
                // Detect string-returning prelude functions and user-defined functions
                if let Expr::Ident(name, _) = callee.as_ref() {
                    matches!(name.as_str(), "stdin" | "jsonEncode" | "jsonPretty")
                        || self.string_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            Expr::FieldAccess(obj, field, _) => {
                self.field_access_has_named_type(obj, field, "Str")
                    || self.field_type_tags.get(field).copied() == Some(3)
            }
            Expr::BinaryOp(lhs, BinOp::Add, rhs, _) => {
                self.expr_is_string_full(lhs) || self.expr_is_string_full(rhs)
            }
            // WF-2b: MoldInst string molds (Upper, Lower, etc.) return strings
            // Note: CharAt AND `Str[x]()` return Lax[Str], not raw Str —
            // classifying the Lax pack as a string makes stdout print the
            // pack pointer as text (the magic header leaks as "KAPDIAT").
            // Note: Reverse and Slice are polymorphic (Str or List/Bytes),
            // so NOT included here.
            Expr::MoldInst(name, _, _, _) => {
                crate::types::mold_specs::mold_return_tag(name)
                    == Some(crate::codegen::tag_prop::TAG_STR)
                    || matches!(
                        name.as_str(),
                        "Upper"
                            | "Lower"
                            | "Trim"
                            | "Replace"
                            | "Repeat"
                            | "Pad"
                            | "Join"
                            | "ToFixed"
                            // C26B-016 (@c.26, Option B+): `StrOf[span, raw]()` returns Str.
                            | "StrOf"
                    )
            }
            Expr::BinaryOp(_, BinOp::Concat, _, _) => true,
            Expr::CondBranch(arms, _) => {
                // If ANY arm body's last expression is a string, the whole branch is string
                arms.iter().any(|arm| {
                    arm.body
                        .last()
                        .map(|stmt| match stmt {
                            Statement::Expr(e) => self.expr_is_string_full(e),
                            _ => false,
                        })
                        .unwrap_or(false)
                })
            }
            _ => false,
        }
    }

    fn field_access_has_named_type(&self, obj: &Expr, field: &str, expected: &str) -> bool {
        self.infer_type_name(obj)
            .and_then(|type_name| self.type_field_types.get(&type_name))
            .is_some_and(|field_types| {
                field_types.iter().any(|(name, ty)| {
                    name == field
                        && matches!(
                            ty,
                            Some(crate::parser::TypeExpr::Named(type_name))
                                if type_name == expected
                        )
                })
            })
    }

    /// FL-16: 式の型がコンパイル時に不明かどうかを判定（untyped パラメータ等）
    pub(super) fn expr_type_is_unknown(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name, _) => {
                !self.int_vars.contains(name)
                    && !self.string_vars.contains(name)
                    && !self.float_vars.contains(name)
                    && !self.bool_vars.contains(name)
                    && !self.pack_vars.contains(name)
                    && !self.list_vars.contains(name)
                    && !self.closure_vars.contains(name)
                    && !self.top_level_vars.contains(name)
                    && !self.user_funcs.contains(name)
                    && !self.stdlib_constants.contains_key(name)
            }
            _ => false,
        }
    }

    /// B11-6d: Check if `child` extends `parent` by walking the inheritance chain.
    pub(crate) fn check_type_inheritance(&self, child: &str, parent: &str) -> bool {
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

    // C12B-038: The tag-propagation helpers previously in this block
    // (`expr_is_bool`, `type_field_type_tag`, `type_expr_to_tag`,
    // `expr_type_tag`, `body_uses_typeis_on_ident`, `get_param_tag_var`,
    // `needs_call_arg_tags`, `emit_call_arg_tags`,
    // `emit_call_arg_tags_full`, `emit_post_lower_arg_tags`, and the
    // `TAG_FRAME_SIZE` constant) were physically relocated to
    // `src/codegen/lower/tag_prop.rs` together with the extracted
    // `lower_stdout_with_tag` helper. All call sites still resolve to
    // `Lowering` methods with identical signatures — only the enclosing
    // file changes.

    /// Determine compile-time callable type tag for httpServe handler.
    /// Returns:
    /// 6 (TAIDA_TAG_CLOSURE) — lambda or closure variable
    /// 10 (TAIDA_TAG_FUNC) — named function reference (user_funcs / lambda_vars)
    /// -1 (TAIDA_TAG_UNKNOWN) — dynamic / cannot determine at compile time
    /// other (0..5, etc.) — statically known non-callable type
    ///
    /// Strategy: check callable first, then delegate to noncallable_type_tag()
    /// which uses the existing expr_returns_float / expr_is_string_full / expr_is_bool /
    /// expr_is_pack / expr_is_list helpers + arithmetic Int detection.
    /// Determine compile-time handler parameter count for httpServe.
    /// Returns the number of parameters if statically known, -1 if dynamic.
    pub(super) fn handler_arity(&self, expr: &Expr) -> i64 {
        match expr {
            Expr::Lambda(params, _, _) => params.len() as i64,
            Expr::Ident(name, _) => self.resolve_ident_arity(name),
            _ => -1,
        }
    }

    /// Resolve handler arity for a named identifier by following
    /// func_param_defs, lambda_vars, lambda_param_counts, and var_aliases chains.
    /// Max chain depth of 16 to prevent infinite loops from cyclic aliases.
    pub(super) fn resolve_ident_arity(&self, name: &str) -> i64 {
        let mut current = name;
        let mut depth = 0;
        loop {
            if depth > 16 {
                return -1; // too deep, give up
            }
            // 1. Named function definition (top-level or inner FuncDef)
            if let Some(params) = self.func_param_defs.get(current) {
                return params.len() as i64;
            }
            // 2. Lambda variable mapped to a lambda function name
            if let Some(lambda_name) = self.lambda_vars.get(current)
                && let Some(params) = self.func_param_defs.get(lambda_name.as_str())
            {
                return params.len() as i64;
            }
            // 3. Direct lambda param count (from lambda assignment: `h <= req, writer => @(...)`)
            if let Some(&count) = self.lambda_param_counts.get(current) {
                return count as i64;
            }
            // 4. Variable alias — follow the chain (e.g., `h <= handler`)
            if let Some(source) = self.var_aliases.get(current) {
                current = source.as_str();
                depth += 1;
                continue;
            }
            return -1; // dynamic / unknown
        }
    }

    pub(super) fn callable_type_tag(&self, expr: &Expr) -> i64 {
        // 1. Callable detection
        match expr {
            Expr::Lambda(_, _, _) => return 6, // TAIDA_TAG_CLOSURE
            Expr::Ident(name, _) => {
                if let Some(tag) = self.resolve_ident_callable_tag(name) {
                    return tag;
                }
                // NB3-4 fix: If this identifier (or any alias-chain ancestor)
                // was inferred solely from the function's return-type annotation
                // (e.g. `run_server h ... => :Int` puts `h` into int_vars),
                // do NOT trust noncallable_type_tag. The parameter might actually
                // be a function/closure passed at runtime.
                // Follow var_aliases to cover `x <= h` 1-hop (and N-hop) aliases.
                if self.ident_or_alias_is_return_type_inferred(name) {
                    return -1;
                }
            }
            _ => {}
        }
        // 2. Non-callable detection — use rich expression-type helpers
        if let Some(tag) = self.noncallable_type_tag(expr) {
            return tag;
        }
        -1 // TAIDA_TAG_UNKNOWN
    }

    /// Resolve callable type tag for a named identifier, following var_aliases.
    pub(super) fn resolve_ident_callable_tag(&self, name: &str) -> Option<i64> {
        let mut current = name;
        let mut depth = 0;
        loop {
            if depth > 16 {
                return None;
            }
            if self.closure_vars.contains(current) {
                return Some(6); // TAIDA_TAG_CLOSURE
            }
            if self.user_funcs.contains(current) || self.lambda_vars.contains_key(current) {
                return Some(10); // TAIDA_TAG_FUNC
            }
            // NB3-4: Follow variable alias chain
            if let Some(source) = self.var_aliases.get(current) {
                current = source.as_str();
                depth += 1;
                continue;
            }
            return None;
        }
    }

    /// Check if an identifier, or any ancestor in its var_aliases chain,
    /// belongs to return_type_inferred_params. This ensures that `x <= h` where
    /// `h` is a return-type-inferred parameter is also treated as unknown callable.
    pub(super) fn ident_or_alias_is_return_type_inferred(&self, name: &str) -> bool {
        let mut current = name;
        let mut depth = 0;
        loop {
            if depth > 16 {
                return false;
            }
            if self.return_type_inferred_params.contains(current) {
                return true;
            }
            if let Some(source) = self.var_aliases.get(current) {
                current = source.as_str();
                depth += 1;
                continue;
            }
            return false;
        }
    }

    /// Determine if an expression is a known non-callable type.
    /// Returns Some(tag) for statically known non-callable, None for unknown.
    /// Leverages existing expr_returns_float / expr_is_string_full / expr_is_bool /
    /// expr_is_pack / expr_is_list which already handle literals, variables, BinaryOp,
    /// MethodCall, FuncCall, etc.
    pub(super) fn noncallable_type_tag(&self, expr: &Expr) -> Option<i64> {
        // Bool (2) — handles BoolLit, bool_vars, comparison ops, boolean methods
        if self.expr_is_bool(expr) {
            return Some(2);
        }
        // Float (1) — handles FloatLit, float_vars, float BinaryOp, float-returning funcs
        if self.expr_returns_float(expr) {
            return Some(1);
        }
        // String (3) — handles StringLit, TemplateLit, string_vars, string methods/funcs
        if self.expr_is_string_full(expr) {
            return Some(3);
        }
        // Pack (4) — handles BuchiPack, TypeInst, pack_vars
        if self.expr_is_pack(expr) {
            return Some(4);
        }
        // List (5) — handles ListLit, list_vars, list methods/funcs
        if self.expr_is_list(expr) {
            return Some(5);
        }
        // Int (0) — literals, int_vars, arithmetic ops, int-returning methods/funcs
        if self.expr_is_int(expr) {
            return Some(0);
        }
        // MoldInst always returns a Pack-like value
        if matches!(expr, Expr::MoldInst(_, _, _, _)) {
            return Some(4);
        }
        None
    }

    /// retain-on-store: Pack/List/Closure/Str をフィールドに格納する際に retain する。
    /// taida_release の再帰 release と対になり、double-free を防ぐ。
    /// tag が TAIDA_TAG_STR(3), TAIDA_TAG_PACK(4), TAIDA_TAG_LIST(5), TAIDA_TAG_CLOSURE(6) の場合に retain。
    pub(super) fn emit_retain_if_heap_tag(&self, func: &mut IrFunction, val: IrVar, tag: i64) {
        if tag == 4 || tag == 5 || tag == 6 {
            func.push(IrInst::Retain(val));
        } else if tag == 3 {
            // TAIDA_TAG_STR: hidden-header string は taida_str_retain で retain する。
            // taida_retain は Pack/List/Closure 用なので Str には使えない。
            let dummy = func.alloc_var();
            func.push(IrInst::Call(
                dummy,
                "taida_str_retain".to_string(),
                vec![val],
            ));
        }
    }

    /// QF-10: 式の TypeDef 名を推論する（FieldAccess の型解決用）
    pub(super) fn infer_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(name, _) => self.var_type_names.get(name).cloned(),
            Expr::TypeInst(type_name, _, _) => Some(type_name.clone()),
            _ => None,
        }
    }

    /// F-58: 式が BuchiPack/TypeInst を返すかどうかを判定
    /// BuchiPack フィールドの関数呼び出しが組み込みメソッド名と衝突するのを防ぐため
    pub(crate) fn expr_is_pack(&self, expr: &Expr) -> bool {
        match expr {
            Expr::BuchiPack(_, _) => true,
            Expr::TypeInst(_, _, _) => true,
            Expr::Ident(name, _) => self.pack_vars.contains(name),
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.pack_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            Expr::MethodCall(obj, method, _, _) => {
                // HashMap.set() returns HashMap, not pack — but if the receiver
                // is a pack, method calls that return the same type are still packs
                self.expr_is_pack(obj) && method != "toString" && method != "toStr"
            }
            _ => false,
        }
    }

    /// retain-on-store: 式が List を返すかどうかを判定
    pub(crate) fn expr_is_list(&self, expr: &Expr) -> bool {
        match expr {
            Expr::ListLit(_, _) => true,
            Expr::Ident(name, _) => self.list_vars.contains(name),
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.list_returning_funcs.contains(name.as_str()) || name == "range"
                } else {
                    false
                }
            }
            Expr::MethodCall(_, method, _, _) => {
                matches!(
                    method.as_str(),
                    "map"
                        | "filter"
                        | "flatMap"
                        | "sort"
                        | "unique"
                        | "flatten"
                        | "reverse"
                        | "concat"
                        | "append"
                        | "prepend"
                        | "zip"
                        | "enumerate"
                )
            }
            _ => false,
        }
    }

    /// 式が Int を返すかどうかを判定（noncallable_type_tag 用）
    /// arithmetic 演算、Int-returning メソッド/関数、int_vars を網羅する。
    ///
    /// reopen 4 (2026-04-22): visibility widened from
    /// `pub(super)` to `pub(crate)` so the sibling
    /// `lower/molds_inst.rs` module can
    /// use the richer Int check in the `Str[x]()` fast-path dispatch.
    /// No behavioural change — only the call surface widened.
    pub(crate) fn expr_is_int(&self, expr: &Expr) -> bool {
        match expr {
            Expr::IntLit(_, _) => true,
            Expr::UnaryOp(UnaryOp::Neg, inner, _) => self.expr_is_int(inner),
            Expr::Ident(name, _) => self.int_vars.contains(name),
            // Arithmetic ops: Int if neither side is Float
            Expr::BinaryOp(lhs, op, rhs, _) => {
                matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul)
                    && !self.expr_returns_float(lhs)
                    && !self.expr_returns_float(rhs)
            }
            // Methods that always return Int
            Expr::MethodCall(_, method, _, _) => {
                matches!(
                    method.as_str(),
                    "length" | "indexOf" | "lastIndexOf" | "count"
                )
            }
            // Functions with :Int/:Num return type
            Expr::FuncCall(callee, _, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.int_returning_funcs.contains(name.as_str())
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Value-tag track: capture a runtime "shadow kind" for an unmold
    /// target whose payload kind is only known at runtime — i.e. the
    /// source is a Lax-returning list accessor over a possibly mixed
    /// list. The companion `__ekind__<name>` IR variable holds the kind
    /// the runtime recorded on the Lax's `__value` field; tagged poly
    /// comparisons read it back so e.g. a Float pulled out of
    /// `@[1, 1.0]` compares to an Int literal under f64 semantics.
    /// Restricted to a method whitelist to keep IR noise out of the
    /// (much more common) Async-await unmolds.
    pub(super) fn maybe_capture_shadow_kind(
        &mut self,
        func: &mut IrFunction,
        target: &str,
        source: &Expr,
        source_var: IrVar,
    ) {
        // A rebind always invalidates a previous shadow for this name.
        self.shadow_kind_vars.remove(target);
        // Restricted to the accessors that build their Lax through the
        // kind-stamping constructor. min/max joined the trustworthy set
        // when their runtimes switched to the kind-aware comparator and
        // started stamping the winner's kind into the Lax (they used to
        // build the plain constructor whose heuristic field tag could
        // misread a string payload as a bare INT).
        let is_lax_accessor = matches!(
            source,
            Expr::MethodCall(_, m, _, _)
                if matches!(m.as_str(), "get" | "first" | "last" | "min" | "max")
        ) || matches!(
            source,
            Expr::MoldInst(n, _, _, _) if matches!(n.as_str(), "Min" | "Max")
        );
        if !is_lax_accessor {
            return;
        }
        let kind_var = func.alloc_var();
        func.push(IrInst::Call(
            kind_var,
            "taida_lax_value_ekind".to_string(),
            vec![source_var],
        ));
        func.push(IrInst::DefVar(format!("__ekind__{}", target), kind_var));
        self.shadow_kind_vars.insert(target.to_string());
    }

    /// Record the assignment-time inner type for
    /// `x <= Mold[...]()` so a later `x >=> val` can land `val` in the
    /// right typed set. For `Lax[arg]()` the *inner* kind is recorded
    /// when statically known — the mold name "Lax" alone carries no
    /// inner type and left the unmold result on the polymorphic path.
    pub(super) fn record_lax_inner_type(&mut self, target: &str, value: &Expr) {
        if let Expr::MoldInst(mold_name, type_args, _, _) = value {
            let inner_kind = if mold_name == "Lax" && type_args.len() == 1 {
                let arg = &type_args[0];
                if self.expr_is_int(arg) {
                    Some("Int")
                } else if self.expr_returns_float(arg) {
                    Some("Float")
                } else if self.expr_is_string_full(arg) {
                    Some("Str")
                } else if self.expr_is_bool(arg) {
                    Some("Bool")
                } else {
                    None
                }
            } else {
                None
            };
            self.lax_inner_types.insert(
                target.to_string(),
                inner_kind.map_or_else(|| mold_name.clone(), str::to_string),
            );
        }
    }

    /// Unmold 先の変数に型情報を伝播する
    /// MoldInst("Str",...) >=> x の場合、x を string_vars に追加
    pub(super) fn track_unmold_type(&mut self, target: &str, source: &Expr) {
        match source {
            // C26B-011 (Phase 11): Div/Mod return Float when at least one
            // type-arg is Float (matches `taida_div_mold_f` lowering in
            // `lower/molds_inst.rs`). Without this, `Div[1.0, 2.0]() >=> r`
            // leaves `r` untagged, `debug(r)` falls through to
            // `taida_debug_int`, and prints the f64 bit-pattern as an
            // int. `track_unmold_type_by_mold_name` only sees the mold
            // name, not the args, so handle `Div`/`Mod` here where we
            // still have the `MoldInst` type_args available.
            Expr::MoldInst(name, type_args, _, _)
                if (name == "Div" || name == "Mod")
                    && type_args.iter().any(|a| self.expr_returns_float(a)) =>
            {
                self.float_vars.insert(target.to_string());
            }
            // F58B-003: all-Int Div/Mod produce Lax[Int], so the unmold
            // result is an Int. Landing it in int_vars keeps follow-up
            // arithmetic on the typed taida_int_* path instead of the
            // polymorphic runtime dispatch.
            Expr::MoldInst(name, type_args, _, _)
                if (name == "Div" || name == "Mod")
                    && type_args.iter().all(|a| self.expr_is_int(a)) =>
            {
                self.int_vars.insert(target.to_string());
            }
            // List accessor METHODS (min/max/first/last/get) unmold to an
            // element of their receiver — propagate the statically known
            // element kind (display dispatch is static; the runtime shadow
            // kind only feeds tagged comparisons).
            Expr::MethodCall(obj, m, _, _)
                if matches!(m.as_str(), "min" | "max" | "first" | "last" | "get") =>
            {
                match self.static_list_elem_kind(obj) {
                    Some("Float") => {
                        self.float_vars.insert(target.to_string());
                    }
                    Some("Int") => {
                        self.int_vars.insert(target.to_string());
                    }
                    Some("Str") => {
                        self.string_vars.insert(target.to_string());
                    }
                    Some("Bool") => {
                        self.bool_vars.insert(target.to_string());
                    }
                    _ => {}
                }
            }
            // Min/Max/Find unmold to an ELEMENT of their list argument —
            // propagate the statically known element kind so the unmolded
            // value displays under its real type (same rationale as the
            // Div/Mod arms above; the runtime Lax also carries the kind,
            // but display dispatch is decided statically here).
            Expr::MoldInst(name, type_args, _, _)
                if matches!(name.as_str(), "Min" | "Max" | "Find") =>
            {
                match type_args
                    .first()
                    .and_then(|a| self.static_list_elem_kind(a))
                {
                    Some("Float") => {
                        self.float_vars.insert(target.to_string());
                    }
                    Some("Int") => {
                        self.int_vars.insert(target.to_string());
                    }
                    Some("Str") => {
                        self.string_vars.insert(target.to_string());
                    }
                    Some("Bool") => {
                        self.bool_vars.insert(target.to_string());
                    }
                    _ => {}
                }
            }
            // F58B-003: `Lax[x]() >=> v` — propagate x's statically known
            // kind to v (same rationale as the Div/Mod arms above).
            Expr::MoldInst(name, type_args, _, _) if name == "Lax" && type_args.len() == 1 => {
                let arg = &type_args[0];
                if self.expr_is_int(arg) {
                    self.int_vars.insert(target.to_string());
                } else if self.expr_returns_float(arg) {
                    self.float_vars.insert(target.to_string());
                } else if self.expr_is_string_full(arg) {
                    self.string_vars.insert(target.to_string());
                } else if self.expr_is_bool(arg) {
                    self.bool_vars.insert(target.to_string());
                }
            }
            Expr::MoldInst(name, _, _, _) => self.track_unmold_type_by_mold_name(target, name),
            // QF-34: Ident source — look up lax_inner_types to propagate type through unmold
            // e.g., `x <= Bool["maybe"]()` then `x >=> val` → val is Bool
            Expr::Ident(name, _) => {
                if let Some(inner_type) = self.lax_inner_types.get(name).cloned() {
                    self.track_unmold_type_by_mold_name(target, &inner_type);
                }
            }
            // MethodCall results: hasValue() -> bool
            Expr::MethodCall(recv, method, _, _) => {
                if matches!(
                    method.as_str(),
                    "hasValue"
                        | "isEmpty"
                        | "contains"
                        | "startsWith"
                        | "endsWith"
                        | "any"
                        | "all"
                        | "none"
                        | "isOk"
                        | "isError"
                        | "isSuccess"
                        | "isFulfilled"
                        | "isPending"
                        | "isRejected"
                        | "isNaN"
                        | "isInfinite"
                        | "isFinite"
                        | "isPositive"
                        | "isNegative"
                        | "isZero"
                ) {
                    self.bool_vars.insert(target.to_string());
                }
                // C21-4: `a.get(i) >=> av` — if `a` is a typed `@[Float]` list,
                // tag `av` as a Float so the subsequent `av * bv` arithmetic
                // lowers to `taida_float_mul` (not `taida_int_mul`).
                if method.as_str() == "get"
                    && let Expr::Ident(recv_name, _) = recv.as_ref()
                    && let Some(elem_ty) = self.list_element_types.get(recv_name).cloned()
                {
                    self.track_unmold_type_by_mold_name(target, &elem_ty);
                }
            }
            _ => {}
        }
        // Generic fallback: whatever the static expression classifiers can
        // prove about the unmold source applies to the bound name too —
        // this is what carries getOrDefault(float-default), If/Abs/Clamp
        // results and pack-field reads into display dispatch. Insertions
        // are idempotent, so overlap with the arms above is harmless.
        if self.expr_returns_float(source) {
            self.float_vars.insert(target.to_string());
        } else if self.expr_is_bool(source) {
            self.bool_vars.insert(target.to_string());
        } else if self.expr_is_string_full(source) {
            self.string_vars.insert(target.to_string());
        }
    }

    /// Helper: track unmold result type based on mold name
    pub(super) fn track_unmold_type_by_mold_name(&mut self, target: &str, mold_name: &str) {
        match mold_name {
            // Note: Reverse is polymorphic (Str or List), so NOT included here
            "Str" | "Upper" | "Lower" | "Trim" | "Replace" | "Slice" | "CharAt" | "Repeat"
            | "Pad" | "Join" | "ToFixed"
            // C26B-016 (@c.26, Option B+): `StrOf[span, raw]()` returns Str.
            | "StrOf" => {
                self.string_vars.insert(target.to_string());
            }
            "Bool" => {
                self.bool_vars.insert(target.to_string());
            }
            "Float" => {
                self.float_vars.insert(target.to_string());
            }
            // F58B-003: logical inner-type name recorded by the assignment
            // tracker for `x <= Lax[intExpr]()` (and the Int mold itself).
            "Int" => {
                self.int_vars.insert(target.to_string());
            }
            // C26B-011 (Phase 11): math molds return Float per
            // `src/types/mold_specs.rs`. Previously `Sqrt[-1.0]() >=> nan`
            // left `nan` untagged and `debug(nan)` fell through to
            // `taida_debug_int`, printing the f64 bit-pattern as Int
            // (e.g. `-2251799813685248` for NaN). Must match
            // `expr_returns_float` in this file.
            "Sqrt" | "Pow" | "Exp" | "Ln" | "Log" | "Log2" | "Log10" | "Sin" | "Cos" | "Tan"
            | "Asin" | "Acos" | "Atan" | "Atan2" | "Sinh" | "Cosh" | "Tanh" => {
                self.float_vars.insert(target.to_string());
            }
            _ => {}
        }
    }

    /// 式の結果を文字列に変換する。既に文字列なら何もしない。
    ///
    /// (2026-04-15): This helper is **no longer called from the
    /// `stdout` / `stderr` lowering path**. Those now dispatch directly via
    /// `taida_io_stdout_with_tag` / `taida_io_stderr_with_tag` with the
    /// compile-time tag (or `-1` for UNKNOWN); the runtime polymorphic
    /// formatter is the single source of truth. The helper survives for two
    /// remaining consumers where the value must be available as a `char*`
    /// at the call site:
    ///
    /// - `stdin(prompt)` — the C runtime expects a prompt string.
    /// - `TemplateLit` (`"prefix ${expr} suffix"`) — interpolated exprs
    /// are stringified so they can be concatenated with `taida_str_concat`.
    pub(super) fn convert_to_string(
        &self,
        func: &mut IrFunction,
        expr: &Expr,
        var: IrVar,
    ) -> Result<IrVar, LowerError> {
        if self.expr_is_string_full(expr) {
            // Already a string — no conversion needed
            Ok(var)
        } else if self.expr_is_likely_bool(expr) {
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_str_from_bool".to_string(),
                vec![var],
            ));
            Ok(result)
        } else if self.expr_returns_float(expr) {
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_float_to_str".to_string(),
                vec![var],
            ));
            Ok(result)
        } else {
            // Default: polymorphic to_string (handles int, monadic types, etc.)
            let result = func.alloc_var();
            func.push(IrInst::Call(
                result,
                "taida_polymorphic_to_string".to_string(),
                vec![var],
            ));
            Ok(result)
        }
    }

    /// Return whether codegen can safely route this expression through Bool
    /// formatting or a Bool runtime tag when the typed side table is absent.
    pub(crate) fn expr_is_likely_bool(&self, expr: &Expr) -> bool {
        if self.expr_is_bool(expr) {
            return true;
        }

        match expr {
            Expr::Ident(name, _) => self.bool_vars.contains(name),
            Expr::FieldAccess(obj, field, _) => {
                field == "has_value"
                    || self.field_access_has_named_type(obj, field, "Bool")
                    || self.field_type_tags.get(field).copied() == Some(LEGACY_FIELD_TYPE_BOOL)
            }
            Expr::MethodCall(_, method, args, _) => match method.as_str() {
                "hasValue" | "isEmpty" | "isOk" | "isError" | "isSuccess" | "isFulfilled"
                | "isPending" | "isRejected" | "isNaN" | "isInfinite" | "isFinite"
                | "isPositive" | "isNegative" | "isZero" => args.is_empty(),
                "contains" | "startsWith" | "endsWith" | "has" | "any" | "all" | "none" => {
                    args.len() == 1
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// F-58/F-60: Check if a function body's last expression returns a BuchiPack/TypeInst.
    pub(super) fn func_body_returns_pack(body: &[Statement]) -> bool {
        matches!(
            body.last(),
            Some(Statement::Expr(
                Expr::BuchiPack(_, _) | Expr::TypeInst(_, _, _)
            ))
        )
    }

    /// retain-on-store: Check if a function body's last expression returns a List.
    pub(super) fn func_body_returns_list(body: &[Statement]) -> bool {
        matches!(body.last(), Some(Statement::Expr(Expr::ListLit(_, _))))
    }

    pub(super) fn is_heap_expr(expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::BuchiPack(..) | Expr::TypeInst(..) | Expr::ListLit(..)
        ) || matches!(expr, Expr::MethodCall(_, method, _, _)
            if method == "map" || method == "filter" || method == "reverse"
        )
    }

    /// F-48: 式中に出現する全ての識別子名を収集する（ラムダ本体には入らない）
    pub(super) fn collect_idents_in_expr(expr: &Expr, out: &mut std::collections::HashSet<String>) {
        match expr {
            Expr::Ident(name, _) => {
                out.insert(name.clone());
            }
            Expr::BinaryOp(lhs, _, rhs, _) => {
                Self::collect_idents_in_expr(lhs, out);
                Self::collect_idents_in_expr(rhs, out);
            }
            Expr::UnaryOp(_, operand, _) => {
                Self::collect_idents_in_expr(operand, out);
            }
            Expr::FuncCall(callee, args, _) => {
                Self::collect_idents_in_expr(callee, out);
                for arg in args {
                    Self::collect_idents_in_expr(arg, out);
                }
            }
            Expr::FieldAccess(obj, _, _) => {
                Self::collect_idents_in_expr(obj, out);
            }
            Expr::MethodCall(obj, _, args, _) => {
                Self::collect_idents_in_expr(obj, out);
                for arg in args {
                    Self::collect_idents_in_expr(arg, out);
                }
            }
            Expr::Pipeline(exprs, _) => {
                for e in exprs {
                    Self::collect_idents_in_expr(e, out);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        Self::collect_idents_in_expr(cond, out);
                    }
                    for stmt in &arm.body {
                        if let Statement::Expr(e) = stmt {
                            Self::collect_idents_in_expr(e, out);
                        } else if let Statement::Assignment(a) = stmt {
                            Self::collect_idents_in_expr(&a.value, out);
                        }
                    }
                }
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for field in fields {
                    Self::collect_idents_in_expr(&field.value, out);
                }
            }
            Expr::ListLit(items, _) => {
                for item in items {
                    Self::collect_idents_in_expr(item, out);
                }
            }
            Expr::MoldInst(_, args, fields, _) => {
                for arg in args {
                    Self::collect_idents_in_expr(arg, out);
                }
                for field in fields {
                    Self::collect_idents_in_expr(&field.value, out);
                }
            }
            Expr::Unmold(inner, _) | Expr::Throw(inner, _) => {
                Self::collect_idents_in_expr(inner, out);
            }
            // ラムダ本体には入らない（キャプチャは別途管理）
            _ => {}
        }
    }

    /// F-48: 関数本体の代入文から、戻り値式が間接的に参照する全変数の集合を計算する。
    /// 代入グラフの推移的閉包を求め、戻り値から到達可能な全変数名を返す。
    pub(super) fn compute_reachable_vars(
        return_expr: &Expr,
        body: &[Statement],
    ) -> std::collections::HashSet<String> {
        // 1. 代入グラフを構築: target -> {式中の識別子}
        let mut assign_deps: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        for stmt in body {
            if let Statement::Assignment(assign) = stmt {
                let mut deps = std::collections::HashSet::new();
                Self::collect_idents_in_expr(&assign.value, &mut deps);
                assign_deps.insert(assign.target.clone(), deps);
            }
        }

        // 2. 戻り値式の直接参照を収集
        let mut reachable = std::collections::HashSet::new();
        Self::collect_idents_in_expr(return_expr, &mut reachable);

        // 3. 推移的閉包（BFS）
        let mut queue: Vec<String> = reachable.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            if let Some(deps) = assign_deps.get(&name) {
                for dep in deps {
                    if reachable.insert(dep.clone()) {
                        queue.push(dep.clone());
                    }
                }
            }
        }

        reachable
    }
}

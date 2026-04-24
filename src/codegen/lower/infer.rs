// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
//
// Semantics-preserving split of the former monolithic `lower.rs`. This file
// groups infer methods of the `Lowering` struct (placement table §2 of
// `.dev/taida-logs/docs/design/file_boundaries.md`). All methods keep their
// original signatures, bodies, and privacy; only the enclosing file changes.

use super::{LowerError, Lowering};
use crate::codegen::ir::*;
use crate::parser::*;

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
            // C25B-025 Phase 5-I: math mold family returns Float. Must
            // match the `Float` entries in `src/types/mold_returns.rs`
            // so nested calls (`Sqrt[Pow[2.0, 3]()]`) skip the
            // int→float widening in the outer mold's lowering.
            Expr::MoldInst(name, _, _, _) => {
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
                )
            }
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
            Expr::BinaryOp(lhs, BinOp::Add, rhs, _) => {
                self.expr_is_string_full(lhs) || self.expr_is_string_full(rhs)
            }
            // WF-2b: MoldInst string molds (Upper, Lower, etc.) return strings
            // Note: CharAt returns Lax[Str], not raw Str (TF-15)
            // Note: Reverse is polymorphic (Str or List), so NOT included here
            Expr::MoldInst(name, _, _, _) => matches!(
                name.as_str(),
                "Str"
                    | "Upper"
                    | "Lower"
                    | "Trim"
                    | "Replace"
                    | "Slice"
                    | "Repeat"
                    | "Pad"
                    | "Join"
                    | "ToFixed"
                    // C26B-016 (@c.26, Option B+): `StrOf[span, raw]()` returns Str.
                    | "StrOf"
            ),
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

    /// NB-31: Determine compile-time callable type tag for httpServe handler.
    /// Returns:
    ///   6  (TAIDA_TAG_CLOSURE) — lambda or closure variable
    ///  10  (TAIDA_TAG_FUNC)    — named function reference (user_funcs / lambda_vars)
    ///  -1  (TAIDA_TAG_UNKNOWN) — dynamic / cannot determine at compile time
    ///   other (0..5, etc.)     — statically known non-callable type
    ///
    /// Strategy: check callable first, then delegate to noncallable_type_tag()
    /// which uses the existing expr_returns_float / expr_is_string_full / expr_is_bool /
    /// expr_is_pack / expr_is_list helpers + arithmetic Int detection.
    /// NET3-5a: Determine compile-time handler parameter count for httpServe.
    /// Returns the number of parameters if statically known, -1 if dynamic.
    pub(super) fn handler_arity(&self, expr: &Expr) -> i64 {
        match expr {
            Expr::Lambda(params, _, _) => params.len() as i64,
            Expr::Ident(name, _) => self.resolve_ident_arity(name),
            _ => -1,
        }
    }

    /// NB3-4: Resolve handler arity for a named identifier by following
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

    /// NB3-4: Resolve callable type tag for a named identifier, following var_aliases.
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

    /// NB3-4: Check if an identifier, or any ancestor in its var_aliases chain,
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

    /// NB-31: Determine if an expression is a known non-callable type.
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
    pub(super) fn expr_is_list(&self, expr: &Expr) -> bool {
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

    /// NB-31: 式が Int を返すかどうかを判定（noncallable_type_tag 用）
    /// arithmetic 演算、Int-returning メソッド/関数、int_vars を網羅する。
    ///
    /// C23B-003 reopen 4 (2026-04-22): visibility widened from
    /// `pub(super)` to `pub(crate)` so the sibling `lower_molds.rs`
    /// module (`src/codegen/lower_molds.rs`, not under `lower/`) can
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

    /// Unmold 先の変数に型情報を伝播する
    /// MoldInst("Str", ...) ]=> x の場合、x を string_vars に追加
    pub(super) fn track_unmold_type(&mut self, target: &str, source: &Expr) {
        match source {
            // C26B-011 (Phase 11): Div/Mod return Float when at least one
            // type-arg is Float (matches `taida_div_mold_f` lowering in
            // `lower_molds.rs`). Without this, `Div[1.0, 2.0]() ]=> r`
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
            Expr::MoldInst(name, _, _, _) => self.track_unmold_type_by_mold_name(target, name),
            // QF-34: Ident source — look up lax_inner_types to propagate type through unmold
            // e.g., `x <= Bool["maybe"]()` then `x ]=> val` → val is Bool
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
                // C21-4: `a.get(i) ]=> av` — if `a` is a typed `@[Float]` list,
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
            // C26B-011 (Phase 11): math molds return Float per
            // `src/types/mold_returns.rs`. Previously `Sqrt[-1.0]() ]=> nan`
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
    /// C12B-016 (2026-04-15): This helper is **no longer called from the
    /// `stdout` / `stderr` lowering path**. Those now dispatch directly via
    /// `taida_io_stdout_with_tag` / `taida_io_stderr_with_tag` with the
    /// compile-time tag (or `-1` for UNKNOWN); the runtime polymorphic
    /// formatter is the single source of truth. The helper survives for two
    /// remaining consumers where the value must be available as a `char*`
    /// at the call site:
    ///
    /// - `stdin(prompt)` — the C runtime expects a prompt string.
    /// - `TemplateLit` (`"prefix ${expr} suffix"`) — interpolated exprs
    ///   are stringified so they can be concatenated with `taida_str_concat`.
    pub(super) fn convert_to_string(
        &self,
        func: &mut IrFunction,
        expr: &Expr,
        var: IrVar,
    ) -> Result<IrVar, LowerError> {
        if self.expr_is_string_full(expr) {
            // Already a string — no conversion needed
            Ok(var)
        } else if self.expr_is_bool(expr) {
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

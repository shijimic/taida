//! check — methods split out of the TypeChecker impl.
//! Pure move from the parent module; behaviour unchanged.

use crate::lexer::Span;
use crate::parser::*;
use crate::types::Type;

use super::{
    BranchInfo, CageBranch, MAX_CALL_ARGUMENTS, RESERVED_INTERNAL_FIELD_PREFIX, TypeChecker,
    TypeError,
};

impl TypeChecker {
    /// Run the `mutual-recursion` verify check and surface any findings as
    /// [`TypeError`]s attached to the checker. See
    /// `src/graph/verify.rs::check_mutual_recursion` for the detection
    /// semantics.
    pub(super) fn check_mutual_recursion_errors(&mut self, program: &Program) {
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

    pub(super) fn check_mold_errors_in_stmt(&mut self, stmt: &Statement) {
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

    pub(super) fn check_mold_errors_in_expr(&mut self, expr: &Expr) {
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
    pub(super) fn check_pinned_field_access_in_expr(&mut self, expr: &Expr) {
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

    pub(super) fn check_str_plus_known_non_str_in_expr(&mut self, expr: &Expr) {
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

    pub(super) fn check_comparison_errors_in_template(&mut self, template: &str, span: &Span) {
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
                    // F56: interpolating a sealed carrier (`` `${secret}` `` or a
                    // `${@(token <= secret)}` literal) is a display sink. The
                    // interpolated parts are not in the AST (TemplateLit holds the
                    // raw string), so check the freshly parsed `${...}` expression
                    // here — directly and one level into a literal. Side-effect-free.
                    if let Some(carrier) = self
                        .first_direct_sealed_operand(&parsed_expr)
                        .or_else(|| self.first_nested_sealed_in_literal(&parsed_expr))
                    {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1533] string interpolation cannot display a sealed carrier \
                                 ({}); the secret value would be exposed. Hint: interpolate \
                                 `Redact[secret]()` for a masked string instead.",
                                carrier
                            ),
                            span: span.clone(),
                        });
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

    /// Type-check a statement (second pass).
    pub(super) fn check_statement(&mut self, stmt: &Statement) {
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

                // [E1537]: `Num` is a constraint marker, not a value type.
                // The constraints reference states it is not an independent
                // wire value type; allowing `=> :Num` / `x: Num` annotations
                // made the lowering guess a representation (it registered the
                // function as Int-returning), so a Float body rendered as a
                // raw f64 bit pattern on the compiled backends. Constraint
                // position (`[T <= :Num]`) and type-query position
                // (`TypeIs[x, :Num]`) remain valid.
                if fd.return_type.is_some() && Self::contains_constraint_marker_type(&ret_ty) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1537] Function '{}' declares return type {}, but Num is a generic-constraint marker, not a value type. Annotate the concrete type instead (`:Int` or `:Float`), or make the function generic with `[T <= :Num] ... => :T`. See docs/reference/type_constraints.md and docs/reference/diagnostic_codes.md [E1537].",
                            fd.name, ret_ty
                        ),
                        span: fd.span.clone(),
                    });
                }
                for (idx, param) in fd.params.iter().enumerate() {
                    if param.type_annotation.is_some()
                        && let Some(pty) = param_types.get(idx)
                        && Self::contains_constraint_marker_type(pty)
                    {
                        self.errors.push(TypeError {
                            message: format!(
                                "[E1537] Function '{}' parameter '{}' has type annotation {}, but Num is a generic-constraint marker, not a value type. Annotate the concrete type instead (`:Int` or `:Float`), or make the function generic with `[T <= :Num] {}: T`. See docs/reference/type_constraints.md and docs/reference/diagnostic_codes.md [E1537].",
                                fd.name, param.name, pty, param.name
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
                self.reject_sealed_carrier_unmold(&source_ty, &uf.span);
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
                self.reject_sealed_carrier_unmold(&source_ty, &ub.span);
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

    /// Check a condition branch expression (extracted from `infer_expr_type`).
    ///
    /// Validates that:
    /// - All arm conditions are Bool (E1604)
    /// - All arms return compatible types (E1603)
    pub(super) fn check_cond_branch(&mut self, arms: &[CondArm], span: &Span) -> Type {
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

impl TypeChecker {
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
    pub(super) fn run_comparison_error_walk(&mut self, expr: &Expr) {
        if self.in_comparison_error_walk {
            return;
        }
        self.in_comparison_error_walk = true;
        self.check_comparison_errors_in_expr(expr);
        self.in_comparison_error_walk = false;
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

    pub(super) fn func_call_args_need_comparison_walk(&self, func: &Expr, args: &[Expr]) -> bool {
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
    pub(super) fn emit_comparison_mismatch_if_needed(
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
}

impl TypeChecker {
    /// [E1539] Top-level executable statements must not reference a
    /// function that is only defined later in the file. The interpreter
    /// executes statements in order — a forward call is an
    /// undefined-variable error at runtime — while the compiled
    /// backends hoist definitions and would silently succeed, so
    /// program success would depend on the backend. References from
    /// inside function and lambda bodies resolve at call time and stay
    /// legal (that is how mutual recursion is written).
    pub(super) fn check_toplevel_forward_function_references(&mut self, program: &Program) {
        let mut later_funcs: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for stmt in &program.statements {
            if let Statement::FuncDef(fd) = stmt {
                later_funcs.insert(fd.name.as_str());
            }
        }
        if later_funcs.is_empty() {
            return;
        }
        let mut defined: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for stmt in &program.statements {
            match stmt {
                Statement::FuncDef(fd) => {
                    defined.insert(fd.name.as_str());
                }
                Statement::Import(imp) => {
                    // Imported names are bound when the import executes;
                    // a same-named later FuncDef must not flag uses in
                    // between.
                    for sym in &imp.symbols {
                        defined.insert(sym.alias.as_deref().unwrap_or(sym.name.as_str()));
                    }
                }
                Statement::Assignment(a) => {
                    self.scan_expr_forward_refs(&a.value, &later_funcs, &defined);
                    // A top-level value binding shadows a later function
                    // of the same name for subsequent statements.
                    defined.insert(a.target.as_str());
                }
                Statement::Expr(e) => self.scan_expr_forward_refs(e, &later_funcs, &defined),
                Statement::UnmoldForward(u) => {
                    self.scan_expr_forward_refs(&u.source, &later_funcs, &defined);
                    defined.insert(u.target.as_str());
                }
                Statement::UnmoldBackward(u) => {
                    self.scan_expr_forward_refs(&u.source, &later_funcs, &defined);
                    defined.insert(u.target.as_str());
                }
                // An ErrorCeiling handler runs whenever a LATER protected
                // statement throws — at that moment only the functions
                // defined up to the throw site exist on the interpreter.
                // The throw site is not statically known, so the handler
                // is conservatively checked against the definitions
                // available at the ceiling itself; referencing a function
                // defined further down would make the program's success
                // depend on where the throw happens (and on the backend,
                // since the compiled ones hoist).
                Statement::ErrorCeiling(ec) => {
                    for st in &ec.handler_body {
                        match st {
                            Statement::Expr(e) => {
                                self.scan_expr_forward_refs(e, &later_funcs, &defined)
                            }
                            Statement::Assignment(a) => {
                                self.scan_expr_forward_refs(&a.value, &later_funcs, &defined)
                            }
                            Statement::UnmoldForward(u) => {
                                self.scan_expr_forward_refs(&u.source, &later_funcs, &defined)
                            }
                            Statement::UnmoldBackward(u) => {
                                self.scan_expr_forward_refs(&u.source, &later_funcs, &defined)
                            }
                            _ => {}
                        }
                    }
                }
                // Definitions and module plumbing are not executable
                // expressions.
                _ => {}
            }
        }
    }

    fn scan_expr_forward_refs(
        &mut self,
        e: &Expr,
        later_funcs: &std::collections::HashSet<&str>,
        defined: &std::collections::HashSet<&str>,
    ) {
        match e {
            Expr::Ident(name, span) => {
                if later_funcs.contains(name.as_str()) && !defined.contains(name.as_str()) {
                    self.errors.push(TypeError {
                        message: format!(
                            "[E1539] Function '{}' is used before its definition. Top-level code runs in order, so move this statement below the definition of '{}' (references from inside function bodies resolve at call time and may stay).",
                            name, name
                        ),
                        span: span.clone(),
                    });
                }
            }
            // Deferred-execution bodies: resolution happens at call time.
            Expr::Lambda(..) => {}
            // The interpreter parses and evaluates `${...}` bodies as
            // expressions, so a forward reference inside one fails at
            // runtime like any other — scan them.
            Expr::TemplateLit(template, _) => {
                for inner in template_interpolation_exprs(template) {
                    self.scan_expr_forward_refs(&inner, later_funcs, defined);
                }
            }
            Expr::IntLit(..)
            | Expr::FloatLit(..)
            | Expr::StringLit(..)
            | Expr::BoolLit(..)
            | Expr::Gorilla(..)
            | Expr::Placeholder(..)
            | Expr::Hole(..)
            | Expr::TypeLiteral(..)
            | Expr::EnumVariant(..) => {}
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for f in fields {
                    self.scan_expr_forward_refs(&f.value, later_funcs, defined);
                }
            }
            Expr::ListLit(items, _) | Expr::Pipeline(items, _) => {
                for i in items {
                    self.scan_expr_forward_refs(i, later_funcs, defined);
                }
            }
            Expr::BinaryOp(l, _, r, _) => {
                self.scan_expr_forward_refs(l, later_funcs, defined);
                self.scan_expr_forward_refs(r, later_funcs, defined);
            }
            Expr::UnaryOp(_, x, _) | Expr::Unmold(x, _) | Expr::Throw(x, _) => {
                self.scan_expr_forward_refs(x, later_funcs, defined);
            }
            Expr::FuncCall(callee, args, _) => {
                self.scan_expr_forward_refs(callee, later_funcs, defined);
                for a in args {
                    self.scan_expr_forward_refs(a, later_funcs, defined);
                }
            }
            Expr::MethodCall(recv, _, args, _) => {
                self.scan_expr_forward_refs(recv, later_funcs, defined);
                for a in args {
                    self.scan_expr_forward_refs(a, later_funcs, defined);
                }
            }
            Expr::FieldAccess(x, _, _) => self.scan_expr_forward_refs(x, later_funcs, defined),
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(c) = &arm.condition {
                        self.scan_expr_forward_refs(c, later_funcs, defined);
                    }
                    for st in &arm.body {
                        match st {
                            Statement::Expr(e) => {
                                self.scan_expr_forward_refs(e, later_funcs, defined)
                            }
                            Statement::Assignment(a) => {
                                self.scan_expr_forward_refs(&a.value, later_funcs, defined)
                            }
                            Statement::UnmoldForward(u) => {
                                self.scan_expr_forward_refs(&u.source, later_funcs, defined)
                            }
                            Statement::UnmoldBackward(u) => {
                                self.scan_expr_forward_refs(&u.source, later_funcs, defined)
                            }
                            _ => {}
                        }
                    }
                }
            }
            Expr::MoldInst(_, targs, fields, _) => {
                for t in targs {
                    self.scan_expr_forward_refs(t, later_funcs, defined);
                }
                for f in fields {
                    self.scan_expr_forward_refs(&f.value, later_funcs, defined);
                }
            }
        }
    }
}

impl TypeChecker {
    /// [E1540] The six value-absence words (`null` / `undefined` /
    /// `none` / `nil` / `unit` / `void`) are reserved: they must not be
    /// introduced as identifiers or type names. The lexer deliberately
    /// keeps them plain identifiers (no keyword tokens — see the BT-2
    /// lexer tests), so reads already fail as undefined variables; this
    /// pass closes the remaining gap where a *binding* would succeed
    /// (`null <= 42`) and resurrect the value-absence vocabulary.
    pub(super) fn check_reserved_absence_words(&mut self, program: &Program) {
        for stmt in &program.statements {
            self.scan_stmt_reserved_words(stmt);
        }
    }

    fn reserved_absence_word(name: &str) -> bool {
        matches!(
            name,
            "null" | "undefined" | "none" | "nil" | "unit" | "void"
        )
    }

    fn push_reserved_word_error(&mut self, name: &str, span: &Span) {
        self.errors.push(TypeError {
            message: format!(
                "[E1540] '{}' is reserved as a value-absence word and cannot be used as an identifier or type name. Every Taida type has a default value — name the binding after what it holds instead.",
                name
            ),
            span: span.clone(),
        });
    }

    fn scan_stmt_reserved_words(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assignment(a) => {
                if Self::reserved_absence_word(&a.target) {
                    self.push_reserved_word_error(&a.target, &a.span);
                }
                self.scan_expr_reserved_words(&a.value);
            }
            Statement::UnmoldForward(u) => {
                if Self::reserved_absence_word(&u.target) {
                    self.push_reserved_word_error(&u.target, &u.span);
                }
                self.scan_expr_reserved_words(&u.source);
            }
            Statement::UnmoldBackward(u) => {
                if Self::reserved_absence_word(&u.target) {
                    self.push_reserved_word_error(&u.target, &u.span);
                }
                self.scan_expr_reserved_words(&u.source);
            }
            Statement::FuncDef(fd) => {
                if Self::reserved_absence_word(&fd.name) {
                    self.push_reserved_word_error(&fd.name, &fd.span);
                }
                for p in &fd.params {
                    if Self::reserved_absence_word(&p.name) {
                        self.push_reserved_word_error(&p.name, &p.span);
                    }
                }
                for st in &fd.body {
                    self.scan_stmt_reserved_words(st);
                }
            }
            Statement::EnumDef(ed) => {
                if Self::reserved_absence_word(&ed.name) {
                    self.push_reserved_word_error(&ed.name, &ed.span);
                }
            }
            Statement::ClassLikeDef(cl) => {
                if Self::reserved_absence_word(&cl.name) {
                    self.push_reserved_word_error(&cl.name, &cl.span);
                }
            }
            Statement::Import(imp) => {
                for sym in &imp.symbols {
                    let bound = sym.alias.as_deref().unwrap_or(sym.name.as_str());
                    if Self::reserved_absence_word(bound) {
                        self.push_reserved_word_error(bound, &imp.span);
                    }
                }
            }
            Statement::Expr(e) => self.scan_expr_reserved_words(e),
            Statement::ErrorCeiling(ec) => {
                for st in &ec.handler_body {
                    self.scan_stmt_reserved_words(st);
                }
            }
            Statement::Export(_) => {}
        }
    }

    fn scan_expr_reserved_words(&mut self, e: &Expr) {
        match e {
            Expr::Lambda(params, body, _) => {
                for p in params {
                    if Self::reserved_absence_word(&p.name) {
                        self.push_reserved_word_error(&p.name, &p.span);
                    }
                }
                self.scan_expr_reserved_words(body);
            }
            Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
                for f in fields {
                    self.scan_expr_reserved_words(&f.value);
                }
            }
            Expr::ListLit(items, _) | Expr::Pipeline(items, _) => {
                for i in items {
                    self.scan_expr_reserved_words(i);
                }
            }
            Expr::BinaryOp(l, _, r, _) => {
                self.scan_expr_reserved_words(l);
                self.scan_expr_reserved_words(r);
            }
            Expr::UnaryOp(_, x, _) | Expr::Unmold(x, _) | Expr::Throw(x, _) => {
                self.scan_expr_reserved_words(x);
            }
            Expr::FuncCall(c, args, _) => {
                self.scan_expr_reserved_words(c);
                for a in args {
                    self.scan_expr_reserved_words(a);
                }
            }
            Expr::MethodCall(recv, _, args, _) => {
                self.scan_expr_reserved_words(recv);
                for a in args {
                    self.scan_expr_reserved_words(a);
                }
            }
            Expr::FieldAccess(x, _, _) => self.scan_expr_reserved_words(x),
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(c) = &arm.condition {
                        self.scan_expr_reserved_words(c);
                    }
                    for st in &arm.body {
                        self.scan_stmt_reserved_words(st);
                    }
                }
            }
            Expr::MoldInst(_, targs, fields, _) => {
                for t in targs {
                    self.scan_expr_reserved_words(t);
                }
                for f in fields {
                    self.scan_expr_reserved_words(&f.value);
                }
            }
            _ => {}
        }
    }
}

/// Extract the interpolation expressions of a template literal the way
/// the interpreter's `eval_template_string` does: each `${...}` body is
/// parsed as a standalone program whose first statement is the
/// expression (unparseable bodies render verbatim at runtime and are
/// skipped here too). Used by the order-sensitive checks ([E1539] /
/// [E1540]) so a forward reference inside `${...}` does not slip
/// through the scan.
pub(super) fn template_interpolation_exprs(template: &str) -> Vec<Expr> {
    let mut out = Vec::new();
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next();
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
            let (program, errors) = crate::parser::parse(&expr_str);
            if errors.is_empty()
                && let Some(Statement::Expr(expr)) = program.statements.into_iter().next()
            {
                out.push(expr);
            }
        }
    }
    out
}

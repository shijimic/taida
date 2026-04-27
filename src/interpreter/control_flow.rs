use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::Value;
/// Control flow evaluation for the Taida interpreter.
///
/// Contains `eval_pipeline_step`, `eval_binary_op`, and `eval_cond_branch`.
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
///
/// Pipeline scope management:
/// Each pipeline step uses explicit `push_scope()` / `pop_scope()` pairs rather
/// than a RAII guard. This is acceptable because `eval_expr` propagates errors
/// via `Result` (not panics), so `pop_scope()` is always reached. A RAII guard
/// would be preferable for panic-safety, but since interpreter panics terminate
/// the process, scope leaks have no observable effect.
use crate::parser::*;

/// Rewrite all `Placeholder` nodes in an expression tree to `Ident(replacement)`.
/// Used by pipeline MoldInst handling to substitute nested `_` with the pipe variable name.
fn rewrite_nested_placeholder(expr: &Expr, replacement: &str, span: &crate::lexer::Span) -> Expr {
    match expr {
        Expr::Placeholder(_) => Expr::Ident(replacement.to_string(), span.clone()),
        Expr::BinaryOp(lhs, op, rhs, s) => Expr::BinaryOp(
            Box::new(rewrite_nested_placeholder(lhs, replacement, span)),
            op.clone(),
            Box::new(rewrite_nested_placeholder(rhs, replacement, span)),
            s.clone(),
        ),
        Expr::UnaryOp(op, inner, s) => Expr::UnaryOp(
            op.clone(),
            Box::new(rewrite_nested_placeholder(inner, replacement, span)),
            s.clone(),
        ),
        Expr::FuncCall(callee, args, s) => Expr::FuncCall(
            Box::new(rewrite_nested_placeholder(callee, replacement, span)),
            args.iter()
                .map(|a| rewrite_nested_placeholder(a, replacement, span))
                .collect(),
            s.clone(),
        ),
        Expr::MethodCall(obj, method, args, s) => Expr::MethodCall(
            Box::new(rewrite_nested_placeholder(obj, replacement, span)),
            method.clone(),
            args.iter()
                .map(|a| rewrite_nested_placeholder(a, replacement, span))
                .collect(),
            s.clone(),
        ),
        Expr::MoldInst(name, type_args, fields, s) => Expr::MoldInst(
            name.clone(),
            type_args
                .iter()
                .map(|a| rewrite_nested_placeholder(a, replacement, span))
                .collect(),
            fields.clone(),
            s.clone(),
        ),
        other => other.clone(),
    }
}

/// Check if an expression contains a Placeholder `_` anywhere in its tree.
/// Used by pipeline MoldInst handling to detect nested placeholders like `_ > 3`.
fn expr_contains_placeholder(expr: &Expr) -> bool {
    match expr {
        Expr::Placeholder(_) => true,
        Expr::BinaryOp(lhs, _, rhs, _) => {
            expr_contains_placeholder(lhs) || expr_contains_placeholder(rhs)
        }
        Expr::UnaryOp(_, inner, _) => expr_contains_placeholder(inner),
        Expr::FuncCall(callee, args, _) => {
            expr_contains_placeholder(callee) || args.iter().any(expr_contains_placeholder)
        }
        Expr::MethodCall(obj, _, args, _) => {
            expr_contains_placeholder(obj) || args.iter().any(expr_contains_placeholder)
        }
        Expr::MoldInst(_, type_args, _, _) => type_args.iter().any(expr_contains_placeholder),
        _ => false,
    }
}

impl Interpreter {
    /// Evaluate a single pipeline step, applying the current value.
    ///
    /// Pipeline semantics:
    /// - FuncCall with placeholders: substitute placeholders with current value
    /// - FuncCall without placeholders: pass current as first argument
    /// - MethodCall: evaluate with current as _ in scope
    /// - Any other expression: evaluate with _ bound to current in scope
    pub(crate) fn eval_pipeline_step(
        &mut self,
        step: &Expr,
        current: Value,
    ) -> Result<Signal, RuntimeError> {
        match step {
            Expr::FuncCall(callee, args, span) => {
                // Check if any arg is a Placeholder
                let has_placeholder = args.iter().any(|a| matches!(a, Expr::Placeholder(_)));

                if has_placeholder {
                    // Replace placeholders with current value in the args,
                    // then evaluate the call directly with the substituted args.
                    // We bind current to a temporary variable to avoid value_to_expr issues.
                    self.env.push_scope();
                    let pipe_val_name = "__pipe_current__";
                    self.env.define_force(pipe_val_name, current);

                    let mut new_args: Vec<Expr> = Vec::new();
                    let dummy_span = crate::lexer::Span::new(0, 0, 0, 0);
                    for arg in args {
                        if matches!(arg, Expr::Placeholder(_)) {
                            new_args
                                .push(Expr::Ident(pipe_val_name.to_string(), dummy_span.clone()));
                        } else {
                            new_args.push(arg.clone());
                        }
                    }

                    let new_call = Expr::FuncCall(callee.clone(), new_args, span.clone());
                    let result = self.eval_expr(&new_call);
                    self.env.pop_scope();
                    result
                } else {
                    // No placeholder — pass current as first argument
                    self.env.push_scope();
                    let pipe_val_name = "__pipe_current__";
                    self.env.define_force(pipe_val_name, current);

                    let dummy_span = crate::lexer::Span::new(0, 0, 0, 0);
                    let mut new_args = vec![Expr::Ident(pipe_val_name.to_string(), dummy_span)];
                    new_args.extend(args.iter().cloned());
                    let new_call = Expr::FuncCall(callee.clone(), new_args, span.clone());
                    let result = self.eval_expr(&new_call);
                    self.env.pop_scope();
                    result
                }
            }
            Expr::MethodCall(obj, method, args, span) => {
                // For method calls in pipeline, check for placeholder in obj
                if matches!(obj.as_ref(), Expr::Placeholder(_)) {
                    // Replace obj placeholder with current
                    self.env.push_scope();
                    let pipe_val_name = "__pipe_current__";
                    self.env.define_force(pipe_val_name, current);
                    let dummy_span = crate::lexer::Span::new(0, 0, 0, 0);
                    let new_call = Expr::MethodCall(
                        Box::new(Expr::Ident(pipe_val_name.to_string(), dummy_span)),
                        method.clone(),
                        args.clone(),
                        span.clone(),
                    );
                    let result = self.eval_expr(&new_call);
                    self.env.pop_scope();
                    result
                } else {
                    // Fallback: evaluate with _ bound to current
                    self.env.push_scope();
                    self.env.define_force("_", current);
                    let result = self.eval_expr(step);
                    self.env.pop_scope();
                    result
                }
            }
            Expr::MoldInst(name, type_args, fields, span) => {
                // MoldInst in pipeline: replace _ placeholders in type_args with current value
                // B11-5a: Use deep rewriting to handle both top-level `_` and nested `_ > 3`.
                let has_any_placeholder = type_args.iter().any(expr_contains_placeholder);

                if has_any_placeholder {
                    // Rewrite ALL Placeholder nodes (top-level and nested) to
                    // Ident("__pipe_current__"). This handles cases like:
                    // - `If[_, "a", "b"]()` — top-level _
                    // - `If[_ > 3, "big", "small"]()` — nested _
                    // - `If[_ > 100, 100, _]()` — mixed
                    self.env.push_scope();
                    let pipe_val_name = "__pipe_current__";
                    self.env.define_force(pipe_val_name, current);

                    let dummy_span = crate::lexer::Span::new(0, 0, 0, 0);
                    let new_type_args: Vec<Expr> = type_args
                        .iter()
                        .map(|arg| rewrite_nested_placeholder(arg, pipe_val_name, &dummy_span))
                        .collect();

                    let new_expr =
                        Expr::MoldInst(name.clone(), new_type_args, fields.clone(), span.clone());
                    let result = self.eval_expr(&new_expr);
                    self.env.pop_scope();
                    result
                } else {
                    // No placeholder — insert current as first type arg
                    self.env.push_scope();
                    let pipe_val_name = "__pipe_current__";
                    self.env.define_force(pipe_val_name, current);

                    let dummy_span = crate::lexer::Span::new(0, 0, 0, 0);
                    let mut new_type_args =
                        vec![Expr::Ident(pipe_val_name.to_string(), dummy_span)];
                    new_type_args.extend(type_args.iter().cloned());

                    let new_expr =
                        Expr::MoldInst(name.clone(), new_type_args, fields.clone(), span.clone());
                    let result = self.eval_expr(&new_expr);
                    self.env.pop_scope();
                    result
                }
            }
            Expr::Ident(_, _) => {
                // Identifier in pipeline: evaluate it, and if it's a function, call it with current
                let step_val = match self.eval_expr(step)? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                match step_val {
                    Value::Function(ref func) => {
                        // Call the function with current as the only argument
                        let args = vec![current];
                        self.call_function_with_values(func, &args)
                            .map(Signal::Value)
                    }
                    _ => {
                        // Not a function — just return the evaluated value
                        Ok(Signal::Value(step_val))
                    }
                }
            }
            _ => {
                // General case: bind _ to current and evaluate
                self.env.push_scope();
                self.env.define_force("_", current);
                let result = self.eval_expr(step);
                self.env.pop_scope();
                result
            }
        }
    }

    /// Evaluate a binary operation.
    pub(crate) fn eval_binary_op(
        &self,
        left: &Value,
        op: &BinOp,
        right: &Value,
    ) -> Result<Signal, RuntimeError> {
        match op {
            BinOp::Add => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(Signal::Value(Value::Int(a.wrapping_add(*b)))),
                (Value::Float(a), Value::Float(b)) => Ok(Signal::Value(Value::Float(a + b))),
                (Value::Int(a), Value::Float(b)) => Ok(Signal::Value(Value::Float(*a as f64 + b))),
                (Value::Float(a), Value::Int(b)) => Ok(Signal::Value(Value::Float(a + *b as f64))),
                (Value::Str(a), Value::Str(b)) => {
                    // String concatenation with + between two strings.
                    // D29B-016 / Phase 10-B (Track-θ): dispatch through
                    // `concat_with` so that combined size >= 1024 bytes
                    // (or either operand already Rope) promotes to the
                    // gap-buffer rope path. Below the threshold, the Flat
                    // fast path is preserved (legacy `format!` semantics
                    // are equivalent because StrValue Display forwards
                    // to the inner str).
                    let combined = a.concat_with(b);
                    Ok(Signal::Value(Value::Str(std::sync::Arc::new(combined))))
                }
                _ => Err(RuntimeError {
                    message: format!("Cannot add {} and {}", left, right),
                }),
            },
            BinOp::Sub => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(Signal::Value(Value::Int(a.wrapping_sub(*b)))),
                (Value::Float(a), Value::Float(b)) => Ok(Signal::Value(Value::Float(a - b))),
                (Value::Int(a), Value::Float(b)) => Ok(Signal::Value(Value::Float(*a as f64 - b))),
                (Value::Float(a), Value::Int(b)) => Ok(Signal::Value(Value::Float(a - *b as f64))),
                _ => Err(RuntimeError {
                    message: format!("Cannot subtract {} and {}", left, right),
                }),
            },
            BinOp::Mul => match (left, right) {
                (Value::Int(a), Value::Int(b)) => Ok(Signal::Value(Value::Int(a.wrapping_mul(*b)))),
                (Value::Float(a), Value::Float(b)) => Ok(Signal::Value(Value::Float(a * b))),
                (Value::Int(a), Value::Float(b)) => Ok(Signal::Value(Value::Float(*a as f64 * b))),
                (Value::Float(a), Value::Int(b)) => Ok(Signal::Value(Value::Float(a * *b as f64))),
                _ => Err(RuntimeError {
                    message: format!("Cannot multiply {} and {}", left, right),
                }),
            },
            // BinOp::Div and BinOp::Mod removed — use Div[x, y]() and Mod[x, y]() molds
            BinOp::Eq => Ok(Signal::Value(Value::Bool(left == right))),
            BinOp::NotEq => Ok(Signal::Value(Value::Bool(left != right))),
            BinOp::Lt => Ok(Signal::Value(Value::Bool(left < right))),
            BinOp::Gt => Ok(Signal::Value(Value::Bool(left > right))),
            BinOp::GtEq => Ok(Signal::Value(Value::Bool(left >= right))),
            BinOp::And => Ok(Signal::Value(Value::Bool(
                left.is_truthy() && right.is_truthy(),
            ))),
            BinOp::Or => Ok(Signal::Value(Value::Bool(
                left.is_truthy() || right.is_truthy(),
            ))),
            BinOp::Concat => {
                let left_str = left.to_display_string();
                let right_str = right.to_display_string();
                Ok(Signal::Value(Value::str(format!(
                    "{}{}",
                    left_str, right_str
                ))))
            }
        }
    }

    /// Evaluate a condition branch.
    pub(crate) fn eval_cond_branch(&mut self, arms: &[CondArm]) -> Result<Signal, RuntimeError> {
        for arm in arms {
            match &arm.condition {
                Some(cond) => {
                    let cond_val = match self.eval_expr(cond)? {
                        Signal::Value(v) => v,
                        other => return Ok(other),
                    };
                    if cond_val.is_truthy() {
                        return self.eval_cond_arm_body(&arm.body);
                    }
                }
                None => {
                    // Default case (| _ |>)
                    return self.eval_cond_arm_body(&arm.body);
                }
            }
        }
        // No branch matched — return Unit
        Ok(Signal::Value(Value::Unit))
    }

    /// Evaluate a condition arm body (Vec<Statement>).
    /// Returns the value of the last expression statement.
    ///
    /// # C25B-032: no-TCO inside non-tail arm bodies
    ///
    /// `eval_cond_arm_body` is the **non-tail** variant, reached from
    /// `eval_cond_branch` which is itself dispatched by `eval_expr` (NOT
    /// `eval_expr_tail`). That means the surrounding function-body scope
    /// is not in tail position at this CondBranch, so the arm body's
    /// last statement must also not be treated as a tail-call site —
    /// otherwise a mutual tail call (e.g. `| _ |> throwBoom(...)`) would
    /// escape via `Signal::TailCall`, bypass the enclosing
    /// `|== error: Error = ...` handler in `call_function`'s trampoline,
    /// and surface as an unhandled error.
    ///
    /// The tail variant lives in `eval_cond_arm_body_tail` (in `eval.rs`),
    /// which *is* reached from `eval_expr_tail::CondBranch` and therefore
    /// legitimately retains TCO.
    fn eval_cond_arm_body(&mut self, body: &[Statement]) -> Result<Signal, RuntimeError> {
        self.eval_statements_no_tco(body)
    }
}

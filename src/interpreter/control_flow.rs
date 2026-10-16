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
pub(super) fn rewrite_nested_placeholder(
    expr: &Expr,
    replacement: &str,
    span: &crate::lexer::Span,
) -> Expr {
    match expr {
        Expr::Placeholder(_) => Expr::Ident(replacement.to_string(), span.clone()),
        Expr::BuchiPack(fields, s) => Expr::BuchiPack(
            fields
                .iter()
                .map(|field| BuchiField {
                    name: field.name.clone(),
                    value: rewrite_nested_placeholder(&field.value, replacement, span),
                    span: field.span.clone(),
                })
                .collect(),
            s.clone(),
        ),
        Expr::ListLit(items, s) => Expr::ListLit(
            items
                .iter()
                .map(|item| rewrite_nested_placeholder(item, replacement, span))
                .collect(),
            s.clone(),
        ),
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
        Expr::FieldAccess(obj, field, s) => Expr::FieldAccess(
            Box::new(rewrite_nested_placeholder(obj, replacement, span)),
            field.clone(),
            s.clone(),
        ),
        Expr::CondBranch(arms, s) => Expr::CondBranch(
            arms.iter()
                .map(|arm| CondArm {
                    condition: arm
                        .condition
                        .as_ref()
                        .map(|condition| rewrite_nested_placeholder(condition, replacement, span)),
                    body: arm
                        .body
                        .iter()
                        .map(|stmt| rewrite_statement_placeholder(stmt, replacement, span))
                        .collect(),
                    span: arm.span.clone(),
                })
                .collect(),
            s.clone(),
        ),
        Expr::Pipeline(steps, s) => Expr::Pipeline(
            steps
                .iter()
                .map(|step| rewrite_nested_placeholder(step, replacement, span))
                .collect(),
            s.clone(),
        ),
        Expr::MoldInst(name, type_args, fields, s) => Expr::MoldInst(
            name.clone(),
            type_args
                .iter()
                .map(|a| rewrite_nested_placeholder(a, replacement, span))
                .collect(),
            fields
                .iter()
                .map(|field| BuchiField {
                    name: field.name.clone(),
                    value: rewrite_nested_placeholder(&field.value, replacement, span),
                    span: field.span.clone(),
                })
                .collect(),
            s.clone(),
        ),
        Expr::Unmold(inner, s) => Expr::Unmold(
            Box::new(rewrite_nested_placeholder(inner, replacement, span)),
            s.clone(),
        ),
        Expr::Lambda(params, body, s) => Expr::Lambda(
            params
                .iter()
                .map(|param| Param {
                    name: param.name.clone(),
                    type_annotation: param.type_annotation.clone(),
                    default_value: param
                        .default_value
                        .as_ref()
                        .map(|value| rewrite_nested_placeholder(value, replacement, span)),
                    span: param.span.clone(),
                })
                .collect(),
            Box::new(rewrite_nested_placeholder(body, replacement, span)),
            s.clone(),
        ),
        Expr::TypeInst(name, fields, s) => Expr::TypeInst(
            name.clone(),
            fields
                .iter()
                .map(|field| BuchiField {
                    name: field.name.clone(),
                    value: rewrite_nested_placeholder(&field.value, replacement, span),
                    span: field.span.clone(),
                })
                .collect(),
            s.clone(),
        ),
        Expr::Throw(inner, s) => Expr::Throw(
            Box::new(rewrite_nested_placeholder(inner, replacement, span)),
            s.clone(),
        ),
        other => other.clone(),
    }
}

fn rewrite_statement_placeholder(
    stmt: &Statement,
    replacement: &str,
    span: &crate::lexer::Span,
) -> Statement {
    match stmt {
        Statement::Expr(expr) => {
            Statement::Expr(rewrite_nested_placeholder(expr, replacement, span))
        }
        Statement::Assignment(assign) => Statement::Assignment(Assignment {
            target: assign.target.clone(),
            type_annotation: assign.type_annotation.clone(),
            value: rewrite_nested_placeholder(&assign.value, replacement, span),
            doc_comments: assign.doc_comments.clone(),
            span: assign.span.clone(),
            forward: assign.forward,
        }),
        Statement::UnmoldForward(unmold) => Statement::UnmoldForward(UnmoldForwardStmt {
            source: rewrite_nested_placeholder(&unmold.source, replacement, span),
            target: unmold.target.clone(),
            type_annotation: unmold.type_annotation.clone(),
            span: unmold.span.clone(),
        }),
        Statement::UnmoldBackward(unmold) => Statement::UnmoldBackward(UnmoldBackwardStmt {
            target: unmold.target.clone(),
            type_annotation: unmold.type_annotation.clone(),
            source: rewrite_nested_placeholder(&unmold.source, replacement, span),
            span: unmold.span.clone(),
        }),
        _ => stmt.clone(),
    }
}

// `expr_contains_placeholder` / `statement_contains_placeholder` moved to
// `crate::parser::ast` (re-exported through `crate::parser::*`) so the
// type checker shares the exact placeholder rule used here for pipeline
// stage evaluation.

impl Interpreter {
    /// Evaluate a single pipeline step, applying the current value.
    ///
    /// F62B-025: pipeline application closes over exactly two rules.
    ///
    /// Rule 1 — the stage contains `_` (one at most, E1543): the piped
    /// value is injected syntactically at the placeholder and the
    /// rewritten stage is evaluated as written. The `_` may sit anywhere
    /// in the stage expression (call argument, method argument, mold
    /// argument, nested comparison like `_ > 3`, ...).
    ///
    /// Rule 2 — the stage contains no `_`: the stage expression is
    /// evaluated exactly as written; when the result is a function value
    /// the piped value is applied to it, anything else is a pipe into a
    /// non-function value (E1544). This is what makes
    /// `5 => add(, 3)` ≡ `f <= add(, 3)` + `5 => f` (compositionality).
    ///
    /// The legacy implicit first-argument injection (`5 => f(3)` running
    /// as `f(5, 3)`) and the zero-arg special form (`5 => f()` running
    /// as `f(5)`) are gone: both now evaluate the stage as written and
    /// fall under rule 2. Empty slots stay an independent partial-
    /// application mechanism — `5 => add(, 3)` evaluates the closure and
    /// applies 5 (rule 2), `5 => add(, _, 3)` injects 5 and keeps the
    /// hole (rule 1).
    pub(crate) fn eval_pipeline_step(
        &mut self,
        step: &Expr,
        current: Value,
    ) -> Result<Signal, RuntimeError> {
        let placeholder_count = expr_count_placeholders(step);
        if placeholder_count > 1 {
            return Err(RuntimeError {
                message: format!(
                    "[E1543] A pipeline stage can contain at most one `_` (found {}). \
                     The pipe carries exactly one value. Hint: bind the value first \
                     (`x => v => f(v, v)`) to use it more than once, and use empty \
                     slots (`f(, _)`) for partial-application holes.",
                    placeholder_count
                ),
            });
        }

        if placeholder_count == 1 {
            // Rule 1: syntactic injection at the placeholder.
            self.env.push_scope();
            let pipe_val_name = "__pipe_current__";
            self.env.define_force(pipe_val_name, current);
            let dummy_span = crate::lexer::Span::new(0, 0, 0, 0);
            let rewritten = rewrite_nested_placeholder(step, pipe_val_name, &dummy_span);
            let result = self.eval_expr(&rewritten);
            self.env.pop_scope();
            return result;
        }

        // Rule 2: evaluate the stage as written, then apply the piped value
        // when the stage evaluated to a function.
        if let Expr::Ident(name, span) = step {
            // `5 => f`: prelude / user functions and closures resolve to
            // Value::Function; builtin dispatch sentinels (net / crypto /
            // addon) resolve to a `__`-prefixed Str and are called through
            // the normal builtin call machinery.
            let step_val = match self.eval_expr(step)? {
                Signal::Value(v) => v,
                other => return Ok(other),
            };
            return match step_val {
                Value::Function(ref func) => {
                    let args = vec![current];
                    self.call_function_with_values(func, &args)
                        .map(Signal::Value)
                }
                Value::Str(ref s) if s.starts_with("__") => {
                    self.env.push_scope();
                    let pipe_val_name = "__pipe_current__";
                    self.env.define_force(pipe_val_name, current);
                    let dummy_span = crate::lexer::Span::new(0, 0, 0, 0);
                    let call = Expr::FuncCall(
                        Box::new(Expr::Ident(name.clone(), span.clone())),
                        vec![Expr::Ident(pipe_val_name.to_string(), dummy_span)],
                        span.clone(),
                    );
                    let result = self.eval_expr(&call);
                    self.env.pop_scope();
                    result
                }
                _ => Err(RuntimeError {
                    message: format!(
                        "[E1544] Pipeline stage '{}' resolved to a non-function value: {}. \
                         A `_`-free stage is evaluated as written and must produce a \
                         function for the piped value. Hint: use `_` to mark the \
                         injection position (`x => f(_, a)`).",
                        name,
                        step_val.to_error_display(200)
                    ),
                }),
            };
        }

        let step_val = match self.eval_expr(step)? {
            Signal::Value(v) => v,
            other => return Ok(other),
        };
        match step_val {
            Value::Function(ref func) => {
                let args = vec![current];
                self.call_function_with_values(func, &args)
                    .map(Signal::Value)
            }
            _ => Err(RuntimeError {
                message: format!(
                    "[E1544] Pipeline stage evaluated to a non-function value: {}. \
                     A `_`-free stage is evaluated as written and must produce a \
                     function for the piped value. Hint: use `_` to mark the \
                     injection position (`x => f(_, a)`), or an empty slot for \
                     partial application (`x => f(, a)`).",
                    step_val.to_error_display(200)
                ),
            }),
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
    /// #: no-TCO inside non-tail arm bodies
    ///
    /// `eval_cond_arm_body` is the **non-tail** variant, reached from
    /// `eval_cond_branch` which is itself dispatched by `eval_expr` (NOT
    /// `eval_expr_tail`). That means the surrounding function-body scope
    /// is not in tail position at this CondBranch, so the arm body's
    /// last statement must also not be treated as a tail-call site —
    /// otherwise a mutual tail call (e.g. `| _ |> throwBoom(...)`) would
    /// escape via `Signal::TailCall`, bypass the enclosing
    /// `|== error: Error =...` handler in `call_function`'s trampoline,
    /// and surface as an unhandled error.
    ///
    /// The tail variant lives in `eval_cond_arm_body_tail` (in `eval.rs`),
    /// which *is* reached from `eval_expr_tail::CondBranch` and therefore
    /// legitimately retains TCO.
    ///
    /// Each arm body is a scope of its own: names it defines are gone once
    /// the arm finishes (`docs/reference/scope_rules.md`).
    fn eval_cond_arm_body(&mut self, body: &[Statement]) -> Result<Signal, RuntimeError> {
        let mark = self.env.mark();
        self.env.push_scope();
        let result = self.eval_statements_no_tco(body);
        self.env.truncate(mark);
        result
    }
}

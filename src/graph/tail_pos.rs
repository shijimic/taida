//! Tail-position analyzer for the `mutual-recursion` verify check (C12-3 / FB-8).
//!
//! This module walks the AST of a single [`FuncDef`] and emits a [`CallSite`]
//! entry for every direct function call (`Expr::FuncCall` where the callee is
//! a plain [`Expr::Ident`]). Each entry carries an [`is_tail`] flag
//! indicating whether the call is in the tail position of the enclosing
//! function's body.
//!
//! Conservative rules (intentionally narrower than the runtime TCO so that
//! the C12-3 compile-time reject does not over-reject programs that happen
//! to work at runtime):
//!
//! - The final statement of a function body is in tail position. Earlier
//!   statements (including assignments and unmold binds) are not.
//! - Inside a [`Expr::CondBranch`], the final statement of each arm
//!   inherits the enclosing tail flag.
//! - Inside an [`Expr::Pipeline`], **only** the last stage inherits tail,
//!   and even then only if the last stage is itself a direct `FuncCall`
//!   (e.g., `x => foo(_)`). A bare `Ident` pipeline stage is conservatively
//!   treated as non-tail (it is morally a reference, not a tail call site).
//! - Function arguments, `MoldInst` header / body args, `BuchiPack` field
//!   values, list elements, binary-op operands, lambda bodies, and
//!   `ErrorCeiling` handler bodies are all **non-tail** — they feed a
//!   surrounding construct and therefore cannot be the last operation.
//! - Lambdas introduce their own tail frame; calls inside a lambda body
//!   are scoped to the lambda, not the outer function. For the purpose of
//!   the mutual-recursion check they are treated as non-tail of the outer
//!   function.
//!
//! The analyzer intentionally does **not** try to reason about
//! `|== Error =` ceilings' return values. A call inside an error handler
//! body is treated as non-tail w.r.t. the outer function (the handler
//! itself is a separate trampoline scope).

use crate::lexer::Span;
use crate::parser::*;

/// A single direct function call inside a function body, with tail-position
/// information relative to the enclosing function.
#[derive(Debug, Clone, PartialEq)]
pub struct CallSite {
    /// Name of the callee (only direct `FuncCall` with `Ident` callees are
    /// tracked; method calls and lambda calls are never recorded).
    pub callee: String,
    /// Whether this call is in the tail position of the enclosing function.
    pub is_tail: bool,
    /// Source span of the call expression.
    pub span: Span,
}

/// Collect every direct function call inside `fd` along with its tail flag.
///
/// The returned vector is in source order.
pub fn collect_call_sites(fd: &FuncDef) -> Vec<CallSite> {
    let mut out = Vec::new();
    let body_len = fd.body.len();
    for (i, stmt) in fd.body.iter().enumerate() {
        let is_tail = i == body_len - 1;
        visit_stmt(stmt, is_tail, &mut out);
    }
    out
}

fn visit_stmt(stmt: &Statement, is_tail: bool, out: &mut Vec<CallSite>) {
    match stmt {
        Statement::Expr(expr) => visit_expr(expr, is_tail, out),
        Statement::Assignment(a) => visit_expr(&a.value, false, out),
        Statement::UnmoldForward(uf) => visit_expr(&uf.source, false, out),
        Statement::UnmoldBackward(ub) => visit_expr(&ub.source, false, out),
        Statement::ErrorCeiling(ec) => {
            // Handler body runs in its own trampoline scope. Calls inside
            // it are not tail-of-outer.
            let h_len = ec.handler_body.len();
            for (i, hstmt) in ec.handler_body.iter().enumerate() {
                let _last = i == h_len - 1;
                // Even the last handler stmt is not tail of the *outer*
                // function in the sense that matters for this check
                // (mutual recursion across the ceiling is unusual and a
                // conservative reject is safer than a conservative pass).
                visit_stmt(hstmt, false, out);
            }
        }
        _ => {}
    }
}

fn visit_expr(expr: &Expr, is_tail: bool, out: &mut Vec<CallSite>) {
    match expr {
        Expr::FuncCall(callee, args, span) => {
            if let Expr::Ident(name, _) = callee.as_ref() {
                out.push(CallSite {
                    callee: name.clone(),
                    is_tail,
                    span: span.clone(),
                });
            } else {
                // Indirect callees (e.g., dynamic dispatch) are not tracked;
                // still recurse into the callee sub-expression for any
                // nested direct calls.
                visit_expr(callee, false, out);
            }
            for arg in args {
                visit_expr(arg, false, out);
            }
        }
        Expr::MethodCall(obj, _, args, _) => {
            visit_expr(obj, false, out);
            for arg in args {
                visit_expr(arg, false, out);
            }
        }
        Expr::FieldAccess(obj, _, _) => {
            visit_expr(obj, false, out);
        }
        Expr::BinaryOp(l, _, r, _) => {
            visit_expr(l, false, out);
            visit_expr(r, false, out);
        }
        Expr::UnaryOp(_, e, _) => visit_expr(e, false, out),
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(cond) = &arm.condition {
                    visit_expr(cond, false, out);
                }
                let a_len = arm.body.len();
                for (i, s) in arm.body.iter().enumerate() {
                    let t = is_tail && i == a_len - 1;
                    visit_stmt(s, t, out);
                }
            }
        }
        Expr::Pipeline(stages, _) => {
            let n = stages.len();
            for (i, stage) in stages.iter().enumerate() {
                let last = i == n - 1;
                // Only the last stage can be tail, and only if it is a
                // direct FuncCall shape. A bare Ident stage is treated as
                // non-tail because it reads more like a function reference
                // than a direct tail call site for this check.
                let stage_tail = is_tail
                    && last
                    && matches!(stage, Expr::FuncCall(_, _, _));
                visit_expr(stage, stage_tail, out);
            }
        }
        Expr::ListLit(items, _) => {
            for item in items {
                visit_expr(item, false, out);
            }
        }
        Expr::BuchiPack(fields, _) => {
            for f in fields {
                visit_expr(&f.value, false, out);
            }
        }
        Expr::TypeInst(_, fields, _) => {
            for f in fields {
                visit_expr(&f.value, false, out);
            }
        }
        Expr::MoldInst(_, type_args, fields, _) => {
            for a in type_args {
                visit_expr(a, false, out);
            }
            for f in fields {
                visit_expr(&f.value, false, out);
            }
        }
        Expr::Unmold(e, _) => visit_expr(e, false, out),
        Expr::Throw(e, _) => visit_expr(e, false, out),
        Expr::Lambda(_, body, _) => {
            // Lambda body is its own tail frame — calls inside are not
            // tail-of-outer. Just recurse to pick up direct calls for
            // cycle enumeration (still marked non-tail of outer).
            visit_expr(body, false, out);
        }
        // Leaves — no sub-expressions.
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::TemplateLit(..)
        | Expr::BoolLit(..)
        | Expr::Gorilla(..)
        | Expr::Ident(..)
        | Expr::Placeholder(..)
        | Expr::Hole(..)
        | Expr::EnumVariant(..)
        | Expr::TypeLiteral(..) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn parse_one_func(src: &str) -> FuncDef {
        let (program, errors) = parse(src);
        assert!(errors.is_empty(), "parse errors: {:?}", errors);
        for stmt in program.statements {
            if let Statement::FuncDef(fd) = stmt {
                return fd;
            }
        }
        panic!("no FuncDef in source");
    }

    #[test]
    fn test_single_tail_call() {
        // `f(n - 1)` is the only body stmt → tail.
        let fd = parse_one_func("foo n =\n  f(n - 1)\n");
        let sites = collect_call_sites(&fd);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].callee, "f");
        assert!(sites[0].is_tail);
    }

    #[test]
    fn test_assignment_then_call() {
        // Assignment is never tail; final expr is tail.
        let fd = parse_one_func("foo n =\n  x <= g(n)\n  h(x)\n");
        let sites = collect_call_sites(&fd);
        assert_eq!(sites.len(), 2);
        let g = sites.iter().find(|s| s.callee == "g").unwrap();
        let h = sites.iter().find(|s| s.callee == "h").unwrap();
        assert!(!g.is_tail);
        assert!(h.is_tail);
    }

    #[test]
    fn test_arg_position_not_tail() {
        // `g` is an argument to `f`, only `f` is tail.
        let fd = parse_one_func("foo n =\n  f(g(n))\n");
        let sites = collect_call_sites(&fd);
        let f = sites.iter().find(|s| s.callee == "f").unwrap();
        let g = sites.iter().find(|s| s.callee == "g").unwrap();
        assert!(f.is_tail);
        assert!(!g.is_tail);
    }

    #[test]
    fn test_cond_branch_arm_tail() {
        // Each arm's last expr inherits tail from the enclosing branch.
        let src = "foo n =\n  | n == 0 |> 1\n  | _ |> g(n - 1)\n";
        let fd = parse_one_func(src);
        let sites = collect_call_sites(&fd);
        let g = sites.iter().find(|s| s.callee == "g").unwrap();
        assert!(g.is_tail, "g in arm tail should be tail");
    }

    #[test]
    fn test_binary_op_not_tail() {
        // `g(n) + 1` → g is inside a BinOp, not tail.
        let fd = parse_one_func("foo n =\n  g(n) + 1\n");
        let sites = collect_call_sites(&fd);
        let g = sites.iter().find(|s| s.callee == "g").unwrap();
        assert!(!g.is_tail);
    }

    #[test]
    fn test_list_element_not_tail() {
        let fd = parse_one_func("foo n =\n  @[g(n), h(n)]\n");
        let sites = collect_call_sites(&fd);
        for s in &sites {
            assert!(!s.is_tail, "list element {} should not be tail", s.callee);
        }
    }

    #[test]
    fn test_lambda_inside_body_not_tail() {
        // Lambda body call is scoped to lambda, not outer.
        let fd = parse_one_func("foo xs =\n  xs.map(_ x = g(x))\n");
        let sites = collect_call_sites(&fd);
        let g = sites.iter().find(|s| s.callee == "g");
        if let Some(g) = g {
            assert!(!g.is_tail);
        }
    }
}

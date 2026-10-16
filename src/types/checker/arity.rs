//! arity — methods split out of the TypeChecker impl.
//! Pure move from the parent module; behaviour unchanged.

use crate::parser::*;

use super::{TypeChecker, TypeError};

impl TypeChecker {
    pub(super) fn core_builtin_arity(name: &str) -> Option<(usize, usize)> {
        match name {
            "debug" => Some((1, 2)),
            // The function forms `toString(x)` / `toStr(x)` / `typeOf(x)`
            // exist nowhere at run time (`.toString()` is a method), so they
            // are undefined names like any other.
            "typeof" => Some((1, 1)),
            "jsonEncode" | "jsonPretty" => Some((1, 1)),
            "nowMs" => Some((0, 0)),
            "assert" => Some((1, 2)),
            "throw" => Some((1, 1)),
            "range" => Some((2, 3)),
            "enumerate" => Some((1, 1)),
            "zip" => Some((2, 2)),
            "hashMap" => Some((0, 1)),
            "setOf" => Some((1, 1)),
            "strOf" => Some((2, 2)),
            "stdout" | "stderr" | "exit" => Some((1, 1)),
            "stdin" | "stdinLine" => Some((0, 1)),
            "argv" => Some((0, 0)),
            "sleep" => Some((1, 1)),
            "Regex" => Some((1, 2)),
            "readBytes" => Some((1, 1)),
            "readBytesAt" => Some((3, 3)),
            "writeFile" | "writeBytes" | "appendFile" => Some((2, 2)),
            "remove" | "createDir" => Some((1, 1)),
            "rename" => Some((2, 2)),
            "allEnv" => Some((0, 0)),
            "dnsResolve" => Some((1, 2)),
            "tcpConnect" => Some((2, 3)),
            "tcpListen" | "tcpAccept" => Some((1, 2)),
            "socketSend" | "socketSendAll" | "socketSendBytes" => Some((2, 3)),
            "socketRecv" | "socketRecvBytes" => Some((1, 2)),
            "socketRecvExact" => Some((2, 3)),
            "udpBind" => Some((2, 3)),
            "udpSendTo" => Some((4, 5)),
            "udpRecvFrom" => Some((1, 2)),
            "socketClose" | "listenerClose" | "udpClose" => Some((1, 1)),
            "poolCreate" => Some((1, 1)),
            "poolAcquire" => Some((1, 2)),
            "poolRelease" => Some((3, 3)),
            "poolClose" | "poolHealth" => Some((1, 1)),
            _ => None,
        }
    }

    pub(super) fn inheritance_child_arity(&self, inh: &ClassLikeDef, parent_arity: usize) -> usize {
        // (E30 Sub-step 2.1) Inheritance kind の ClassLikeDef のみ呼び出される想定。
        inh.name_args
            .as_ref()
            .map(Vec::len)
            .or_else(|| inh.parent_args().map(Vec::len))
            .unwrap_or(parent_arity)
    }

    pub(super) fn http_serve_handler_arity(&self, expr: Option<&Expr>) -> Option<usize> {
        match expr? {
            Expr::Lambda(params, _, _) => Some(params.len()),
            Expr::Ident(name, _) => self.func_param_counts.get(name).copied(),
            _ => None,
        }
    }

    // C12-2c: Walk an expression subtree and emit E1508 for any
    // `.toString(args)` call with a non-empty argument list. Scoped
    // narrowly so that builtin arg contexts (e.g. `stdout(...)`) still
    // reject `.toString(16)` without otherwise changing type inference
    // for those args (avoids triggering E1510 on callable-variable
    // sites and E1602 on Error-type `__type` field access).
    pub(super) fn check_tostring_arity_in_expr(&mut self, expr: &Expr) {
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
}

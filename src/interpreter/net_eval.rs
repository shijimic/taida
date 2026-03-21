/// Net package evaluation for the Taida interpreter.
///
/// Implements `taida-lang/net` (core-bundled):
///
/// Legacy surface (shared with os runtime dispatch):
///   dnsResolve, tcpConnect, tcpListen, tcpAccept,
///   socketSend, socketSendAll, socketRecv,
///   socketSendBytes, socketRecvBytes, socketRecvExact,
///   udpBind, udpSendTo, udpRecvFrom,
///   socketClose, listenerClose, udpClose
///
/// HTTP v1 surface (new):
///   httpServe, httpParseRequestHead, httpEncodeResponse
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.

use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::Value;
use crate::parser::Expr;

/// All symbols exported by the net package.
/// Legacy (16) + HTTP v1 (3) = 19 symbols.
pub(crate) const NET_SYMBOLS: &[&str] = &[
    // Legacy surface (shared with os)
    "dnsResolve",
    "tcpConnect",
    "tcpListen",
    "tcpAccept",
    "socketSend",
    "socketSendAll",
    "socketRecv",
    "socketSendBytes",
    "socketRecvBytes",
    "socketRecvExact",
    "udpBind",
    "udpSendTo",
    "udpRecvFrom",
    "socketClose",
    "listenerClose",
    "udpClose",
    // HTTP v1
    "httpServe",
    "httpParseRequestHead",
    "httpEncodeResponse",
];

impl Interpreter {
    /// Try to handle a net built-in function call.
    /// Returns None if the name is not a recognized net function
    /// or if the function was not imported from taida-lang/net (sentinel guard).
    pub(crate) fn try_net_func(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        // Sentinel guard: only dispatch if imported from taida-lang/net
        let sentinel = format!("__net_builtin_{}", name);
        if !matches!(self.env.get(name), Some(Value::Str(tag)) if tag == &sentinel) {
            return Ok(None);
        }

        match name {
            // ── Legacy surface — delegate to os_eval implementations ──
            "dnsResolve" | "tcpConnect" | "tcpListen" | "tcpAccept"
            | "socketSend" | "socketSendAll" | "socketRecv"
            | "socketSendBytes" | "socketRecvBytes" | "socketRecvExact"
            | "udpBind" | "udpSendTo" | "udpRecvFrom"
            | "socketClose" | "listenerClose" | "udpClose" => {
                self.try_os_func(name, args)
            }

            // ── HTTP v1 — stub (implementation in NET-1/NET-2) ──
            "httpServe" | "httpParseRequestHead" | "httpEncodeResponse" => {
                Err(RuntimeError {
                    message: format!(
                        "{} is not yet implemented (taida-lang/net HTTP v1 pending)",
                        name
                    ),
                })
            }

            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_net_symbols_count() {
        // 16 legacy + 3 HTTP v1 = 19
        assert_eq!(NET_SYMBOLS.len(), 19);
        // Legacy
        assert!(NET_SYMBOLS.contains(&"dnsResolve"));
        assert!(NET_SYMBOLS.contains(&"tcpConnect"));
        assert!(NET_SYMBOLS.contains(&"socketClose"));
        assert!(NET_SYMBOLS.contains(&"udpClose"));
        // HTTP v1
        assert!(NET_SYMBOLS.contains(&"httpServe"));
        assert!(NET_SYMBOLS.contains(&"httpParseRequestHead"));
        assert!(NET_SYMBOLS.contains(&"httpEncodeResponse"));
    }

    #[test]
    fn test_sentinel_guard_blocks_without_import() {
        // httpServe without sentinel → try_net_func returns None (no interception)
        let mut interp = Interpreter::new();
        let args: Vec<Expr> = vec![];
        let result = interp.try_net_func("httpServe", &args).unwrap();
        assert!(result.is_none(), "should return None without sentinel");
    }

    #[test]
    fn test_sentinel_guard_passes_with_correct_sentinel() {
        // httpServe with correct sentinel → dispatches (returns error since stub)
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__net_builtin_httpServe".into()));
        let args: Vec<Expr> = vec![];
        let result = interp.try_net_func("httpServe", &args);
        assert!(result.is_err(), "stub should return RuntimeError");
        let err = result.unwrap_err();
        assert!(
            err.message.contains("not yet implemented"),
            "error: {}",
            err.message
        );
    }

    #[test]
    fn test_sentinel_guard_blocks_wrong_sentinel() {
        // httpServe with os sentinel → try_net_func returns None
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Str("__os_builtin_httpServe".into()));
        let args: Vec<Expr> = vec![];
        let result = interp.try_net_func("httpServe", &args).unwrap();
        assert!(result.is_none(), "wrong sentinel should return None");
    }

    #[test]
    fn test_sentinel_guard_blocks_user_function() {
        // httpServe defined as user function → try_net_func returns None
        let mut interp = Interpreter::new();
        interp
            .env
            .define_force("httpServe", Value::Int(42));
        let args: Vec<Expr> = vec![];
        let result = interp.try_net_func("httpServe", &args).unwrap();
        assert!(result.is_none(), "user value should return None");
    }

    #[test]
    fn test_http_stubs_return_not_implemented() {
        for name in ["httpServe", "httpParseRequestHead", "httpEncodeResponse"] {
            let mut interp = Interpreter::new();
            interp.env.define_force(
                name,
                Value::Str(format!("__net_builtin_{}", name)),
            );
            let args: Vec<Expr> = vec![];
            let result = interp.try_net_func(name, &args);
            assert!(result.is_err(), "{} should error as stub", name);
        }
    }
}

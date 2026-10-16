use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::{
    AsyncStatus, AsyncValue, FuncValue, PRELUDE_FUNCTION_PREFIX, PendingState, Value,
};
use crate::crypto;
/// Prelude (built-in) functions for the Taida interpreter.
///
/// Contains `try_builtin_func` and `type_name_of` — the built-in function
/// dispatch that handles: debug, stdout, stderr, stdin, nowMs, sleep,
/// jsonEncode, jsonPretty, Lax, hashMap, setOf, typeof, range, enumerate, zip, assert.
///
/// NOTE: Some/None/Ok/Err are ABOLISHED. Optional is fully abolished (use Lax[v]()). Use Result[v, pred]() mold syntax.
/// NOTE: jsonParse, jsonFrom, jsonDecode are ABOLISHED (Molten Iron design).
/// JSON data must be cast through a schema: JSON[raw, Schema]().
/// Only output-direction functions remain: jsonEncode, jsonPretty.
///
/// NOTE: print/println are intentionally NOT provided. Use stdout() only.
///
/// These are `impl Interpreter` methods split from eval.rs for maintainability.
use crate::parser::Expr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

impl Interpreter {
    /// Get the type name of a value as a string.
    pub(crate) fn type_name_of(val: &Value) -> &'static str {
        match val {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Str(_) => "Str",
            Value::Bytes(_) => "Bytes",
            Value::Bool(_) => "Bool",
            Value::BuchiPack(fields) => {
                // Check for __type field for typed BuchiPacks
                for (name, v) in fields.iter() {
                    if name == "__type"
                        && let Value::Str(s) = v
                    {
                        return match s.as_str() {
                            "Result" => "Result",
                            "Lax" => "Lax",
                            "HashMap" => "HashMap",
                            "Set" => "Set",
                            _ => "BuchiPack",
                        };
                    }
                }
                "BuchiPack"
            }
            Value::List(_) => "List",
            Value::Function(_) => "Function",
            Value::Gorilla => "Gorilla",
            Value::Unit => "Unit",
            Value::Error(_) => "Error",
            Value::Async(_) => "Async",
            Value::AsyncTask(_) => "AsyncTask",
            Value::Json(_) => "JSON",
            Value::Molten => "Molten",
            Value::Moltenized { policy, .. } => {
                if policy == "Secret" {
                    "Secret"
                } else {
                    "Moltenized"
                }
            }
            Value::Stream(_) => "Stream",
            // C18-2: EnumVal reports as "Int" so that prelude `typeOf` /
            // `typeof` builtins return the same type name they did before
            // Phase 2. Existing user code expects Enum values to `typeOf`
            // as Int because the internal representation has always been
            // Int(ordinal); we preserve that observable behaviour.
            Value::EnumVal(_, _) => "Int",
        }
    }

    /// Get the value-time public type identity used by `TypeName[value]()`.
    pub(crate) fn value_type_name(&self, val: &Value) -> String {
        match val {
            Value::BuchiPack(fields) => fields
                .iter()
                .find_map(|(name, v)| {
                    if name == "__type"
                        && let Value::Str(s) = v
                    {
                        Some(s.as_string().clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
            Value::Error(err) => {
                if err.error_type.is_empty() {
                    "Error".to_string()
                } else {
                    err.error_type.clone()
                }
            }
            Value::EnumVal(enum_key, ordinal) => self
                .enum_variants_of(enum_key)
                .and_then(|variants| variants.get(*ordinal as usize))
                .cloned()
                .unwrap_or_else(|| enum_key.clone()),
            other => Self::type_name_of(other).to_string(),
        }
    }

    /// Dispatch a `taida-lang/crypto` symbol (resolved from the
    /// `__crypto_builtin_<sym>` sentinel). Returns the computed value.
    ///
    /// PHILOSOPHY I: every branch returns Str / Bytes / Bool / Lax — never
    /// Unit / `@()`. Decode failures surface as the empty side of
    /// `Lax[Bytes]`; `randomBytes` entropy failures throw.
    fn try_crypto_func(&mut self, sym: &str, args: &[Expr]) -> Result<Signal, RuntimeError> {
        // Evaluate a Str|Bytes argument into a raw byte vector.
        let eval_str_or_bytes = |me: &mut Self,
                                 idx: usize,
                                 fn_name: &str|
         -> Result<Result<Vec<u8>, Signal>, RuntimeError> {
            let val = match me.eval_expr(&args[idx])? {
                Signal::Value(v) => v,
                other => return Ok(Err(other)),
            };
            match val {
                Value::Bytes(b) => Ok(Ok(Value::bytes_take(b))),
                Value::Str(s) => Ok(Ok(Value::str_take(s).into_bytes())),
                other => Err(RuntimeError {
                    message: format!("{}: input must be Str or Bytes, got {}", fn_name, other),
                }),
            }
        };

        // Arity guard.
        let want = match sym {
            "hmacSha256" | "constantTimeEquals" => 2,
            _ => 1,
        };
        if args.len() != want {
            return Err(RuntimeError {
                message: format!(
                    "{} requires exactly {} argument(s), got {}",
                    sym,
                    want,
                    args.len()
                ),
            });
        }

        match sym {
            "sha256" | "sha512" | "sha384" | "sha224" => {
                let bytes = match eval_str_or_bytes(self, 0, sym)? {
                    Ok(b) => b,
                    Err(sig) => return Ok(sig),
                };
                let hex = match sym {
                    "sha256" => crypto::sha256_hex_bytes(&bytes),
                    "sha512" => crypto::sha512_hex_bytes(&bytes),
                    "sha384" => crypto::sha384_hex_bytes(&bytes),
                    "sha224" => crypto::sha224_hex_bytes(&bytes),
                    _ => unreachable!(),
                };
                Ok(Signal::Value(Value::str(hex)))
            }
            "hmacSha256" => {
                let key = match eval_str_or_bytes(self, 0, sym)? {
                    Ok(b) => b,
                    Err(sig) => return Ok(sig),
                };
                let data = match eval_str_or_bytes(self, 1, sym)? {
                    Ok(b) => b,
                    Err(sig) => return Ok(sig),
                };
                Ok(Signal::Value(Value::str(crypto::hmac_sha256_hex(
                    &key, &data,
                ))))
            }
            "constantTimeEquals" => {
                let a = match eval_str_or_bytes(self, 0, sym)? {
                    Ok(b) => b,
                    Err(sig) => return Ok(sig),
                };
                let b = match eval_str_or_bytes(self, 1, sym)? {
                    Ok(b) => b,
                    Err(sig) => return Ok(sig),
                };
                Ok(Signal::Value(Value::Bool(crypto::constant_time_eq(&a, &b))))
            }
            "hexEncode" => {
                let bytes = match eval_str_or_bytes(self, 0, sym)? {
                    Ok(b) => b,
                    Err(sig) => return Ok(sig),
                };
                Ok(Signal::Value(Value::str(crypto::hex_encode(&bytes))))
            }
            "base64Encode" => {
                let bytes = match eval_str_or_bytes(self, 0, sym)? {
                    Ok(b) => b,
                    Err(sig) => return Ok(sig),
                };
                Ok(Signal::Value(Value::str(crypto::base64_encode(&bytes))))
            }
            "hexDecode" | "base64Decode" => {
                // Decode accepts Str only.
                let val = match self.eval_expr(&args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                let input = match val {
                    Value::Str(s) => Value::str_take(s),
                    other => {
                        return Err(RuntimeError {
                            message: format!("{}: input must be Str, got {}", sym, other),
                        });
                    }
                };
                let decoded = if sym == "hexDecode" {
                    crypto::hex_decode(&input)
                } else {
                    crypto::base64_decode(&input)
                };
                let lax = match decoded {
                    Some(bytes) => super::os::make_lax_success_pub(Value::bytes(bytes)),
                    None => super::os::make_lax_failure_pub(Value::bytes(Vec::new())),
                };
                Ok(Signal::Value(lax))
            }
            "randomBytes" => {
                let val = match self.eval_expr(&args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(other),
                };
                let n = match val {
                    Value::Int(n) => n,
                    other => {
                        return Err(RuntimeError {
                            message: format!("randomBytes: argument must be Int, got {}", other),
                        });
                    }
                };
                // A bad count and an entropy failure are thrown as a
                // CryptoError, so `|==` catches them; the count is at most
                // 256 MiB. The wording is the compiled backends'.
                let crypto_error = |message: String| -> Result<Signal, RuntimeError> {
                    Ok(Signal::Throw(Value::Error(super::value::ErrorValue {
                        error_type: "CryptoError".to_string(),
                        message,
                        fields: Vec::new(),
                    })))
                };
                if n < 0 {
                    return crypto_error("randomBytes: count must be non-negative".to_string());
                }
                if n > 256 * 1024 * 1024 {
                    return crypto_error("randomBytes: count exceeds 256 MiB limit".to_string());
                }
                match crypto::random_bytes(n as usize) {
                    Ok(bytes) => Ok(Signal::Value(Value::bytes(bytes))),
                    Err(e) => crypto_error(e),
                }
            }
            other => Err(RuntimeError {
                message: format!("unknown crypto builtin '{}'", other),
            }),
        }
    }

    /// Prelude functions that can be used as values (`f <= stdout`,
    /// `Map[xs, jsonEncode]()`, `x => debug => y`). A name the program
    /// binds itself shadows the prelude one.
    pub(crate) const PRELUDE_FUNCTION_VALUES: &'static [&'static str] = &[
        "stdout",
        "stderr",
        "stdin",
        "stdinLine",
        "nowMs",
        "sleep",
        "jsonEncode",
        "jsonPretty",
        "debug",
        "range",
        "exit",
        "hashMap",
        "setOf",
        "strOf",
        "zip",
        "enumerate",
        "typeof",
        "assert",
        "throw",
    ];

    /// The value of the unbound name `name` when it is a prelude function.
    pub(crate) fn prelude_function_value(&self, name: &str) -> Option<Value> {
        if !Self::PRELUDE_FUNCTION_VALUES.contains(&name) {
            return None;
        }
        Some(Value::Function(FuncValue {
            name: format!("{}{}", PRELUDE_FUNCTION_PREFIX, name),
            params: Vec::new(),
            type_params_count: 0,
            body: Vec::new(),
            closure: Arc::new(std::collections::HashMap::new()),
            return_type: None,
            module: self.active_module,
            def_site: None,
        }))
    }

    /// Call a prelude function through its function value: the arguments
    /// reach the builtin exactly as in a direct call.
    pub(crate) fn call_prelude_function_value(
        &mut self,
        name: &str,
        arg_values: Vec<Value>,
    ) -> Result<Signal, RuntimeError> {
        let span = crate::lexer::Span::new(0, 0, 0, 0);
        let mut scope = std::collections::HashMap::new();
        let args: Vec<Expr> = arg_values
            .into_iter()
            .enumerate()
            .map(|(i, value)| {
                let arg_name = format!("__prelude_arg_{}", i);
                scope.insert(arg_name.clone(), value);
                Expr::Ident(arg_name, span.clone())
            })
            .collect();
        // The arguments live in a scope of their own; no binding of the
        // program can stand in for the builtin.
        let mark = self.env.mark();
        self.env
            .push_linked_scope(scope, crate::interpreter::env::Link::End);
        let result = self.try_builtin_func(name, &args);
        self.env.truncate(mark);
        match result? {
            Some(signal) => Ok(signal),
            None => Err(RuntimeError {
                message: format!("Unknown prelude function '{}'", name),
            }),
        }
    }

    /// Call through a dispatch sentinel an import bound to `name`.
    ///
    /// `taida-lang/os` and `taida-lang/pool` builtins are matched by
    /// their export name, which the sentinel carries — so an aliased
    /// import (`>>> taida-lang/os => @(readFile: rf)`) reaches the same
    /// builtin. The other families resolve the sentinel through the
    /// environment themselves.
    pub(crate) fn try_imported_builtin(
        &mut self,
        name: &str,
        tag: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        if let Some(sym) = tag.strip_prefix("__os_builtin_") {
            return self.try_os_func(sym, args);
        }
        if let Some(sym) = tag.strip_prefix("__pool_builtin_") {
            return self.try_pool_func(sym, args);
        }
        self.try_builtin_func(name, args)
    }

    /// Try to handle a built-in function call (prelude functions).
    pub(crate) fn try_builtin_func(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        // RC1 Phase 4: addon-backed function dispatch.
        //
        // The sentinel `__taida_addon_call::<package>::<function>` is
        // structurally distinct from every other builtin sentinel
        // (`__os_builtin_*`, `__net_builtin_*`, `__crypto_builtin_*`,
        // etc. — all underscore-flat single-segment names) so the
        // guard cannot collide. We check this *first* so addon
        // dispatch happens before any prelude shadowing.
        #[cfg(feature = "native")]
        {
            if let Some(signal) = self.try_addon_func(name, args)? {
                return Ok(Some(signal));
            }
        }

        // taida-lang/crypto imported symbol dispatch.
        // We intentionally do not provide prelude-level crypto compatibility:
        // the symbols resolve only through the `__crypto_builtin_*` sentinel
        // injected on import.
        if let Some(Value::Str(tag)) = self.env.get(name)
            && let Some(sym) = tag.as_str().strip_prefix("__crypto_builtin_")
        {
            let sym = sym.to_string();
            return self.try_crypto_func(&sym, args).map(Some);
        }

        // taida-lang/pool runtime dispatch.
        // Current parity policy keeps these functions callable without strict import gating.
        if let Some(result) = self.try_pool_func(name, args)? {
            return Ok(Some(result));
        }

        // taida-lang/abi request/response helper dispatch.
        if let Some(result) = self.try_abi_func(name, args)? {
            return Ok(Some(result));
        }

        match name {
            "throw" => {
                if args.len() != 1 {
                    return Err(RuntimeError {
                        message: format!("throw requires exactly 1 argument, got {}", args.len()),
                    });
                }
                let val = match self.eval_expr(&args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Throw(val)))
            }
            // ── exit(code): terminate the process with the given code ──
            // F62B-030: documented in prelude.md §6.1 (and registered in the
            // checker) since gen-D, but never implemented — `exit(2)` died
            // with `Undefined variable: 'exit'`. The declared return type is
            // `:Int` (the annotation must name a concrete type, PHILOSOPHY I)
            // but control never reaches past the call. Buffered output is
            // flushed by `exit()`'s stdio cleanup; the interpreter's own
            // buffered mode flushes explicitly so REPL / harness captures
            // are not lost.
            "exit" => {
                let code = match args.first() {
                    Some(arg) => match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(n)) => n,
                        Signal::Value(v) => {
                            return Err(RuntimeError {
                                message: format!(
                                    "exit: argument must be an Int exit code, got {}",
                                    v
                                ),
                            });
                        }
                        other => return Ok(Some(other)),
                    },
                    None => {
                        return Err(RuntimeError {
                            message: "exit requires 1 argument: exit(code)".into(),
                        });
                    }
                };
                if !self.output.is_empty() {
                    use std::io::Write;
                    let stdout = std::io::stdout();
                    let mut lock = stdout.lock();
                    for line in &self.output {
                        let _ = writeln!(lock, "{}", line);
                    }
                    let _ = lock.flush();
                }
                std::process::exit(code as i32);
            }

            // ── stdout(...args): write to output buffer (prelude) ──
            // C12-5 (FB-18): returns the byte count (Int) of the written
            // content so that `n <= stdout("hi")` binds `n = 2`. `Value::Unit`
            // must not escape to Taida surface (null/undefined complete
            // elimination, PHILOSOPHY I). Byte count excludes the implicit
            // trailing newline so it matches the payload the user supplied.
            "stdout" => {
                let mut parts = Vec::new();
                for arg in args {
                    let val = match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    };
                    parts.push(val.to_display_string());
                }
                let joined = parts.join("");
                let bytes = joined.len() as i64;
                // C22-2 / C22B-002: 2-mode split.
                // - stream mode: write to real stdout with immediate flush so that
                //   progress / spinner / TUI (via terminal.Write) / printf-debug
                //   all surface to the user in real time. `writeln!` + `flush().ok()`
                //   (not `println!`) is deliberate — C22-4 (SIGPIPE) relies on the
                //   write error being silently absorbed, and `println!` panics on
                //   BrokenPipe. The implicit `\n` is preserved (existing behavior).
                // - buffered mode: keep the legacy `self.output.push` so REPL /
                //   in-process test harness / JS codegen embedding continue to
                //   capture output via the Vec.
                if self.stream_stdout {
                    use std::io::Write;
                    let stdout = std::io::stdout();
                    let mut lock = stdout.lock();
                    let _ = writeln!(lock, "{}", joined);
                    let _ = lock.flush();
                    self.stdout_emissions += 1;
                } else {
                    self.output.push(joined);
                }
                Ok(Some(Signal::Value(Value::Int(bytes))))
            }

            // ── stderr(...args): write to stderr (prelude) ──
            // C12-5 (FB-18): mirrors `stdout` — returns bytes written as Int.
            "stderr" => {
                let mut parts = Vec::new();
                for arg in args {
                    let val = match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    };
                    parts.push(val.to_display_string());
                }
                let joined = parts.join("");
                let bytes = joined.len() as i64;
                eprintln!("{}", joined);
                Ok(Some(Signal::Value(Value::Int(bytes))))
            }

            // ── stdin(prompt?): read line from stdin (prelude) ──
            "stdin" => {
                use std::io::{self, BufRead, Write};
                // Optional prompt
                if let Some(arg) = args.first() {
                    let prompt_val = match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    };
                    print!("{}", prompt_val.to_display_string());
                    io::stdout().flush().ok();
                }
                let stdin = io::stdin();
                let mut line = String::new();
                match stdin.lock().read_line(&mut line) {
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                        }
                        Ok(Some(Signal::Value(Value::str(line))))
                    }
                    // C20-3 (ROOT-9): previously threw IoError, but JS and
                    // Native silently returned "" on failure, breaking
                    // 3-backend parity. Align on the silently-empty fallback
                    // (default-value guarantee). Callers that need
                    // error-awareness must switch to the new `stdinLine`
                    // API which returns `Async[Lax[Str]]`.
                    Err(_) => Ok(Some(Signal::Value(Value::str(String::new())))),
                }
            }

            // ── stdinLine(prompt?): UTF-8-aware read line (prelude) ──
            //
            // C20-2 (ROOT-7): the cooked-mode `stdin` above deletes one
            // byte at a time when Backspace is pressed, which corrupts
            // multibyte UTF-8 codepoints. `stdinLine` instead delegates
            // to `rustyline`, whose default editor treats Backspace as a
            // char-wide deletion and implements arrow-key / Ctrl-U / etc.
            // line-editing that callers expect from a modern REPL.
            //
            // Return shape: `Async[Lax[Str]]`. The Async wrapper exists
            // purely so that the JS backend (node:readline/promises is
            // async-only) and the Interpreter / Native backends (rustyline
            // / linenoise are synchronous) can share a single surface
            // type. The wrapper is fulfilled immediately on this backend;
            // callers unmold it with `>=> line` to obtain the `Lax[Str]`.
            //
            // Failure modes (all collapse to `Lax[Str].failure("")` so the
            // default-value guarantee is preserved):
            //   * `DefaultEditor::new()` failed (no TTY, no tcgetattr, …)
            //   * `readline` returned `Eof` (pipe / ^D)
            //   * `readline` returned `Interrupted` (^C)
            //   * any other IO / utf-8 error
            "stdinLine" => {
                use rustyline::{DefaultEditor, error::ReadlineError};
                let prompt = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(v) => v.to_display_string(),
                        other => return Ok(Some(other)),
                    }
                } else {
                    String::new()
                };
                let inner = match DefaultEditor::new() {
                    Ok(mut rl) => match rl.readline(&prompt) {
                        Ok(line) => super::os::make_lax_success_pub(Value::str(line)),
                        Err(ReadlineError::Eof)
                        | Err(ReadlineError::Interrupted)
                        | Err(_) => super::os::make_lax_failure_pub(Value::str(String::new())),
                    },
                    Err(_) => super::os::make_lax_failure_pub(Value::str(String::new())),
                };
                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Fulfilled,
                    value: Box::new(inner),
                    error: Box::new(Value::Unit),
                    task: None,
                }))))
            }

            // ── nowMs(): wall-clock epoch milliseconds (prelude) ──
            "nowMs" => {
                if !args.is_empty() {
                    return Err(RuntimeError {
                        message: format!("nowMs takes no arguments, got {}", args.len()),
                    });
                }
                let ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| RuntimeError {
                        message: format!("nowMs failed to read system clock: {}", e),
                    })?
                    .as_millis();
                if ms > i64::MAX as u128 {
                    return Err(RuntimeError {
                        message: "nowMs overflowed Int range".to_string(),
                    });
                }
                Ok(Some(Signal::Value(Value::Int(ms as i64))))
            }

            // ── sleep(ms): Async[Int] wait primitive (F42 sweep) ──
            // Resolves to the requested ms count (elapsed time approximation).
            "sleep" => {
                const MAX_SLEEP_MS: i64 = 2_147_483_647;
                if args.len() != 1 {
                    return Err(RuntimeError {
                        message: format!(
                            "sleep requires exactly 1 argument (ms), got {}",
                            args.len()
                        ),
                    });
                }
                let ms = match self.eval_expr(&args[0])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!("sleep: ms must be Int, got {}", v),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                if !(0..=MAX_SLEEP_MS).contains(&ms) {
                    let err = Value::Error(super::value::ErrorValue {
                        error_type: "RangeError".into(),
                        message: format!(
                            "sleep: ms must be in range 0..={MAX_SLEEP_MS}, got {}",
                            ms
                        ),
                        fields: Vec::new(),
                    });
                    return Ok(Some(Signal::Value(Value::Async(AsyncValue {
                        status: AsyncStatus::Rejected,
                        value: Box::new(Value::Int(0)),
                        error: Box::new(err),
                        task: None,
                    }))));
                }

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms as u64)).await;
                    let _ = tx.send(Ok(Value::Int(ms)));
                });

                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Int(0)),
                    error: Box::new(Value::Int(0)),
                    task: Some(Arc::new(Mutex::new(PendingState::Waiting(rx)))),
                }))))
            }

            // ── jsonParse — ABOLISHED (Molten Iron) ──
            "jsonParse" => {
                Err(RuntimeError {
                    message: "jsonParse has been removed. Use JSON[raw, Schema]() to cast JSON through a schema.".to_string(),
                })
            }

            // ── jsonEncode(value): convert value to JSON string (prelude) ──
            "jsonEncode" => {
                let val = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    }
                } else {
                    Value::Unit
                };
                // C18-2: route through the enum-aware encoder so that
                // Enum values (`Value::EnumVal(enum_name, ordinal)`) are
                // emitted as their declared variant-name Str, symmetric
                // with the C16 `JSON[raw, Schema]()` decoder.
                let json = match &val {
                    Value::Json(j) => j.clone(),
                    _ => crate::interpreter::json::taida_value_to_json_with_enum_defs(
                        &val,
                        &self.enum_variants,
                    ),
                };
                Ok(Some(Signal::Value(Value::str(
                    serde_json::to_string(&json).unwrap_or_default(),
                ))))
            }

            // ── jsonDecode — ABOLISHED (Molten Iron) ──
            "jsonDecode" => {
                Err(RuntimeError {
                    message: "jsonDecode has been removed. Use JSON[raw, Schema]() to cast JSON through a schema.".to_string(),
                })
            }

            // ── jsonPretty(value): convert value to pretty JSON string (prelude) ──
            "jsonPretty" => {
                let val = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    }
                } else {
                    Value::Unit
                };
                // C18-2: route through the enum-aware encoder for variant-
                // name Str output; matches jsonEncode / C16 symmetry.
                let json = match &val {
                    Value::Json(j) => j.clone(),
                    _ => crate::interpreter::json::taida_value_to_json_with_enum_defs(
                        &val,
                        &self.enum_variants,
                    ),
                };
                Ok(Some(Signal::Value(Value::str(
                    serde_json::to_string_pretty(&json).unwrap_or_default(),
                ))))
            }

            // ── jsonFrom — ABOLISHED (Molten Iron) ──
            "jsonFrom" => {
                Err(RuntimeError {
                    message: "jsonFrom has been removed. Use JSON[raw, Schema]() to cast JSON through a schema.".to_string(),
                })
            }

            // ── debug: casual output (no label) or labeled debug output ──
            // debug(value)          → prints value as-is (casual output)
            // debug(value, "label") → prints [label] Type: value (debug output)
            "debug" => {
                let val = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    }
                } else {
                    Value::Unit
                };
                let label = if let Some(label_arg) = args.get(1) {
                    match self.eval_expr(label_arg)? {
                        Signal::Value(Value::Str(s)) => Some(s),
                        Signal::Value(_) => None,
                        other => return Ok(Some(other)),
                    }
                } else {
                    None
                };
                let line = if let Some(label) = label {
                    let type_name = Self::type_name_of(&val);
                    format!("[{}] {}: {}", label, type_name, val.to_debug_string())
                } else {
                    val.to_display_string()
                };
                // C22-2 / C22B-002: 2-mode split, symmetric with `stdout`.
                //
                // Stream mode: `debug` writes to **stdout** (not stderr). This
                // matches the observable behavior of the JS backend
                // (`__taida_debug` uses `console.log`, which writes to stdout
                // on Node.js) and the WASM / Native backends (all of which
                // emit `debug` output to stdout). Routing to stderr would
                // break 3-backend parity on `test_native_compile_parity` and
                // similar tests that diff captured stdout across backends.
                //
                // The flush-timing difference with `stdout` is still the
                // primary win: previously `debug` accumulated in the Vec and
                // only surfaced after `eval_program` returned, making it
                // unusable for progress / printf-debug in long-running CLI
                // scripts. Stream mode flushes each `debug` call immediately.
                //
                // Buffered mode keeps the legacy Vec push for REPL / test
                // captures that depend on Vec contents.
                if self.stream_stdout {
                    use std::io::Write;
                    let stdout = std::io::stdout();
                    let mut lock = stdout.lock();
                    let _ = writeln!(lock, "{}", line);
                    let _ = lock.flush();
                    self.stdout_emissions += 1;
                } else {
                    self.output.push(line);
                }
                Ok(Some(Signal::Value(val)))
            }

            // ── Some() — ABOLISHED ──
            "Some" => {
                Err(RuntimeError {
                    message:
                        "Some() has been removed. Optional is abolished. Use Lax[value]() instead."
                            .to_string(),
                })
            }

            // ── None() — ABOLISHED ──
            "None" => {
                Err(RuntimeError {
                    message:
                        "None() has been removed. Optional is abolished. Use Lax[value]() instead."
                            .to_string(),
                })
            }

            // ── Lax(value): create Lax with value (convenience function) ──
            "Lax" => {
                let val = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    }
                } else {
                    Value::Unit
                };
                let default_value = Self::default_for_value(&val);
                Ok(Some(Signal::Value(Value::pack(vec![
                    ("has_value".into(), Value::Bool(true)),
                    ("__value".into(), val),
                    ("__default".into(), default_value),
                    ("__type".into(), Value::str("Lax".into())),
                ]))))
            }

            // ── JSON() — ABOLISHED (Molten Iron) ──
            // JSON must be used as a mold: JSON[raw, Schema]()
            "JSON" => {
                Err(RuntimeError {
                    message: "JSON requires a schema type argument: JSON[raw, Schema](). Raw JSON cannot be used without a schema.".to_string(),
                })
            }

            // ── Regex(pattern, flags?): C12 Phase 6 (FB-5 Phase 2-3) ──
            // Build a :Regex BuchiPack. Validated eagerly so invalid
            // patterns / flags fail at construction time rather than
            // during first method dispatch. Philosophy I — no silent
            // undefined Regex values; construction either yields a
            // typed pack or throws a `ValueError`.
            "Regex" => {
                if args.is_empty() || args.len() > 2 {
                    return Err(RuntimeError {
                        message: format!(
                            "Regex requires 1 or 2 arguments (pattern, flags?), got {}",
                            args.len()
                        ),
                    });
                }
                let pattern = match self.eval_expr(&args[0])? {
                    Signal::Value(Value::Str(s)) => s,
                    Signal::Value(v) => {
                        return Err(RuntimeError {
                            message: format!(
                                "Regex: pattern must be Str, got {}",
                                Self::type_name_of(&v)
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let flags: String = if let Some(arg) = args.get(1) {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::Str(s)) => Value::str_take(s),
                        Signal::Value(v) => {
                            return Err(RuntimeError {
                                message: format!(
                                    "Regex: flags must be Str, got {}",
                                    Self::type_name_of(&v)
                                ),
                            });
                        }
                        other => return Ok(Some(other)),
                    }
                } else {
                    String::new()
                };
                match super::regex::build_regex_value(&pattern, &flags) {
                    Ok(v) => Ok(Some(Signal::Value(v))),
                    Err(msg) => Ok(Some(Signal::Throw(Value::Error(
                        super::value::ErrorValue {
                            error_type: "ValueError".to_string(),
                            message: msg,
                            fields: Vec::new(),
                        },
                    )))),
                }
            }

            // ── Ok() — ABOLISHED ──
            "Ok" => {
                Err(RuntimeError {
                    message: "Ok() has been removed. Use Result[value]() instead.".to_string(),
                })
            }

            // ── Err() — ABOLISHED ──
            "Err" => {
                Err(RuntimeError {
                    message: "Err() has been removed. Use Result[value](throw <= error) instead."
                        .to_string(),
                })
            }

            // ── hashMap(entries): create HashMap ──
            // A list of `@(key, value)` packs (as `entries()` returns) or a
            // pack, whose fields become entries keyed by the field names.
            // Entries are stored as successive `.set(key, value)` calls
            // store them: a later duplicate key replaces the value and
            // keeps the first position. A list element pack without `key`
            // and `value` fields gives its first two fields, and a list
            // element list its first two elements.
            "hashMap" => {
                let mut entries: Vec<(Value, Value)> = Vec::new();
                let mut add = |key: Value, value: Value| {
                    match entries.iter_mut().find(|(k, _)| *k == key) {
                        Some(entry) => entry.1 = value,
                        None => entries.push((key, value)),
                    }
                };
                if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => {
                            for item in items.iter() {
                                match item {
                                    Value::BuchiPack(fields) => {
                                        let named = |name: &str| {
                                            fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
                                        };
                                        if let (Some(key), Some(value)) =
                                            (named("key"), named("value"))
                                        {
                                            add(key.clone(), value.clone());
                                        } else if fields.len() >= 2 {
                                            add(fields[0].1.clone(), fields[1].1.clone());
                                        }
                                    }
                                    Value::List(pair) if pair.len() >= 2 => {
                                        add(pair[0].clone(), pair[1].clone());
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Signal::Value(Value::BuchiPack(fields)) => {
                            for (name, value) in fields.iter() {
                                if !name.starts_with("__") {
                                    add(Value::str(name.clone()), value.clone());
                                }
                            }
                        }
                        Signal::Value(_) => {}
                        other => return Ok(Some(other)),
                    }
                }
                let entries: Vec<Value> = entries
                    .into_iter()
                    .map(|(key, value)| {
                        Value::pack(vec![("key".into(), key), ("value".into(), value)])
                    })
                    .collect();
                Ok(Some(Signal::Value(Value::pack(vec![
                    ("__entries".into(), Value::list(entries)),
                    ("__type".into(), Value::str("HashMap".into())),
                ]))))
            }

            // ── setOf(items): create Set ──
            "setOf" => {
                let items = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => {
                            // Deduplicate
                            let mut unique = Vec::new();
                            for item in Value::list_take(items) {
                                if !unique.contains(&item) {
                                    unique.push(item);
                                }
                            }
                            unique
                        }
                        Signal::Value(_) => Vec::new(),
                        other => return Ok(Some(other)),
                    }
                } else {
                    Vec::new()
                };
                Ok(Some(Signal::Value(Value::pack(vec![
                    ("__items".into(), Value::list(items)),
                    ("__type".into(), Value::str("Set".into())),
                ]))))
            }

            // ── typeof(x): return type name as string ──
            "typeof" => {
                let val = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    }
                } else {
                    Value::Unit
                };
                Ok(Some(Signal::Value(Value::str(
                    Self::type_name_of(&val).to_string(),
                ))))
            }

            // ── range(start, end): generate integer list ──
            "range" => {
                const MAX_RANGE_ITEMS: i64 = 1_000_000;
                let start = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(n)) => n,
                        Signal::Value(_) => 0,
                        other => return Ok(Some(other)),
                    }
                } else {
                    0
                };
                let end = if let Some(arg) = args.get(1) {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::Int(n)) => n,
                        Signal::Value(_) => 0,
                        other => return Ok(Some(other)),
                    }
                } else {
                    0
                };
                let len = end.saturating_sub(start);
                if len > MAX_RANGE_ITEMS {
                    return Ok(Some(Signal::Throw(Value::Error(super::value::ErrorValue {
                        error_type: "RangeError".into(),
                        message: format!(
                            "range(start, end) would create {} items; maximum is {}",
                            len, MAX_RANGE_ITEMS
                        ),
                        fields: Vec::new(),
                    }))));
                }
                let list: Vec<Value> = (start..end).map(Value::Int).collect();
                Ok(Some(Signal::Value(Value::list(list))))
            }

            // ── enumerate(list): add indices ──
            "enumerate" => {
                let list = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => Value::list_take(items),
                        Signal::Value(_) => Vec::new(),
                        other => return Ok(Some(other)),
                    }
                } else {
                    Vec::new()
                };
                let result: Vec<Value> = list
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| {
                        Value::pack(vec![
                            ("index".into(), Value::Int(i as i64)),
                            ("value".into(), v),
                        ])
                    })
                    .collect();
                Ok(Some(Signal::Value(Value::list(result))))
            }

            // ── zip(a, b): combine two lists ──
            "zip" => {
                let list_a = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => Value::list_take(items),
                        Signal::Value(_) => Vec::new(),
                        other => return Ok(Some(other)),
                    }
                } else {
                    Vec::new()
                };
                let list_b = if let Some(arg) = args.get(1) {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => Value::list_take(items),
                        Signal::Value(_) => Vec::new(),
                        other => return Ok(Some(other)),
                    }
                } else {
                    Vec::new()
                };
                let result: Vec<Value> = list_a
                    .into_iter()
                    .zip(list_b)
                    .map(|(a, b)| Value::pack(vec![("first".into(), a), ("second".into(), b)]))
                    .collect();
                Ok(Some(Signal::Value(Value::list(result))))
            }

            // ── assert(cond, msg): throw if condition is false ──
            "assert" => {
                let cond = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(v) => v.is_truthy(),
                        other => return Ok(Some(other)),
                    }
                } else {
                    false
                };
                if !cond {
                    let msg = if let Some(arg) = args.get(1) {
                        match self.eval_expr(arg)? {
                            Signal::Value(v) => v.to_display_string(),
                            other => return Ok(Some(other)),
                        }
                    } else {
                        "Assertion failed".to_string()
                    };
                    return Ok(Some(Signal::Throw(Value::Error(
                        super::value::ErrorValue {
                            error_type: "AssertionError".into(),
                            message: msg,
                            fields: Vec::new(),
                        },
                    ))));
                }
                Ok(Some(Signal::Value(Value::Bool(true))))
            }

            // ── D28B-015: `strOf(span, raw)` lowercase function-form ──
            // Cold-path span → owned Str materialization, the function-form
            // counterpart of the existing `StrOf[span, raw]()` mold (registered
            // in `mold.rs::lookup_mold_call`).
            //
            // The 2026-04-26 D28B-001 naming-rules Lock justifies the
            // co-existence: `Str` / `StrOf` are mold/PascalCase, `strOf` is the
            // function/camelCase entry. They produce identical results in all
            // 4 backends (interpreter / JS / native / wasm-full).
            //
            // Semantics (matching `StrOf` mold, `mold.rs:3004`):
            //   - extract `(start, len)` from span pack
            //   - clamp `end = start + len` against `raw.len()`
            //   - try UTF-8 decode the slice; invalid UTF-8 → empty Str
            //   - non-pack span / non-Bytes/Str raw → empty Str (tolerant)
            "strOf" => {
                if args.len() != 2 {
                    return Err(RuntimeError {
                        message: format!(
                            "strOf requires exactly 2 arguments: strOf(span, raw), got {}",
                            args.len()
                        ),
                    });
                }
                let span = match self.eval_expr(&args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let raw = match self.eval_expr(&args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let result = match (
                    super::mold::extract_span_pack(&span),
                    super::mold::raw_as_bytes(&raw),
                ) {
                    (Some((start, len)), Some(bytes)) => {
                        let end = start.saturating_add(len);
                        if end <= bytes.len() {
                            std::str::from_utf8(&bytes[start..end])
                                .map(|s| s.to_string())
                                .unwrap_or_default()
                        } else {
                            String::new()
                        }
                    }
                    _ => String::new(),
                };
                Ok(Some(Signal::Value(Value::str(result))))
            }

            // ── Net functions (sentinel-guarded), then OS functions ──
            _ => match self.try_net_func(name, args)? {
                Some(signal) => Ok(Some(signal)),
                None => self.try_os_func(name, args),
            },
        }
    }
}

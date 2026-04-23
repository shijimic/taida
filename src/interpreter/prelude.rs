use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::{AsyncStatus, AsyncValue, PendingState, Value};
use crate::crypto::sha256_hex_bytes;
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
                for (name, v) in fields {
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
            Value::Json(_) => "JSON",
            Value::Molten => "Molten",
            Value::Stream(_) => "Stream",
            // C18-2: EnumVal reports as "Int" so that prelude `typeOf` /
            // `typeof` builtins return the same type name they did before
            // Phase 2. Existing user code expects Enum values to `typeOf`
            // as Int because the internal representation has always been
            // Int(ordinal); we preserve that observable behaviour.
            Value::EnumVal(_, _) => "Int",
        }
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
        // We intentionally do not provide prelude-level sha256 compatibility.
        if matches!(
            self.env.get(name),
            Some(Value::Str(tag)) if tag == "__crypto_builtin_sha256"
        ) {
            if args.len() != 1 {
                return Err(RuntimeError {
                    message: format!("sha256 requires exactly 1 argument, got {}", args.len()),
                });
            }
            let val = match self.eval_expr(&args[0])? {
                Signal::Value(v) => v,
                other => return Ok(Some(other)),
            };
            let bytes: Vec<u8> = match val {
                Value::Bytes(b) => b,
                Value::Str(s) => s.into_bytes(),
                other => other.to_display_string().into_bytes(),
            };
            return Ok(Some(Signal::Value(Value::Str(sha256_hex_bytes(&bytes)))));
        }

        // taida-lang/pool runtime dispatch.
        // Current parity policy keeps these functions callable without strict import gating.
        if let Some(result) = self.try_pool_func(name, args)? {
            return Ok(Some(result));
        }

        match name {
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
                        Ok(Some(Signal::Value(Value::Str(line))))
                    }
                    // C20-3 (ROOT-9): previously threw IoError, but JS and
                    // Native silently returned "" on failure, breaking
                    // 3-backend parity. Align on the silently-empty fallback
                    // (default-value guarantee). Callers that need
                    // error-awareness must switch to the new `stdinLine`
                    // API which returns `Async[Lax[Str]]`.
                    Err(_) => Ok(Some(Signal::Value(Value::Str(String::new())))),
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
            // callers unmold it with `]=> line` to obtain the `Lax[Str]`.
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
                        Ok(line) => super::os_eval::make_lax_success_pub(Value::Str(line)),
                        Err(ReadlineError::Eof)
                        | Err(ReadlineError::Interrupted)
                        | Err(_) => super::os_eval::make_lax_failure_pub(Value::Str(String::new())),
                    },
                    Err(_) => super::os_eval::make_lax_failure_pub(Value::Str(String::new())),
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

            // ── sleep(ms): Async[Unit] wait primitive (prelude) ──
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
                        value: Box::new(Value::Unit),
                        error: Box::new(err),
                        task: None,
                    }))));
                }

                let (tx, rx) = tokio::sync::oneshot::channel();
                self.tokio_runtime.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms as u64)).await;
                    let _ = tx.send(Ok(Value::Unit));
                });

                Ok(Some(Signal::Value(Value::Async(AsyncValue {
                    status: AsyncStatus::Pending,
                    value: Box::new(Value::Unit),
                    error: Box::new(Value::Unit),
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
                        &self.enum_defs,
                    ),
                };
                Ok(Some(Signal::Value(Value::Str(
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
                        &self.enum_defs,
                    ),
                };
                Ok(Some(Signal::Value(Value::Str(
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
                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("hasValue".into(), Value::Bool(true)),
                    ("__value".into(), val),
                    ("__default".into(), default_value),
                    ("__type".into(), Value::Str("Lax".into())),
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
                let flags = if let Some(arg) = args.get(1) {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::Str(s)) => s,
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
                match super::regex_eval::build_regex_value(&pattern, &flags) {
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
            "hashMap" => {
                let entries = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => {
                            let mut entries = Vec::new();
                            for item in items.iter() {
                                if let Value::BuchiPack(fields) = item {
                                    // Try tuple-like @(first, second) or @(key, value) patterns
                                    if fields.len() >= 2 {
                                        let key = fields[0].1.clone();
                                        let value = fields[1].1.clone();
                                        entries.push(Value::BuchiPack(vec![
                                            ("key".into(), key),
                                            ("value".into(), value),
                                        ]));
                                    }
                                } else if let Value::List(pair) = item
                                    && pair.len() >= 2 {
                                        entries.push(Value::BuchiPack(vec![
                                            ("key".into(), pair[0].clone()),
                                            ("value".into(), pair[1].clone()),
                                        ]));
                                    }
                            }
                            entries
                        }
                        Signal::Value(Value::BuchiPack(fields)) => {
                            // BuchiPack argument: each field becomes a key-value entry
                            // hashMap(@(a <= 1, b <= 2)) -> [{key: "a", value: 1}, {key: "b", value: 2}]
                            let mut entries = Vec::new();
                            for (name, value) in &fields {
                                entries.push(Value::BuchiPack(vec![
                                    ("key".into(), Value::Str(name.clone())),
                                    ("value".into(), value.clone()),
                                ]));
                            }
                            entries
                        }
                        Signal::Value(_) => Vec::new(),
                        other => return Ok(Some(other)),
                    }
                } else {
                    Vec::new()
                };
                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("__entries".into(), Value::list(entries)),
                    ("__type".into(), Value::Str("HashMap".into())),
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
                Ok(Some(Signal::Value(Value::BuchiPack(vec![
                    ("__items".into(), Value::list(items)),
                    ("__type".into(), Value::Str("Set".into())),
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
                Ok(Some(Signal::Value(Value::Str(
                    Self::type_name_of(&val).to_string(),
                ))))
            }

            // ── range(start, end): generate integer list ──
            "range" => {
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
                        Value::BuchiPack(vec![
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
                    .map(|(a, b)| Value::BuchiPack(vec![("first".into(), a), ("second".into(), b)]))
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

            // ── Net functions (sentinel-guarded), then OS functions ──
            _ => match self.try_net_func(name, args)? {
                Some(signal) => Ok(Some(signal)),
                None => self.try_os_func(name, args),
            },
        }
    }
}

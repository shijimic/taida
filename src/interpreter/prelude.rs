use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::{AsyncStatus, AsyncValue, PendingState, Value};
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
use std::fmt::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256_hex_bytes(input: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut msg = input.to_vec();
    let bit_len = (msg.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 4;
            *word = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = String::with_capacity(64);
    for word in h {
        let _ = write!(out, "{:08x}", word);
    }
    out
}

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
        }
    }

    /// Try to handle a built-in function call (prelude functions).
    pub(crate) fn try_builtin_func(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
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
            "stdout" => {
                let mut parts = Vec::new();
                for arg in args {
                    let val = match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    };
                    parts.push(val.to_display_string());
                }
                self.output.push(parts.join(""));
                Ok(Some(Signal::Value(Value::Unit)))
            }

            // ── stderr(...args): write to stderr (prelude) ──
            "stderr" => {
                let mut parts = Vec::new();
                for arg in args {
                    let val = match self.eval_expr(arg)? {
                        Signal::Value(v) => v,
                        other => return Ok(Some(other)),
                    };
                    parts.push(val.to_display_string());
                }
                eprintln!("{}", parts.join(""));
                Ok(Some(Signal::Value(Value::Unit)))
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
                    Err(e) => Ok(Some(Signal::Throw(Value::Error(
                        super::value::ErrorValue {
                            error_type: "IoError".to_string(),
                            message: format!("Cannot read from stdin: {}", e),
                            fields: Vec::new(),
                        },
                    )))),
                }
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
                match crate::interpreter::json::stdlib_json_encode(&[val]) {
                    Ok(result) => Ok(Some(Signal::Value(result))),
                    Err(e) => Err(RuntimeError { message: e }),
                }
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
                match crate::interpreter::json::stdlib_json_pretty(&[val]) {
                    Ok(result) => Ok(Some(Signal::Value(result))),
                    Err(e) => Err(RuntimeError { message: e }),
                }
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
                if let Some(label) = label {
                    let type_name = Self::type_name_of(&val);
                    self.output.push(format!(
                        "[{}] {}: {}",
                        label,
                        type_name,
                        val.to_debug_string()
                    ));
                } else {
                    self.output.push(val.to_display_string());
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
                            for item in &items {
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
                    ("__entries".into(), Value::List(entries)),
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
                            for item in items {
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
                    ("__items".into(), Value::List(items)),
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
                Ok(Some(Signal::Value(Value::List(list))))
            }

            // ── enumerate(list): add indices ──
            "enumerate" => {
                let list = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => items,
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
                Ok(Some(Signal::Value(Value::List(result))))
            }

            // ── zip(a, b): combine two lists ──
            "zip" => {
                let list_a = if let Some(arg) = args.first() {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => items,
                        Signal::Value(_) => Vec::new(),
                        other => return Ok(Some(other)),
                    }
                } else {
                    Vec::new()
                };
                let list_b = if let Some(arg) = args.get(1) {
                    match self.eval_expr(arg)? {
                        Signal::Value(Value::List(items)) => items,
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
                Ok(Some(Signal::Value(Value::List(result))))
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

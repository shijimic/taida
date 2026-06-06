use super::eval::{Interpreter, RuntimeError, Signal};
use super::value::Value;
use crate::parser::Expr;

const ABI_SYMBOLS: &[&str] = &["text", "json", "bytes", "status", "header"];

pub(crate) fn abi_symbols() -> &'static [&'static str] {
    ABI_SYMBOLS
}

impl Interpreter {
    pub(crate) fn try_abi_func(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<Signal>, RuntimeError> {
        let original_name = match self.env.get(name) {
            Some(Value::Str(tag)) if tag.starts_with("__abi_builtin_") => {
                tag["__abi_builtin_".len()..].to_string()
            }
            _ => return Ok(None),
        };

        match original_name.as_str() {
            "text" => {
                if args.len() != 1 {
                    return Err(RuntimeError {
                        message: format!("text requires exactly 1 argument, got {}", args.len()),
                    });
                }
                let body = self.eval_str_arg(&args[0], "text")?;
                Ok(Some(Signal::Value(abi_response(
                    200,
                    vec![(
                        "content-type".to_string(),
                        "text/plain; charset=utf-8".to_string(),
                    )],
                    body.into_bytes(),
                ))))
            }
            "json" => {
                if args.len() != 1 {
                    return Err(RuntimeError {
                        message: format!("json requires exactly 1 argument, got {}", args.len()),
                    });
                }
                let value = match self.eval_expr(&args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let json = match &value {
                    Value::Json(j) => j.clone(),
                    _ => crate::interpreter::json::taida_value_to_json_with_enum_defs(
                        &value,
                        &self.enum_defs,
                    ),
                };
                let body = serde_json::to_vec(&json).unwrap_or_default();
                Ok(Some(Signal::Value(abi_response(
                    200,
                    vec![("content-type".to_string(), "application/json".to_string())],
                    body,
                ))))
            }
            "bytes" => {
                if args.len() != 1 {
                    return Err(RuntimeError {
                        message: format!("bytes requires exactly 1 argument, got {}", args.len()),
                    });
                }
                let value = match self.eval_expr(&args[0])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                let body = match value {
                    Value::Bytes(bytes) => Value::bytes_take(bytes),
                    Value::Str(s) => Value::str_take(s).into_bytes(),
                    other => other.to_display_string().into_bytes(),
                };
                Ok(Some(Signal::Value(abi_response(
                    200,
                    vec![(
                        "content-type".to_string(),
                        "application/octet-stream".to_string(),
                    )],
                    body,
                ))))
            }
            "status" => {
                if args.len() != 2 {
                    return Err(RuntimeError {
                        message: format!("status requires exactly 2 arguments, got {}", args.len()),
                    });
                }
                let code = match self.eval_expr(&args[0])? {
                    Signal::Value(Value::Int(n)) => n,
                    Signal::Value(other) => {
                        return Err(RuntimeError {
                            message: format!(
                                "status: first argument must be Int, got {}",
                                Self::type_name_of(&other)
                            ),
                        });
                    }
                    other => return Ok(Some(other)),
                };
                let response = match self.eval_expr(&args[1])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                Ok(Some(Signal::Value(with_response_field(
                    response,
                    "status",
                    Value::Int(clamp_status(code)),
                ))))
            }
            "header" => {
                if args.len() != 3 {
                    return Err(RuntimeError {
                        message: format!("header requires exactly 3 arguments, got {}", args.len()),
                    });
                }
                let key = self.eval_str_arg(&args[0], "header")?;
                let value = self.eval_str_arg(&args[1], "header")?;
                let response = match self.eval_expr(&args[2])? {
                    Signal::Value(v) => v,
                    other => return Ok(Some(other)),
                };
                if !valid_header_name(&key) || !valid_header_value(&value) {
                    return Ok(Some(Signal::Value(invalid_header_response())));
                }
                Ok(Some(Signal::Value(add_response_header(
                    response, key, value,
                ))))
            }
            _ => Ok(None),
        }
    }

    fn eval_str_arg(&mut self, arg: &Expr, func: &str) -> Result<String, RuntimeError> {
        match self.eval_expr(arg)? {
            Signal::Value(Value::Str(s)) => Ok(Value::str_take(s)),
            Signal::Value(other) => Err(RuntimeError {
                message: format!(
                    "{}: argument must be Str, got {}",
                    func,
                    Self::type_name_of(&other)
                ),
            }),
            other => Err(RuntimeError {
                message: format!("{}: unexpected control signal {:?}", func, other),
            }),
        }
    }
}

fn abi_response(status: i64, headers: Vec<(String, String)>, body: Vec<u8>) -> Value {
    Value::pack(vec![
        ("status".to_string(), Value::Int(clamp_status(status))),
        ("headers".to_string(), header_list(headers)),
        ("body".to_string(), Value::bytes(body)),
    ])
}

fn clamp_status(status: i64) -> i64 {
    status.clamp(100, 599)
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b == b'\t' || (b >= 0x20 && b != b'\r' && b != b'\n'))
}

fn invalid_header_response() -> Value {
    abi_response(
        500,
        vec![("x-taida-error".to_string(), "abi".to_string())],
        b"invalid response header".to_vec(),
    )
}

fn header_list(headers: Vec<(String, String)>) -> Value {
    let entries = headers
        .into_iter()
        .map(|(name, value)| {
            Value::pack(vec![
                ("name".to_string(), Value::str(name)),
                ("value".to_string(), Value::str(value)),
            ])
        })
        .collect();
    Value::list(entries)
}

fn with_response_field(response: Value, field_name: &str, field_value: Value) -> Value {
    match response {
        Value::BuchiPack(fields) => {
            // COW: chained helper calls (`header(.., status(.., text(..)))`)
            // hand us the only reference, so `pack_take` moves the fields
            // out without cloning; shared packs fall back to a clone.
            let mut out = Value::pack_take(fields);
            let mut replaced = false;
            for (name, value) in out.iter_mut() {
                if name == field_name {
                    *value = field_value.clone();
                    replaced = true;
                }
            }
            if !replaced {
                out.push((field_name.to_string(), field_value));
            }
            Value::pack(out)
        }
        _ => response,
    }
}

fn add_response_header(response: Value, key: String, value: String) -> Value {
    match response {
        Value::BuchiPack(fields) => {
            // COW: take ownership of the pack fields and append to the
            // headers list in place. A `header(...)` chain of k calls is
            // O(k) total — each link moves the uniquely-owned list and
            // pushes one entry — instead of re-cloning the whole pack and
            // headers list per link (O(k²)).
            let mut out = Value::pack_take(fields);
            let mut appended = false;
            for (name, field_value) in out.iter_mut() {
                if name == "headers" {
                    let headers = std::mem::replace(field_value, Value::default_list());
                    *field_value = append_header(headers, &key, &value);
                    appended = true;
                }
            }
            if !appended {
                out.push((
                    "headers".to_string(),
                    header_list(vec![(key.clone(), value.clone())]),
                ));
            }
            Value::pack(out)
        }
        _ => response,
    }
}

fn append_header(headers: Value, key: &str, value: &str) -> Value {
    if let Value::List(entries) = headers {
        let mut next = Value::list_take(entries);
        next.push(Value::pack(vec![
            ("name".to_string(), Value::str(key.to_string())),
            ("value".to_string(), Value::str(value.to_string())),
        ]));
        return Value::list(next);
    }
    header_list(vec![(key.to_string(), value.to_string())])
}

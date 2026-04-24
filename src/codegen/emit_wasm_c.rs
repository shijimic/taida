/// wasm-min C emitter -- Taida IR を C コードに変換し、clang で wasm32 object を生成
///
/// wasm-min は Cranelift の ISA に wasm32 が存在しないため、IR -> C -> clang -> wasm32 .o
/// というパイプラインを採用する。サポートする IR 命令:
///
/// - ConstInt, ConstFloat, ConstBool, ConstStr
/// - Call (runtime 関数のみ)
/// - CallUser (ユーザー定義関数)
/// - DefVar, UseVar
/// - CondBranch, Return, TailCall
/// - GlobalSet, GlobalGet
/// - Retain, Release (no-op)
/// - PackNew, PackSet, PackSetTag, PackGet (W-4)
/// - FuncAddr, MakeClosure, CallIndirect (W-5)
///
/// 未対応 IR は silent miscompile ではなく compile error を返す。
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use super::ir::*;

/// WASM profile: determines which runtime functions are allowed.
/// - `Min`: wasm-min baseline (no OS APIs)
/// - `Wasi`: wasm-wasi (adds env, file read/write, exists via WASI imports)
/// - `Edge`: wasm-edge (adds env via host imports, no file I/O)
/// - `Full`: wasm-full (wasm-wasi superset + extended runtime: string/number molds, JSON, bytes, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmProfile {
    Min,
    Wasi,
    Edge,
    Full,
}

#[derive(Debug)]
pub struct WasmCEmitError {
    pub message: String,
}

impl std::fmt::Display for WasmCEmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// ---------------------------------------------------------------------------
// F-1: Capability validator -- 未対応 IR を compile error にする
// ---------------------------------------------------------------------------

/// wasm-min で未対応の IR 命令を検出して compile error にする。
/// silent miscompile を防ぐための事前バリデーション。
pub fn validate_wasm_min_capabilities(ir_module: &IrModule) -> Result<(), WasmCEmitError> {
    let mut unsupported = Vec::new();

    for func in &ir_module.functions {
        collect_unsupported_insts(&func.body, &mut unsupported);
    }

    if unsupported.is_empty() {
        Ok(())
    } else {
        // Deduplicate by feature name
        let mut features: Vec<&str> = unsupported.iter().map(|s| s.as_str()).collect();
        features.sort();
        features.dedup();
        Err(WasmCEmitError {
            message: format!(
                "wasm-min does not support the following features: {}. \
                 Use the interpreter or native backend instead.",
                features.join(", ")
            ),
        })
    }
}

/// NET-6 fix: early-out validation for taida-lang/net HTTP API on WASM profiles.
///
/// When `httpParseRequestHead(Bytes[...])` is compiled for wasm-edge, the `Bytes`
/// mold hits a generic unsupported runtime error before the net-specific error.
/// This pre-check fires based on the collected runtime function set, ensuring the
/// net-specific diagnostic always takes priority over argument-level errors.
fn validate_net_http_api_for_wasm(
    needed_funcs: &HashSet<String>,
    profile: WasmProfile,
) -> Result<(), WasmCEmitError> {
    const NET_HTTP_FUNCS: &[(&str, &str)] = &[
        ("taida_net_http_serve", "httpServe"),
        ("taida_net_http_parse_request_head", "httpParseRequestHead"),
        ("taida_net_http_encode_response", "httpEncodeResponse"),
        ("taida_net_read_body", "readBody"),
        // v3 streaming API
        ("taida_net_start_response", "startResponse"),
        ("taida_net_write_chunk", "writeChunk"),
        ("taida_net_end_response", "endResponse"),
        ("taida_net_sse_event", "sseEvent"),
        // v4 request body streaming API
        ("taida_net_read_body_chunk", "readBodyChunk"),
        ("taida_net_read_body_all", "readBodyAll"),
        // v4 WebSocket API
        ("taida_net_ws_upgrade", "wsUpgrade"),
        ("taida_net_ws_send", "wsSend"),
        ("taida_net_ws_receive", "wsReceive"),
        ("taida_net_ws_close", "wsClose"),
        // v5 WebSocket revision
        ("taida_net_ws_close_code", "wsCloseCode"),
    ];

    for &(runtime_name, api_name) in NET_HTTP_FUNCS {
        if needed_funcs.contains(runtime_name) {
            let profile_name = match profile {
                WasmProfile::Min => "wasm-min",
                WasmProfile::Wasi => "wasm-wasi",
                WasmProfile::Edge => "wasm-edge",
                WasmProfile::Full => "wasm-full",
            };
            return Err(WasmCEmitError {
                message: format!(
                    "{} does not support taida-lang/net HTTP API '{}'. \
                     Use the interpreter, JS, or native backend instead.",
                    profile_name, api_name
                ),
            });
        }
    }

    Ok(())
}

/// C12B-023: early-out validation for the Regex type / builtins on WASM profiles.
///
/// The wasm runtime (`runtime_core_wasm/02_containers.inc.c`) only ships
/// *stubs* for `taida_regex_new` / `taida_str_match_regex` /
/// `taida_str_search_regex` — the real POSIX `regex.h` is not linked in any
/// wasm profile (min / wasi / edge / full). Historically these stubs returned
/// `0` / `-1` / forwarded the pattern pointer silently, which produced
/// **undefined behaviour** when a `Regex(...)` pack flowed back through the
/// polymorphic dispatchers. PHILOSOPHY I forbids silent-undefined paths, so
/// we reject the Regex surface at compile time instead.
///
/// Detected by the presence of any of the three Regex-specific runtime
/// helpers in the collected `needed_funcs` set:
///
/// - `taida_regex_new` — emitted for `Regex(pattern, flags?)`
/// - `taida_str_match_regex` — emitted for `str.match(re)`
/// - `taida_str_search_regex` — emitted for `str.search(re)`
///
/// `str.replace(Regex(...), ...)` / `str.replaceAll(Regex(...), ...)` go
/// through the `_poly` dispatchers which are safe for plain-Str callers; the
/// `Regex(...)` construction on the arg side already emits `taida_regex_new`,
/// so those cases are caught transitively.
fn validate_regex_api_for_wasm(
    needed_funcs: &HashSet<String>,
    profile: WasmProfile,
) -> Result<(), WasmCEmitError> {
    const REGEX_FUNCS: &[(&str, &str)] = &[
        ("taida_regex_new", "Regex"),
        ("taida_str_match_regex", "Str.match"),
        ("taida_str_search_regex", "Str.search"),
    ];

    for &(runtime_name, api_name) in REGEX_FUNCS {
        if needed_funcs.contains(runtime_name) {
            let profile_name = match profile {
                WasmProfile::Min => "wasm-min",
                WasmProfile::Wasi => "wasm-wasi",
                WasmProfile::Edge => "wasm-edge",
                WasmProfile::Full => "wasm-full",
            };
            return Err(WasmCEmitError {
                message: format!(
                    "[E1617] {} does not support `{}` (Regex is unavailable on the wasm profile). \
                     Hint: Use the interpreter, JS, or native backend for Regex-based code, \
                     or switch to the fixed-string overloads (`split`, `replace`, `replaceAll` with Str args) on wasm.",
                    profile_name, api_name
                ),
            });
        }
    }

    Ok(())
}

fn collect_unsupported_insts(insts: &[IrInst], _out: &mut Vec<String>) {
    for inst in insts {
        match inst {
            // W-3: Float literals are now supported (f64 bits stored in int64_t via bitcast)
            IrInst::ConstFloat(_, _) => {}
            // W-4: BuchiPack operations are now supported
            IrInst::PackNew(_, _) => {}
            IrInst::PackGet(_, _, _) => {}
            IrInst::PackSet(_, _, _) => {}
            IrInst::PackSetTag(_, _, _) => {}
            // W-5: FuncAddr, MakeClosure, CallIndirect are now supported
            IrInst::FuncAddr(_, _) => {}
            IrInst::MakeClosure(_, _, _) => {}
            IrInst::CallIndirect(_, _, _) => {}
            IrInst::CondBranch(_, arms) => {
                for arm in arms {
                    collect_unsupported_insts(&arm.body, _out);
                }
            }
            // All other supported instructions (ConstFloat and Pack* are handled above)
            IrInst::ConstInt(_, _)
            | IrInst::ConstBool(_, _)
            | IrInst::ConstStr(_, _)
            | IrInst::DefVar(_, _)
            | IrInst::UseVar(_, _)
            | IrInst::Call(_, _, _)
            | IrInst::CallUser(_, _, _)
            | IrInst::Return(_)
            | IrInst::Retain(_)
            | IrInst::Release(_)
            | IrInst::GlobalSet(_, _)
            | IrInst::GlobalGet(_, _)
            | IrInst::TailCall(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// F-4: Global variable name collection (for name-based C variables)
// ---------------------------------------------------------------------------

/// Collect all global variable hashes used in the module and assign them
/// unique C variable names based on their hash values.
fn collect_global_hashes(ir_module: &IrModule) -> Vec<i64> {
    let mut hashes = HashSet::new();
    for func in &ir_module.functions {
        collect_global_hashes_from_insts(&func.body, &mut hashes);
    }
    let mut sorted: Vec<i64> = hashes.into_iter().collect();
    sorted.sort();
    sorted
}

fn collect_global_hashes_from_insts(insts: &[IrInst], hashes: &mut HashSet<i64>) {
    for inst in insts {
        match inst {
            IrInst::GlobalSet(hash, _) | IrInst::GlobalGet(_, hash) => {
                hashes.insert(*hash);
            }
            IrInst::CondBranch(_, arms) => {
                for arm in arms {
                    collect_global_hashes_from_insts(&arm.body, hashes);
                }
            }
            _ => {}
        }
    }
}

/// Build a map from hash -> C variable name for globals.
fn build_global_name_map(hashes: &[i64]) -> HashMap<i64, String> {
    let mut map = HashMap::new();
    for (i, hash) in hashes.iter().enumerate() {
        // Use hash value in the name to avoid ambiguity, plus index for uniqueness
        let unsigned = *hash as u64;
        map.insert(*hash, format!("_tg_{}_{}", i, unsigned));
    }
    map
}

// ---------------------------------------------------------------------------
// C string literal helper
// ---------------------------------------------------------------------------

/// C コード上でのエスケープ済み文字列リテラルを生成
fn c_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            c => {
                // UTF-8 bytes as hex escapes
                let mut buf = [0u8; 4];
                let encoded = c.encode_utf8(&mut buf);
                for b in encoded.bytes() {
                    write!(out, "\\x{:02x}", b).unwrap();
                }
            }
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Taida IR モジュールを wasm 用 C ソースに変換する
///
/// F-1: 事前に capability validation を実行し、未対応 IR は compile error にする。
/// `profile` により許可する runtime 関数セットが変わる。
pub fn emit_c(ir_module: &IrModule, profile: WasmProfile) -> Result<String, WasmCEmitError> {
    // F-1: capability validation (prevents silent miscompile)
    validate_wasm_min_capabilities(ir_module)?;

    let mut c = String::new();

    // ヘッダー
    writeln!(c, "/* Generated by Taida wasm-min C emitter */").unwrap();
    writeln!(c, "#include <stdint.h>").unwrap();
    writeln!(c).unwrap();

    // WF-2f: wasm-full overrides for polymorphic functions whose core implementations
    // do not handle Lax/Result (runtime_core_wasm.c is FROZEN).
    // The generated C code calls the macro-redirected name; the override lives in
    // runtime_full_wasm.c alongside the other wasm-full extensions.
    if profile == WasmProfile::Full {
        writeln!(
            c,
            "#define taida_polymorphic_is_empty taida_polymorphic_is_empty_full"
        )
        .unwrap();
        writeln!(c, "#define taida_collection_get taida_collection_get_full").unwrap();
        // WF-3a: redirect field registration to full's shadow registry (for JSON lookup)
        writeln!(
            c,
            "#define taida_register_field_name taida_register_field_name_full"
        )
        .unwrap();
        writeln!(
            c,
            "#define taida_register_field_type taida_register_field_type_full"
        )
        .unwrap();
        // WF-3 fix: redirect polymorphic_to_string to full's version that properly handles
        // Gorillax/RelaxedGorillax type detection (core's version has > 4096 address threshold
        // that fails for data section strings at low addresses in wasm-full)
        writeln!(
            c,
            "#define taida_polymorphic_to_string taida_polymorphic_to_string_full"
        )
        .unwrap();
        // WF-3 fix: core's int_mold_str returns raw value, full needs Lax wrapper
        writeln!(c, "#define taida_int_mold_str taida_int_mold_str_full").unwrap();
    }

    // W-3: f64 -> i64 bitcast helper (union-based, no libc dependency)
    writeln!(c, "static int64_t _d2l(double v) {{ union {{ int64_t l; double d; }} u; u.d = v; return u.l; }}").unwrap();
    writeln!(c).unwrap();

    // F-4: グローバル変数を名前ベースの C 変数として宣言
    let global_hashes = collect_global_hashes(ir_module);
    let global_map = build_global_name_map(&global_hashes);
    for hash in &global_hashes {
        let var_name = &global_map[hash];
        writeln!(c, "static int64_t {};", var_name).unwrap();
    }
    if !global_hashes.is_empty() {
        writeln!(c).unwrap();
    }

    // runtime 関数のプロトタイプ宣言（必要なもののみ）
    let mut needed_funcs = HashSet::new();
    for func in &ir_module.functions {
        collect_needed_runtime_funcs(&func.body, &mut needed_funcs);
    }

    // NET-6 fix: check for net HTTP API usage before individual prototype checks.
    // Arguments like Bytes[...] may hit a generic unsupported error first,
    // masking the net-specific diagnostic. Early-out ensures the intended message.
    validate_net_http_api_for_wasm(&needed_funcs, profile)?;

    // C12B-023: reject Regex construction / matching on wasm profiles.
    // The wasm runtime only ships silent-stub implementations for these
    // helpers (returning 0 / -1 / UB) which violates PHILOSOPHY I
    // "silent undefined 禁止". Fail at compile time instead.
    validate_regex_api_for_wasm(&needed_funcs, profile)?;

    for name in &needed_funcs {
        writeln!(c, "{}", runtime_func_prototype(name, profile)?).unwrap();
    }
    if !needed_funcs.is_empty() {
        writeln!(c).unwrap();
    }

    // ユーザー関数（forward declarations）
    for func in &ir_module.functions {
        if func.name != "_taida_main" {
            write!(c, "int64_t {}(", func.name).unwrap();
            for (i, _param) in func.params.iter().enumerate() {
                if i > 0 {
                    write!(c, ", ").unwrap();
                }
                write!(c, "int64_t").unwrap();
            }
            if func.params.is_empty() {
                write!(c, "void").unwrap();
            }
            writeln!(c, ");").unwrap();
        }
    }

    // W-5g: Build function name -> user arity map for closure creation.
    // User arity = total params - 1 (for __env), or total params if no __env.
    let mut func_user_arity: HashMap<String, usize> = HashMap::new();
    for func in &ir_module.functions {
        let arity = if func.params.first().map(|s| s.as_str()) == Some("__env") {
            func.params.len().saturating_sub(1)
        } else {
            func.params.len()
        };
        func_user_arity.insert(func.name.clone(), arity);
    }

    // 関数定義
    for func in &ir_module.functions {
        writeln!(c).unwrap();
        emit_function(&mut c, func, &global_map, &func_user_arity)?;
    }

    Ok(c)
}

fn collect_needed_runtime_funcs(insts: &[IrInst], set: &mut HashSet<String>) {
    for inst in insts {
        match inst {
            IrInst::Call(_, name, _) => {
                set.insert(name.clone());
                // RCB-101 fix: taida_error_type_check_or_rethrow needs
                // taida_is_error_thrown for the post-call early-return check.
                if name == "taida_error_type_check_or_rethrow" {
                    set.insert("taida_is_error_thrown".to_string());
                }
            }
            // W-4: Pack IR instructions need runtime function prototypes
            IrInst::PackNew(_, _) => {
                set.insert("taida_pack_new".to_string());
            }
            IrInst::PackSet(_, _, _) => {
                set.insert("taida_pack_set".to_string());
            }
            IrInst::PackSetTag(_, _, _) => {
                set.insert("taida_pack_set_tag".to_string());
            }
            IrInst::PackGet(_, _, _) => {
                set.insert("taida_pack_get_idx".to_string());
            }
            // W-5: Closure IR instructions need runtime function prototypes
            IrInst::MakeClosure(_, _, _) => {
                set.insert("taida_pack_new".to_string());
                set.insert("taida_pack_set".to_string());
                set.insert("taida_pack_set_hash".to_string());
                set.insert("taida_closure_new".to_string());
            }
            IrInst::CallIndirect(_, _, _) => {
                set.insert("taida_is_closure_value".to_string());
                set.insert("taida_closure_get_fn".to_string());
                set.insert("taida_closure_get_env".to_string());
            }
            // W-5: FuncAddr does not need extra runtime functions
            IrInst::FuncAddr(_, _) => {}
            IrInst::CondBranch(_, arms) => {
                for arm in arms {
                    collect_needed_runtime_funcs(&arm.body, set);
                }
            }
            _ => {}
        }
    }
}

/// runtime 関数の C プロトタイプを生成
///
/// wasm-min runtime では全値を int64_t (boxed value) として統一する。
/// 文字列ポインタも int64_t にキャストして渡す。runtime 側で適切にキャストする。
fn runtime_func_prototype(name: &str, profile: WasmProfile) -> Result<String, WasmCEmitError> {
    let proto = match name {
        // I/O
        "taida_io_stdout" | "taida_io_stderr" => {
            format!("int64_t {}(int64_t val);", name)
        }
        // B11-2: Type-tagged I/O for Bool display parity
        "taida_io_stdout_with_tag" | "taida_io_stderr_with_tag" => {
            format!("int64_t {}(int64_t val, int64_t tag);", name)
        }
        // B11-2: Pack field tag lookup for runtime Bool display
        "taida_pack_get_field_tag" => {
            "int64_t taida_pack_get_field_tag(int64_t pack, int64_t hash);".to_string()
        }
        // Debug 出力 (W-3: taida_debug_float 追加, W-6: taida_debug_polymorphic 追加)
        "taida_debug_int"
        | "taida_debug_str"
        | "taida_debug_bool"
        | "taida_debug_float"
        | "taida_debug_polymorphic" => {
            format!("int64_t {}(int64_t val);", name)
        }
        // 整数演算 (2引数)
        "taida_int_add" | "taida_int_sub" | "taida_int_mul" | "taida_int_eq" | "taida_int_neq"
        | "taida_int_lt" | "taida_int_gt" | "taida_int_gte" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }
        // 整数演算 (1引数)
        "taida_int_neg" => "int64_t taida_int_neg(int64_t a);".to_string(),
        // W-3: Float 演算 (boxed float as int64_t via bitcast)
        "taida_float_add" | "taida_float_sub" | "taida_float_mul" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }
        "taida_float_neg" => "int64_t taida_float_neg(int64_t a);".to_string(),
        // W-3: Int→Float 変換 (returns boxed float as int64_t)
        "taida_int_to_float" => "int64_t taida_int_to_float(int64_t a);".to_string(),
        // W-3: Float→Int 変換
        "taida_float_to_int" => "int64_t taida_float_to_int(int64_t a);".to_string(),
        // W-3: String operations
        "taida_str_concat" => "int64_t taida_str_concat(int64_t a, int64_t b);".to_string(),
        "taida_str_length" => "int64_t taida_str_length(int64_t s);".to_string(),
        "taida_str_eq" => "int64_t taida_str_eq(int64_t a, int64_t b);".to_string(),
        "taida_str_neq" => "int64_t taida_str_neq(int64_t a, int64_t b);".to_string(),
        // W-3: Type conversions
        "taida_int_to_str" => "int64_t taida_int_to_str(int64_t a);".to_string(),
        "taida_str_to_int" => "int64_t taida_str_to_int(int64_t s);".to_string(),
        "taida_str_from_bool" => "int64_t taida_str_from_bool(int64_t v);".to_string(),
        "taida_float_to_str" => "int64_t taida_float_to_str(int64_t a);".to_string(),
        "taida_int_abs" => "int64_t taida_int_abs(int64_t a);".to_string(),
        "taida_int_lte" => "int64_t taida_int_lte(int64_t a, int64_t b);".to_string(),
        // W-3f: Polymorphic methods (wasm-min simplified versions)
        "taida_polymorphic_length" => "int64_t taida_polymorphic_length(int64_t ptr);".to_string(),
        "taida_polymorphic_to_string" => {
            "int64_t taida_polymorphic_to_string(int64_t obj);".to_string()
        }
        // W-3f: Int mold from string
        "taida_int_mold_str" => "int64_t taida_int_mold_str(int64_t v);".to_string(),
        // ブール演算
        "taida_bool_and" | "taida_bool_or" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }
        "taida_bool_not" => "int64_t taida_bool_not(int64_t a);".to_string(),
        // Div/Mod mold + unmold
        "taida_div_mold" | "taida_mod_mold" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }
        "taida_generic_unmold" => "int64_t taida_generic_unmold(int64_t val);".to_string(),
        // Polymorphic comparison
        "taida_poly_eq" | "taida_poly_neq" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }
        // W-4: Field registry (no-op in wasm-min, used for display in native)
        "taida_register_field_name" => {
            "int64_t taida_register_field_name(int64_t hash, int64_t name_ptr);".to_string()
        }
        "taida_register_field_type" => {
            "int64_t taida_register_field_type(int64_t hash, int64_t name_ptr, int64_t type_tag);"
                .to_string()
        }
        // W-4: BuchiPack runtime functions
        "taida_pack_new" => "int64_t taida_pack_new(int64_t field_count);".to_string(),
        "taida_pack_set" => {
            "int64_t taida_pack_set(int64_t pack_ptr, int64_t index, int64_t value);".to_string()
        }
        "taida_pack_set_tag" => {
            "int64_t taida_pack_set_tag(int64_t pack_ptr, int64_t index, int64_t tag);".to_string()
        }
        // NB-14: Stack-based call-site arg tag propagation
        "taida_push_call_tags" => {
            "void taida_push_call_tags(void);".to_string()
        }
        "taida_pop_call_tags" => {
            "void taida_pop_call_tags(void);".to_string()
        }
        "taida_set_call_arg_tag" => {
            "int64_t taida_set_call_arg_tag(int64_t index, int64_t tag);".to_string()
        }
        "taida_get_call_arg_tag" => {
            "int64_t taida_get_call_arg_tag(int64_t index);".to_string()
        }
        // C12B-022: Runtime primitive-type check for TypeIs on param-tag idents
        "taida_primitive_tag_match" => {
            "int64_t taida_primitive_tag_match(int64_t tag, int64_t expected);".to_string()
        }
        "taida_set_return_tag" => {
            "int64_t taida_set_return_tag(int64_t tag);".to_string()
        }
        "taida_get_return_tag" => {
            "int64_t taida_get_return_tag(void);".to_string()
        }
        "taida_pack_get_idx" => {
            "int64_t taida_pack_get_idx(int64_t pack_ptr, int64_t index);".to_string()
        }
        "taida_pack_set_hash" => {
            "int64_t taida_pack_set_hash(int64_t pack_ptr, int64_t index, int64_t hash);"
                .to_string()
        }
        "taida_pack_get" => {
            "int64_t taida_pack_get(int64_t pack_ptr, int64_t field_hash);".to_string()
        }
        "taida_pack_has_hash" => {
            "int64_t taida_pack_has_hash(int64_t pack_ptr, int64_t field_hash);".to_string()
        }
        // W-4: List runtime functions
        "taida_list_new" => "int64_t taida_list_new(void);".to_string(),
        "taida_list_push" => "int64_t taida_list_push(int64_t list_ptr, int64_t item);".to_string(),
        "taida_list_length" => "int64_t taida_list_length(int64_t list_ptr);".to_string(),
        "taida_list_get" => "int64_t taida_list_get(int64_t list_ptr, int64_t index);".to_string(),
        "taida_list_is_empty" => "int64_t taida_list_is_empty(int64_t list_ptr);".to_string(),
        "taida_list_set_elem_tag" => {
            "void taida_list_set_elem_tag(int64_t list_ptr, int64_t tag);".to_string()
        }
        // W-4: HashMap runtime functions
        "taida_hashmap_new" => "int64_t taida_hashmap_new(void);".to_string(),
        "taida_hashmap_set" => {
            "int64_t taida_hashmap_set(int64_t hm, int64_t kh, int64_t kp, int64_t v);".to_string()
        }
        "taida_hashmap_set_immut" => {
            "int64_t taida_hashmap_set_immut(int64_t hm, int64_t kh, int64_t kp, int64_t v);"
                .to_string()
        }
        "taida_hashmap_get" => {
            "int64_t taida_hashmap_get(int64_t hm, int64_t kh, int64_t kp);".to_string()
        }
        "taida_hashmap_has" => {
            "int64_t taida_hashmap_has(int64_t hm, int64_t kh, int64_t kp);".to_string()
        }
        "taida_hashmap_is_empty" => "int64_t taida_hashmap_is_empty(int64_t hm);".to_string(),
        "taida_hashmap_get_lax" => {
            "int64_t taida_hashmap_get_lax(int64_t hm, int64_t kh, int64_t kp);".to_string()
        }
        "taida_hashmap_set_value_tag" => {
            "void taida_hashmap_set_value_tag(int64_t hm, int64_t tag);".to_string()
        }
        "taida_str_hash" => "int64_t taida_str_hash(int64_t str_ptr);".to_string(),
        // W-4: Set runtime functions
        "taida_set_from_list" => "int64_t taida_set_from_list(int64_t list_ptr);".to_string(),
        "taida_set_add" => "int64_t taida_set_add(int64_t set_ptr, int64_t item);".to_string(),
        "taida_set_has" => "int64_t taida_set_has(int64_t set_ptr, int64_t item);".to_string(),
        "taida_set_set_elem_tag" => {
            "void taida_set_set_elem_tag(int64_t set_ptr, int64_t tag);".to_string()
        }
        // W-4f: Set operations (union/intersect/diff/toList/remove)
        "taida_set_union" => "int64_t taida_set_union(int64_t set_a, int64_t set_b);".to_string(),
        "taida_set_intersect" => {
            "int64_t taida_set_intersect(int64_t set_a, int64_t set_b);".to_string()
        }
        "taida_set_diff" => "int64_t taida_set_diff(int64_t set_a, int64_t set_b);".to_string(),
        "taida_set_to_list" => "int64_t taida_set_to_list(int64_t set_ptr);".to_string(),
        "taida_set_remove" => {
            "int64_t taida_set_remove(int64_t set_ptr, int64_t item);".to_string()
        }
        // W-4f: Polymorphic collection methods
        "taida_collection_get" => {
            "int64_t taida_collection_get(int64_t ptr, int64_t item);".to_string()
        }
        "taida_collection_has" => {
            "int64_t taida_collection_has(int64_t ptr, int64_t item);".to_string()
        }
        "taida_collection_remove" => {
            "int64_t taida_collection_remove(int64_t ptr, int64_t item);".to_string()
        }
        "taida_collection_size" => "int64_t taida_collection_size(int64_t ptr);".to_string(),
        // W-4f: Value hash (polymorphic key hashing for HashMap/Set)
        "taida_value_hash" => "int64_t taida_value_hash(int64_t val);".to_string(),
        // W-4f: HashMap additional methods (keys/values/entries/merge)
        "taida_hashmap_keys" => "int64_t taida_hashmap_keys(int64_t hm);".to_string(),
        "taida_hashmap_values" => "int64_t taida_hashmap_values(int64_t hm);".to_string(),
        "taida_hashmap_entries" => "int64_t taida_hashmap_entries(int64_t hm);".to_string(),
        "taida_hashmap_merge" => {
            "int64_t taida_hashmap_merge(int64_t hm, int64_t other);".to_string()
        }
        // W-4f: Polymorphic isEmpty
        "taida_polymorphic_is_empty" => {
            "int64_t taida_polymorphic_is_empty(int64_t ptr);".to_string()
        }
        // W-5: Closure runtime functions
        "taida_closure_new" => {
            "int64_t taida_closure_new(int64_t fn_ptr, int64_t env_ptr, int64_t user_arity);"
                .to_string()
        }
        "taida_closure_get_fn" => "int64_t taida_closure_get_fn(int64_t closure_ptr);".to_string(),
        "taida_closure_get_env" => {
            "int64_t taida_closure_get_env(int64_t closure_ptr);".to_string()
        }
        "taida_is_closure_value" => "int64_t taida_is_closure_value(int64_t val);".to_string(),
        // W-5: Error ceiling runtime functions
        "taida_error_ceiling_push" => "int64_t taida_error_ceiling_push(void);".to_string(),
        "taida_error_ceiling_pop" => "void taida_error_ceiling_pop(void);".to_string(),
        "taida_throw" => "int64_t taida_throw(int64_t error_val);".to_string(),
        "taida_error_try_call" => {
            "int64_t taida_error_try_call(int64_t fn_ptr, int64_t env_ptr, int64_t depth);"
                .to_string()
        }
        "taida_error_try_get_result" => {
            "int64_t taida_error_try_get_result(int64_t depth);".to_string()
        }
        "taida_error_get_value" => "int64_t taida_error_get_value(int64_t depth);".to_string(),
        "taida_error_setjmp" => "int64_t taida_error_setjmp(int64_t depth);".to_string(),
        // RCB-101: Error type filtering for |==
        "taida_register_type_parent" => "void taida_register_type_parent(int64_t child_str, int64_t parent_str);".to_string(),
        "taida_error_type_matches" => "int64_t taida_error_type_matches(int64_t error_val, int64_t handler_type_str);".to_string(),
        "taida_error_type_check_or_rethrow" => "int64_t taida_error_type_check_or_rethrow(int64_t error_val, int64_t handler_type_str);".to_string(),
        // B11B-015: TypeIs named type runtime check
        "taida_typeis_named" => "int64_t taida_typeis_named(int64_t val, int64_t expected_type_str);".to_string(),
        "taida_is_error_thrown" => "int64_t taida_is_error_thrown(void);".to_string(),
        "taida_make_error" => {
            "int64_t taida_make_error(int64_t type_ptr, int64_t msg_ptr);".to_string()
        }
        // W-5: Lax runtime functions
        "taida_lax_new" => {
            "int64_t taida_lax_new(int64_t value, int64_t default_value);".to_string()
        }
        "taida_lax_empty" => "int64_t taida_lax_empty(int64_t default_value);".to_string(),
        "taida_lax_has_value" => "int64_t taida_lax_has_value(int64_t lax_ptr);".to_string(),
        "taida_lax_get_or_default" => {
            "int64_t taida_lax_get_or_default(int64_t lax_ptr, int64_t fallback);".to_string()
        }
        "taida_lax_unmold" => "int64_t taida_lax_unmold(int64_t lax_ptr);".to_string(),
        "taida_lax_is_empty" => "int64_t taida_lax_is_empty(int64_t lax_ptr);".to_string(),
        // W-5: Gorillax/Result runtime functions
        "taida_gorillax_new" => "int64_t taida_gorillax_new(int64_t value);".to_string(),
        "taida_gorillax_err" => "int64_t taida_gorillax_err(int64_t error);".to_string(),
        "taida_gorillax_is_ok" => "int64_t taida_gorillax_is_ok(int64_t gx);".to_string(),
        "taida_gorillax_get_value" => "int64_t taida_gorillax_get_value(int64_t gx);".to_string(),
        "taida_gorillax_get_error" => "int64_t taida_gorillax_get_error(int64_t gx);".to_string(),
        "taida_gorillax_relax" => "int64_t taida_gorillax_relax(int64_t gx);".to_string(),
        "taida_relaxed_gorillax_new" => {
            "int64_t taida_relaxed_gorillax_new(int64_t value);".to_string()
        }
        "taida_relaxed_gorillax_err" => {
            "int64_t taida_relaxed_gorillax_err(int64_t error);".to_string()
        }
        "taida_result_create" => {
            "int64_t taida_result_create(int64_t value, int64_t throw_val, int64_t predicate);"
                .to_string()
        }
        "taida_result_is_ok" => "int64_t taida_result_is_ok(int64_t result);".to_string(),
        "taida_result_is_error" => "int64_t taida_result_is_error(int64_t result);".to_string(),
        "taida_result_map_error" => {
            "int64_t taida_result_map_error(int64_t result, int64_t fn_ptr);".to_string()
        }
        "taida_cage_apply" => {
            "int64_t taida_cage_apply(int64_t cage_value, int64_t fn_ptr);".to_string()
        }
        // W-5: Error/Molten/Stub helpers
        "taida_molten_new" => "int64_t taida_molten_new(void);".to_string(),
        // C25B-001: minimal Stream wrapper (runtime lives in
        // runtime_core_wasm/02_containers.inc.c).
        "taida_stream_new" => "int64_t taida_stream_new(int64_t inner_value);".to_string(),
        "taida_stub_new" => "int64_t taida_stub_new(int64_t message);".to_string(),
        "taida_todo_new" => {
            "int64_t taida_todo_new(int64_t id, int64_t task, int64_t sol, int64_t unm);"
                .to_string()
        }
        // BE-WASM-2: Gorilla literal (exit with status 1)
        "taida_gorilla" => "void taida_gorilla(void);".to_string(),
        // W-5: Type molds that return Lax
        "taida_str_mold_int" => "int64_t taida_str_mold_int(int64_t v);".to_string(),
        "taida_str_mold_float" => "int64_t taida_str_mold_float(int64_t v);".to_string(),
        "taida_str_mold_bool" => "int64_t taida_str_mold_bool(int64_t v);".to_string(),
        "taida_str_mold_str" => "int64_t taida_str_mold_str(int64_t v);".to_string(),
        // C23-2: generic Str[x]() entry — defined alongside the primitive
        // mold helpers in `02_containers.inc.c`.
        "taida_str_mold_any" => "int64_t taida_str_mold_any(int64_t v);".to_string(),
        "taida_int_mold_int" => "int64_t taida_int_mold_int(int64_t v);".to_string(),
        "taida_int_mold_float" => "int64_t taida_int_mold_float(int64_t v);".to_string(),
        "taida_int_mold_bool" => "int64_t taida_int_mold_bool(int64_t v);".to_string(),
        "taida_float_mold_int" => "int64_t taida_float_mold_int(int64_t v);".to_string(),
        "taida_float_mold_float" => "int64_t taida_float_mold_float(int64_t v);".to_string(),
        "taida_float_mold_str" => "int64_t taida_float_mold_str(int64_t v);".to_string(),
        "taida_float_mold_bool" => "int64_t taida_float_mold_bool(int64_t v);".to_string(),
        "taida_bool_mold_int" => "int64_t taida_bool_mold_int(int64_t v);".to_string(),
        "taida_bool_mold_float" => "int64_t taida_bool_mold_float(int64_t v);".to_string(),
        "taida_bool_mold_str" => "int64_t taida_bool_mold_str(int64_t v);".to_string(),
        "taida_bool_mold_bool" => "int64_t taida_bool_mold_bool(int64_t v);".to_string(),
        // FL-16: Polymorphic add (fallback to string concat or int add)
        "taida_poly_add" => "int64_t taida_poly_add(int64_t a, int64_t b);".to_string(),
        // W-5: Float div/mod molds
        "taida_float_div_mold" => "int64_t taida_float_div_mold(int64_t a, int64_t b);".to_string(),
        "taida_float_mod_mold" => "int64_t taida_float_mod_mold(int64_t a, int64_t b);".to_string(),
        // W-5: String template helpers (str_from_int/float/bool are aliases)
        "taida_str_from_int" => "int64_t taida_str_from_int(int64_t v);".to_string(),
        "taida_str_from_float" => "int64_t taida_str_from_float(int64_t v);".to_string(),
        // W-5: Lax method helpers
        "taida_can_throw_payload" => "int64_t taida_can_throw_payload(int64_t val);".to_string(),
        // W-5: Float comparison
        "taida_float_eq" | "taida_float_neq" | "taida_float_lt" | "taida_float_gt"
        | "taida_float_lte" | "taida_float_gte" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }
        // WC-1: String molds (all profiles — implemented in runtime_core_wasm.c)
        "taida_str_to_upper" | "taida_str_to_lower" | "taida_str_trim"
        | "taida_str_trim_start" | "taida_str_trim_end" | "taida_str_reverse"
        | "taida_str_alloc" | "taida_str_new_copy" => {
            format!("int64_t {}(int64_t s);", name)
        }
        "taida_str_split" => "int64_t taida_str_split(int64_t s, int64_t sep);".to_string(),
        "taida_str_replace" | "taida_str_replace_first" => {
            format!("int64_t {}(int64_t s, int64_t from, int64_t to);", name)
        }
        // C12-6c: Regex polymorphic dispatchers (wasm stubs forward
        // to fixed-string versions — see runtime_core_wasm.c).
        "taida_str_split_poly" => {
            "int64_t taida_str_split_poly(int64_t s, int64_t sep);".to_string()
        }
        "taida_str_replace_poly" | "taida_str_replace_first_poly" => {
            format!("int64_t {}(int64_t s, int64_t target, int64_t rep);", name)
        }
        "taida_str_match_regex" => {
            "int64_t taida_str_match_regex(int64_t s, int64_t regex);".to_string()
        }
        "taida_str_search_regex" => {
            "int64_t taida_str_search_regex(int64_t s, int64_t regex);".to_string()
        }
        "taida_regex_new" => {
            "int64_t taida_regex_new(int64_t pattern, int64_t flags);".to_string()
        }
        "taida_str_pad" => "int64_t taida_str_pad(int64_t s, int64_t target_len, int64_t pad_char, int64_t pad_end);".to_string(),
        "taida_str_slice" => "int64_t taida_str_slice(int64_t s, int64_t start, int64_t end);".to_string(),
        "taida_str_char_at" | "taida_str_get" => {
            format!("int64_t {}(int64_t s, int64_t idx);", name)
        }
        "taida_str_repeat" => "int64_t taida_str_repeat(int64_t s, int64_t n);".to_string(),
        "taida_str_index_of" | "taida_str_last_index_of" => {
            format!("int64_t {}(int64_t s, int64_t sub);", name)
        }
        "taida_str_contains" | "taida_str_starts_with" | "taida_str_ends_with" => {
            format!("int64_t {}(int64_t s, int64_t sub);", name)
        }
        "taida_cmp_strings" => "int64_t taida_cmp_strings(int64_t a, int64_t b);".to_string(),
        "taida_str_release" => "void taida_str_release(int64_t s);".to_string(),
        "taida_slice_mold" => "int64_t taida_slice_mold(int64_t target, int64_t start, int64_t end);".to_string(),
        // WC-1: Char/Codepoint molds (all profiles — implemented in runtime_core_wasm.c)
        "taida_char_mold_int" | "taida_char_mold_str" => {
            format!("int64_t {}(int64_t v);", name)
        }
        "taida_char_to_digit" => "int64_t taida_char_to_digit(int64_t v);".to_string(),
        "taida_codepoint_mold_str" => "int64_t taida_codepoint_mold_str(int64_t v);".to_string(),
        "taida_digit_to_char" => "int64_t taida_digit_to_char(int64_t v);".to_string(),
        // WC-2: Number molds (all profiles — implemented in runtime_core_wasm.c)
        "taida_float_abs" | "taida_float_ceil" | "taida_float_floor"
        | "taida_float_round" => {
            format!("int64_t {}(int64_t a);", name)
        }
        // C25B-025 Phase 5-I: math mold family. Manual freestanding
        // implementations in runtime_core_wasm/03_typeof_list.inc.c
        // (Sqrt uses the `f64.sqrt` wasm opcode via `__builtin_sqrt`;
        // the rest use range-reduced series / Newton iteration — see
        // the C source for per-function algorithm notes).
        "taida_float_sqrt"
        | "taida_float_exp"
        | "taida_float_ln"
        | "taida_float_log2"
        | "taida_float_log10"
        | "taida_float_sin"
        | "taida_float_cos"
        | "taida_float_tan"
        | "taida_float_asin"
        | "taida_float_acos"
        | "taida_float_atan"
        | "taida_float_sinh"
        | "taida_float_cosh"
        | "taida_float_tanh" => {
            format!("int64_t {}(int64_t a);", name)
        }
        "taida_float_pow" | "taida_float_log" | "taida_float_atan2" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }
        "taida_float_to_fixed" => "int64_t taida_float_to_fixed(int64_t a, int64_t b);".to_string(),
        "taida_float_clamp" => "int64_t taida_float_clamp(int64_t a, int64_t lo, int64_t hi);".to_string(),
        "taida_float_is_nan" | "taida_float_is_infinite" | "taida_float_is_finite_check"
        | "taida_float_is_positive" | "taida_float_is_negative" | "taida_float_is_zero" => {
            format!("int64_t {}(int64_t a);", name)
        }
        "taida_int_clamp" => "int64_t taida_int_clamp(int64_t v, int64_t lo, int64_t hi);".to_string(),
        "taida_int_is_positive" | "taida_int_is_negative" | "taida_int_is_zero" => {
            format!("int64_t {}(int64_t a);", name)
        }
        "taida_int_mold_auto" => "int64_t taida_int_mold_auto(int64_t v);".to_string(),
        "taida_int_mold_str_base" => "int64_t taida_int_mold_str_base(int64_t v, int64_t base);".to_string(),
        "taida_to_radix" => "int64_t taida_to_radix(int64_t v, int64_t radix);".to_string(),
        // WC-3: Callback invoke (all profiles — implemented in runtime_core_wasm.c)
        "taida_invoke_callback1" => "int64_t taida_invoke_callback1(int64_t fn_ptr, int64_t a);".to_string(),
        "taida_invoke_callback2" => "int64_t taida_invoke_callback2(int64_t fn_ptr, int64_t a, int64_t b);".to_string(),
        // WC-3: List HOF functions (all profiles — implemented in runtime_core_wasm.c)
        "taida_list_map" | "taida_list_filter" => {
            format!("int64_t {}(int64_t list, int64_t fn_ptr);", name)
        }
        "taida_list_fold" | "taida_list_foldr" => {
            format!("int64_t {}(int64_t list, int64_t init, int64_t fn_ptr);", name)
        }
        "taida_list_find" | "taida_list_find_index" => {
            format!("int64_t {}(int64_t list, int64_t fn_ptr);", name)
        }
        "taida_list_take_while" | "taida_list_drop_while"
        | "taida_list_any" | "taida_list_all" | "taida_list_none" => {
            format!("int64_t {}(int64_t list, int64_t fn_ptr);", name)
        }
        // WC-3: List operation functions (all profiles — implemented in runtime_core_wasm.c)
        "taida_list_sort" | "taida_list_sort_desc" | "taida_list_unique"
        | "taida_list_flatten" | "taida_list_reverse"
        | "taida_list_to_display_string" => {
            format!("int64_t {}(int64_t list);", name)
        }
        "taida_list_sort_by" => {
            "int64_t taida_list_sort_by(int64_t list, int64_t fn_ptr);".to_string()
        }
        "taida_list_join" => "int64_t taida_list_join(int64_t list, int64_t sep);".to_string(),
        "taida_list_concat" | "taida_list_zip" => {
            format!("int64_t {}(int64_t list_a, int64_t list_b);", name)
        }
        "taida_list_append" | "taida_list_prepend" => {
            format!("int64_t {}(int64_t list, int64_t item);", name)
        }
        "taida_list_take" | "taida_list_drop" => {
            format!("int64_t {}(int64_t list, int64_t n);", name)
        }
        "taida_list_enumerate" => "int64_t taida_list_enumerate(int64_t list);".to_string(),
        // WC-3: List query functions (all profiles — implemented in runtime_core_wasm.c)
        "taida_list_first" | "taida_list_last" | "taida_list_min" | "taida_list_max"
        | "taida_list_sum" => {
            format!("int64_t {}(int64_t list);", name)
        }
        "taida_list_contains" | "taida_list_index_of" | "taida_list_last_index_of" => {
            format!("int64_t {}(int64_t list, int64_t item);", name)
        }
        "taida_list_count" => {
            "int64_t taida_list_count(int64_t list, int64_t fn_ptr);".to_string()
        }
        // WC-3: List elem retain/release (all profiles — no-ops in runtime_core_wasm.c)
        "taida_list_elem_retain" | "taida_list_elem_release" => {
            format!("void {}(int64_t list);", name)
        }
        // typeof (all profiles — implemented in runtime_core_wasm.c)
        "taida_typeof" => "int64_t taida_typeof(int64_t val, int64_t tag);".to_string(),
        // WC-4: JSON functions (all profiles — implemented in runtime_core_wasm.c)
        "taida_json_parse" | "taida_json_stringify" | "taida_json_encode"
        | "taida_json_pretty" | "taida_json_unmold" => {
            format!("int64_t {}(int64_t val);", name)
        }
        "taida_json_schema_cast" => "int64_t taida_json_schema_cast(int64_t raw, int64_t schema);".to_string(),
        "taida_json_empty" => "int64_t taida_json_empty(void);".to_string(),
        "taida_json_has" => "int64_t taida_json_has(int64_t json, int64_t key);".to_string(),
        "taida_json_size" => "int64_t taida_json_size(int64_t json);".to_string(),
        "taida_json_from_int" | "taida_json_from_str" => {
            format!("int64_t {}(int64_t val);", name)
        }
        "taida_json_to_int" | "taida_json_to_str" => {
            format!("int64_t {}(int64_t val);", name)
        }
        "taida_debug_json" | "taida_debug_list" => {
            format!("int64_t {}(int64_t val);", name)
        }
        // WC-4: Field lookup (all profiles — implemented in runtime_core_wasm.c)
        "taida_lookup_field_name" => "int64_t taida_lookup_field_name(int64_t hash);".to_string(),
        "taida_lookup_field_type" => "int64_t taida_lookup_field_type(int64_t hash, int64_t name_ptr);".to_string(),
        // WC-5: Lax extended ops (all profiles — implemented in runtime_core_wasm.c)
        "taida_lax_map" | "taida_lax_flat_map" => {
            format!("int64_t {}(int64_t lax, int64_t fn_ptr);", name)
        }
        "taida_lax_to_string" => "int64_t taida_lax_to_string(int64_t lax);".to_string(),
        // WC-5: Result extended ops (all profiles — implemented in runtime_core_wasm.c)
        "taida_result_map" | "taida_result_flat_map" => {
            format!("int64_t {}(int64_t result, int64_t fn_ptr);", name)
        }
        "taida_result_get_or_default" => "int64_t taida_result_get_or_default(int64_t result, int64_t fallback);".to_string(),
        "taida_result_get_or_throw" | "taida_result_to_string"
        | "taida_result_is_error_check" => {
            format!("int64_t {}(int64_t result);", name)
        }
        // WC-5: Gorillax extended ops (all profiles — implemented in runtime_core_wasm.c)
        "taida_gorillax_to_string" | "taida_gorillax_unmold" => {
            format!("int64_t {}(int64_t gx);", name)
        }
        "taida_relaxed_gorillax_to_string" | "taida_relaxed_gorillax_unmold" => {
            format!("int64_t {}(int64_t gx);", name)
        }
        // WC-5: Monadic ops (all profiles — implemented in runtime_core_wasm.c)
        "taida_monadic_field_count" | "taida_monadic_get_or_throw"
        | "taida_monadic_to_string" => {
            format!("int64_t {}(int64_t val);", name)
        }
        "taida_monadic_flat_map" => "int64_t taida_monadic_flat_map(int64_t val, int64_t fn_ptr);".to_string(),
        // WC-6a: HashMap extensions (all profiles — implemented in runtime_core_wasm.c)
        "taida_hashmap_length" | "taida_hashmap_clone" | "taida_hashmap_to_string" => {
            format!("int64_t {}(int64_t hm);", name)
        }
        "taida_hashmap_remove_immut" => {
            "int64_t taida_hashmap_remove_immut(int64_t hm, int64_t kh, int64_t kp);".to_string()
        }
        "taida_hashmap_new_with_cap" => "int64_t taida_hashmap_new_with_cap(int64_t cap);".to_string(),
        "taida_hashmap_adjust_hash" => "int64_t taida_hashmap_adjust_hash(int64_t h);".to_string(),
        "taida_hashmap_set_internal" => {
            "int64_t taida_hashmap_set_internal(int64_t hm, int64_t kh, int64_t kp, int64_t v, int64_t mode);".to_string()
        }
        "taida_hashmap_resize" => "int64_t taida_hashmap_resize(int64_t hm, int64_t new_cap);".to_string(),
        "taida_hashmap_key_eq" | "taida_hashmap_key_retain" | "taida_hashmap_key_release"
        | "taida_hashmap_val_retain" | "taida_hashmap_val_release" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }
        "taida_hashmap_key_valid" => "int64_t taida_hashmap_key_valid(int64_t v);".to_string(),
        // WC-6b: Set extensions (all profiles — implemented in runtime_core_wasm.c)
        "taida_set_contains" => "int64_t taida_set_contains(int64_t set, int64_t item);".to_string(),
        "taida_set_is_empty" | "taida_set_size" | "taida_set_to_string" => {
            format!("int64_t {}(int64_t set);", name)
        }
        // WC-6c: Pack extensions (all profiles — implemented in runtime_core_wasm.c)
        "taida_pack_call_field0" => "int64_t taida_pack_call_field0(int64_t pack, int64_t hash);".to_string(),
        "taida_pack_call_field1" => "int64_t taida_pack_call_field1(int64_t pack, int64_t hash, int64_t a);".to_string(),
        "taida_pack_call_field2" => "int64_t taida_pack_call_field2(int64_t pack, int64_t hash, int64_t a, int64_t b);".to_string(),
        "taida_pack_call_field3" => "int64_t taida_pack_call_field3(int64_t pack, int64_t hash, int64_t a, int64_t b, int64_t c);".to_string(),
        "taida_pack_to_display_string" => "int64_t taida_pack_to_display_string(int64_t pack);".to_string(),
        "taida_make_io_error" => "int64_t taida_make_io_error(int64_t msg);".to_string(),
        "taida_retain_and_tag_field" => "int64_t taida_retain_and_tag_field(int64_t val, int64_t tag);".to_string(),
        // WC-6d: Type detection / display (all profiles — implemented in runtime_core_wasm.c)
        "taida_is_string_value" | "taida_is_list" | "taida_is_hashmap" | "taida_is_set"
        | "taida_is_buchi_pack" | "taida_is_molten" | "taida_is_bytes" | "taida_is_async" => {
            format!("int64_t {}(int64_t val);", name)
        }
        "taida_detect_value_tag" | "taida_detect_gorillax_type" => {
            format!("int64_t {}(int64_t val);", name)
        }
        "taida_bool_to_int" | "taida_bool_to_str" => {
            format!("int64_t {}(int64_t v);", name)
        }
        "taida_value_to_display_string" | "taida_value_to_debug_string" => {
            format!("int64_t {}(int64_t val);", name)
        }
        "taida_has_magic_header" | "taida_ptr_is_readable" => {
            format!("int64_t {}(int64_t val);", name)
        }
        "taida_read_cstr_len_safe" => "int64_t taida_read_cstr_len_safe(int64_t ptr, int64_t max);".to_string(),
        // WC-6e: Polymorphic extensions (all profiles — implemented in runtime_core_wasm.c)
        "taida_polymorphic_contains" | "taida_polymorphic_get_or_default"
        | "taida_polymorphic_index_of" | "taida_polymorphic_last_index_of" => {
            format!("int64_t {}(int64_t ptr, int64_t item);", name)
        }
        "taida_polymorphic_has_value" => "int64_t taida_polymorphic_has_value(int64_t ptr);".to_string(),
        "taida_polymorphic_map" => "int64_t taida_polymorphic_map(int64_t ptr, int64_t fn_ptr);".to_string(),
        // PR-4: Async runtime functions (synchronous blocking in wasm-min)
        "taida_async_ok_tagged" => {
            "int64_t taida_async_ok_tagged(int64_t value, int64_t value_tag);".to_string()
        }
        "taida_async_ok" => "int64_t taida_async_ok(int64_t value);".to_string(),
        "taida_async_err" => "int64_t taida_async_err(int64_t error);".to_string(),
        "taida_async_set_value_tag" => {
            "void taida_async_set_value_tag(int64_t async_ptr, int64_t tag);".to_string()
        }
        "taida_async_unmold" => "int64_t taida_async_unmold(int64_t async_ptr);".to_string(),
        "taida_async_is_pending" | "taida_async_is_fulfilled"
        | "taida_async_is_rejected" | "taida_async_get_value"
        | "taida_async_get_error" => {
            format!("int64_t {}(int64_t async_ptr);", name)
        }
        "taida_async_map" => {
            "int64_t taida_async_map(int64_t async_ptr, int64_t fn_ptr);".to_string()
        }
        "taida_async_get_or_default" => {
            "int64_t taida_async_get_or_default(int64_t async_ptr, int64_t fallback);".to_string()
        }
        "taida_async_all" | "taida_async_race" | "taida_async_cancel" => {
            format!("int64_t {}(int64_t arg);", name)
        }
        "taida_async_spawn" => {
            "int64_t taida_async_spawn(int64_t fn_ptr, int64_t arg);".to_string()
        }
        // RC no-ops
        "taida_retain" | "taida_release" | "taida_str_retain" => {
            format!("void {}(int64_t val);", name)
        }
        // WW-2: wasm-wasi / wasm-full OS API functions (env, file I/O)
        "taida_os_env_var" if profile == WasmProfile::Wasi || profile == WasmProfile::Full => {
            "int64_t taida_os_env_var(int64_t name_ptr);".to_string()
        }
        "taida_os_all_env" if profile == WasmProfile::Wasi || profile == WasmProfile::Full => {
            "int64_t taida_os_all_env(void);".to_string()
        }
        "taida_os_read" if profile == WasmProfile::Wasi || profile == WasmProfile::Full => {
            "int64_t taida_os_read(int64_t path_ptr);".to_string()
        }
        "taida_os_write_file" if profile == WasmProfile::Wasi || profile == WasmProfile::Full => {
            "int64_t taida_os_write_file(int64_t path_ptr, int64_t content_ptr);".to_string()
        }
        "taida_os_exists" if profile == WasmProfile::Wasi || profile == WasmProfile::Full => {
            "int64_t taida_os_exists(int64_t path_ptr);".to_string()
        }
        // C26B-020 柱 3: readBytesAt(path, offset, len) -> Lax[Bytes]
        // Implemented in runtime_wasi_io.c (wasm-wasi / wasm-full link
        // this object; wasm-full additionally links runtime_full_wasm.c
        // which supplies Bytes constructors but the wasi-side produces
        // layout-compatible Bytes values via static helpers).
        "taida_os_read_bytes_at"
            if profile == WasmProfile::Wasi || profile == WasmProfile::Full =>
        {
            "int64_t taida_os_read_bytes_at(int64_t path_ptr, int64_t offset, int64_t len);"
                .to_string()
        }
        // WE-2: wasm-edge OS API functions (env only, no file I/O)
        "taida_os_env_var" if profile == WasmProfile::Edge => {
            "int64_t taida_os_env_var(int64_t name_ptr);".to_string()
        }
        "taida_os_all_env" if profile == WasmProfile::Edge => {
            "int64_t taida_os_all_env(void);".to_string()
        }
        // wasm-edge does not support file I/O
        "taida_os_read"
        | "taida_os_write_file"
        | "taida_os_exists"
        | "taida_os_read_bytes_at"
            if profile == WasmProfile::Edge =>
        {
            return Err(WasmCEmitError {
                message: format!(
                    "wasm-edge does not support '{}'. \
                     Use wasm-wasi or native backend instead.",
                    name
                ),
            });
        }
        // wasm-min unsupported OS APIs: give a specific error message
        "taida_os_env_var"
        | "taida_os_all_env"
        | "taida_os_read"
        | "taida_os_write_file"
        | "taida_os_exists"
        | "taida_os_read_bytes_at"
            if profile == WasmProfile::Min =>
        {
            return Err(WasmCEmitError {
                message: "wasm-min does not support OS operations. \
                          Use wasm-wasi or native backend."
                    .to_string(),
            });
        }
        // NET-6: taida-lang/net HTTP API is unsupported on all WASM profiles.
        // All 3 HTTP v1 functions (httpServe, httpParseRequestHead, httpEncodeResponse)
        // produce explicit compile errors with profile-specific diagnostics.
        // Source of truth: .dev/NET_WASM_POLICY.md
        "taida_net_http_serve"
        | "taida_net_http_parse_request_head"
        | "taida_net_http_encode_response"
        | "taida_net_read_body"
        | "taida_net_read_body_chunk"
        | "taida_net_read_body_all"
        | "taida_net_ws_upgrade"
        | "taida_net_ws_send"
        | "taida_net_ws_receive"
        | "taida_net_ws_close"
        | "taida_net_ws_close_code" =>
        {
            let profile_name = match profile {
                WasmProfile::Min => "wasm-min",
                WasmProfile::Wasi => "wasm-wasi",
                WasmProfile::Edge => "wasm-edge",
                WasmProfile::Full => "wasm-full",
            };
            let api_name = match name {
                "taida_net_http_serve" => "httpServe",
                "taida_net_http_parse_request_head" => "httpParseRequestHead",
                "taida_net_http_encode_response" => "httpEncodeResponse",
                "taida_net_read_body" => "readBody",
                "taida_net_read_body_chunk" => "readBodyChunk",
                "taida_net_read_body_all" => "readBodyAll",
                "taida_net_ws_upgrade" => "wsUpgrade",
                "taida_net_ws_send" => "wsSend",
                "taida_net_ws_receive" => "wsReceive",
                "taida_net_ws_close" => "wsClose",
                "taida_net_ws_close_code" => "wsCloseCode",
                _ => name,
            };
            return Err(WasmCEmitError {
                message: format!(
                    "{} does not support taida-lang/net HTTP API '{}'. \
                     Use the interpreter, JS, or native backend instead.",
                    profile_name, api_name
                ),
            });
        }
        // WF-2: wasm-full extended runtime functions (Tier 1)
        _ if profile == WasmProfile::Full => {
            // wasm-full accepts all runtime functions -- prototype is generated
            // from the uniform ABI (all args/return as int64_t).
            // The actual implementation lives in runtime_full_wasm.c.
            match wasm_full_runtime_prototype(name) {
                Some(proto) => proto,
                None => {
                    return Err(WasmCEmitError {
                        message: format!(
                            "wasm-full does not support runtime function '{}'. \
                             Use the interpreter or native backend instead.",
                            name
                        ),
                    });
                }
            }
        }
        other => {
            // F-1: unsupported runtime functions are compile errors, not silent stubs
            let profile_name = match profile {
                WasmProfile::Min => "wasm-min",
                WasmProfile::Wasi => "wasm-wasi",
                WasmProfile::Edge => "wasm-edge",
                WasmProfile::Full => "wasm-full",
            };
            return Err(WasmCEmitError {
                message: format!(
                    "{} does not support runtime function '{}'. \
                     Use the interpreter or native backend instead.",
                    profile_name, other
                ),
            });
        }
    };
    Ok(proto)
}

/// WF-2: wasm-full extended runtime function prototypes.
///
/// These functions are implemented in `runtime_full_wasm.c` and provide
/// extended capabilities beyond wasm-min/wasi baseline: string molds,
/// number molds, extended list/hashmap/set ops, JSON, bytes, bitwise, etc.
///
/// All values use int64_t (boxed value) ABI, matching native_runtime.c.
fn wasm_full_runtime_prototype(name: &str) -> Option<String> {
    let proto = match name {
        // --- String molds: moved to runtime_func_prototype() (WC-1e) ---
        // --- Number molds: moved to runtime_func_prototype() (WC-2c) ---
        // --- Char / codepoint: moved to runtime_func_prototype() (WC-1e) ---
        // --- List ops / HOF / query / callback: moved to runtime_func_prototype() (WC-3e) ---

        // --- HashMap extended: moved to runtime_func_prototype() (WC-6g) ---
        // --- Set extended: moved to runtime_func_prototype() (WC-6g) ---
        // --- Type detection / conversion: moved to runtime_func_prototype() (WC-6g) ---
        // --- Polymorphic extended: moved to runtime_func_prototype() (WC-6g) ---
        // --- Pack / Error extended: moved to runtime_func_prototype() (WC-6g) ---
        // --- Monadic: moved to runtime_func_prototype() (WC-5e) ---
        // --- JSON: moved to runtime_func_prototype() (WC-4b) ---
        // --- Gorillax / Relaxed Gorillax: moved to runtime_func_prototype() (WC-5e) ---
        // --- Lax extended: moved to runtime_func_prototype() (WC-5e) ---
        // --- Result extended: moved to runtime_func_prototype() (WC-5e) ---
        // --- Field lookup: moved to runtime_func_prototype() (WC-4b) ---
        // --- Callback invoke: moved to runtime_func_prototype() (WC-3e) ---

        // --- Bitwise / Shift ---
        "taida_bit_and" | "taida_bit_or" | "taida_bit_xor" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }
        "taida_bit_not" => "int64_t taida_bit_not(int64_t a);".to_string(),
        "taida_shift_l" | "taida_shift_r" | "taida_shift_ru" => {
            format!("int64_t {}(int64_t a, int64_t b);", name)
        }

        // --- Bytes ---
        "taida_bytes_mold" => "int64_t taida_bytes_mold(int64_t v, int64_t fill);".to_string(),
        "taida_bytes_clone"
        | "taida_bytes_len"
        | "taida_bytes_to_list"
        | "taida_bytes_to_display_string"
        | "taida_bytes_default_value" => {
            format!("int64_t {}(int64_t v);", name)
        }
        "taida_bytes_from_raw" => {
            "int64_t taida_bytes_from_raw(int64_t ptr, int64_t len);".to_string()
        }
        "taida_bytes_new_filled" => {
            "int64_t taida_bytes_new_filled(int64_t len, int64_t fill);".to_string()
        }
        "taida_bytes_get_lax" => {
            "int64_t taida_bytes_get_lax(int64_t bytes, int64_t idx);".to_string()
        }
        "taida_bytes_set" => {
            "int64_t taida_bytes_set(int64_t bytes, int64_t idx, int64_t val);".to_string()
        }
        // Bytes cursor
        "taida_bytes_cursor_new" => {
            "int64_t taida_bytes_cursor_new(int64_t bytes, int64_t offset);".to_string()
        }
        "taida_bytes_cursor_u8" | "taida_bytes_cursor_remaining" => {
            format!("int64_t {}(int64_t cursor);", name)
        }
        "taida_bytes_cursor_take" | "taida_bytes_cursor_step" => {
            format!("int64_t {}(int64_t cursor, int64_t n);", name)
        }
        "taida_bytes_cursor_unpack" => {
            "int64_t taida_bytes_cursor_unpack(int64_t cursor, int64_t schema);".to_string()
        }
        // Bytes encode/decode molds
        "taida_u16be_mold"
        | "taida_u16le_mold"
        | "taida_u32be_mold"
        | "taida_u32le_mold"
        | "taida_u16be_decode_mold"
        | "taida_u16le_decode_mold"
        | "taida_u32be_decode_mold"
        | "taida_u32le_decode_mold"
        | "taida_uint8_mold"
        | "taida_uint8_mold_float" => {
            format!("int64_t {}(int64_t v);", name)
        }
        // UTF-8 molds
        "taida_utf8_encode_mold"
        | "taida_utf8_decode_mold"
        | "taida_utf8_encode_scalar"
        | "taida_utf8_decode_one"
        | "taida_utf8_single_scalar" => {
            format!("int64_t {}(int64_t v);", name)
        }

        // --- Global get/set ---
        "taida_global_get" => "int64_t taida_global_get(int64_t key);".to_string(),
        "taida_global_set" => "int64_t taida_global_set(int64_t key, int64_t val);".to_string(),

        _ => return None,
    };
    Some(proto)
}

/// 現在の関数のパラメータ名（TailCall で使用）
struct FuncContext<'a> {
    param_names: Vec<String>,
    global_map: &'a HashMap<i64, String>,
    func_user_arity: &'a HashMap<String, usize>,
}

/// 単一関数を C コードに変換
fn emit_function(
    c: &mut String,
    func: &IrFunction,
    global_map: &HashMap<i64, String>,
    func_user_arity: &HashMap<String, usize>,
) -> Result<(), WasmCEmitError> {
    // 関数シグネチャ
    write!(c, "int64_t {}(", func.name).unwrap();
    for (i, param_name) in func.params.iter().enumerate() {
        if i > 0 {
            write!(c, ", ").unwrap();
        }
        write!(c, "int64_t v_{}", param_to_var_idx(param_name, i)).unwrap();
    }
    if func.params.is_empty() {
        write!(c, "void").unwrap();
    }
    writeln!(c, ") {{").unwrap();

    // ローカル変数の宣言（全 IrVar を事前宣言）
    // パラメータは既に関数引数として宣言されている
    let param_count = func.params.len() as u32;
    if func.next_var > param_count {
        for v in param_count..func.next_var {
            writeln!(c, "    int64_t v_{} = 0;", v).unwrap();
        }
    }

    // Named variables（DefVar/UseVar 用）
    let mut named_vars = HashSet::new();
    collect_named_vars(&func.body, &mut named_vars);
    // パラメータ名も named_vars に含める
    for param_name in &func.params {
        named_vars.insert(param_name.clone());
    }
    for name in &named_vars {
        writeln!(c, "    int64_t nv_{} = 0;", sanitize_name(name)).unwrap();
    }

    // パラメータを named variables にコピー（IR は DefVar なしで UseVar("n") を使う）
    for (i, param_name) in func.params.iter().enumerate() {
        writeln!(c, "    nv_{} = v_{};", sanitize_name(param_name), i).unwrap();
    }

    let fctx = FuncContext {
        param_names: func.params.clone(),
        global_map,
        func_user_arity,
    };

    // 末尾再帰のサポート: TailCall を含む場合はループで囲む
    let has_tail_call = contains_tail_call(&func.body);
    if has_tail_call {
        writeln!(c, "    while (1) {{").unwrap();
        emit_insts(c, &func.body, "        ", &fctx)?;
        writeln!(c, "    }}").unwrap();
    } else {
        // 命令列
        emit_insts(c, &func.body, "    ", &fctx)?;
    }

    // デフォルト return（最後の命令が Return でない場合）
    if !func
        .body
        .last()
        .is_some_and(|i| matches!(i, IrInst::Return(_)))
    {
        writeln!(c, "    return 0;").unwrap();
    }

    writeln!(c, "}}").unwrap();
    Ok(())
}

fn collect_named_vars(insts: &[IrInst], set: &mut HashSet<String>) {
    for inst in insts {
        match inst {
            IrInst::DefVar(name, _) => {
                set.insert(name.clone());
            }
            IrInst::UseVar(_, name) => {
                set.insert(name.clone());
            }
            IrInst::CondBranch(_, arms) => {
                for arm in arms {
                    collect_named_vars(&arm.body, set);
                }
            }
            _ => {}
        }
    }
}

fn emit_insts(
    c: &mut String,
    insts: &[IrInst],
    indent: &str,
    fctx: &FuncContext,
) -> Result<(), WasmCEmitError> {
    for inst in insts {
        emit_inst(c, inst, indent, fctx)?;
    }
    Ok(())
}

fn emit_inst(
    c: &mut String,
    inst: &IrInst,
    indent: &str,
    fctx: &FuncContext,
) -> Result<(), WasmCEmitError> {
    match inst {
        IrInst::ConstInt(dst, val) => {
            writeln!(c, "{}v_{} = {}LL;", indent, dst, val).unwrap();
        }
        IrInst::ConstFloat(dst, val) => {
            // W-3: Store f64 bits in int64_t via bitcast (same representation as native backend)
            // Use _d2l() helper to bitcast double -> int64_t
            // Format with enough precision to round-trip
            writeln!(c, "{}v_{} = _d2l({:.17e});", indent, dst, val).unwrap();
        }
        IrInst::ConstBool(dst, val) => {
            writeln!(c, "{}v_{} = {};", indent, dst, if *val { 1 } else { 0 }).unwrap();
        }
        IrInst::ConstStr(dst, s) => {
            // 静的文字列リテラル: ポインタを i64 として格納
            writeln!(
                c,
                "{}v_{} = (int64_t)(intptr_t){};",
                indent,
                dst,
                c_string_literal(s)
            )
            .unwrap();
        }
        IrInst::DefVar(name, src) => {
            writeln!(c, "{}nv_{} = v_{};", indent, sanitize_name(name), src).unwrap();
        }
        IrInst::UseVar(dst, name) => {
            writeln!(c, "{}v_{} = nv_{};", indent, dst, sanitize_name(name)).unwrap();
        }
        IrInst::Call(dst, name, args) => {
            // void-returning functions: RC no-ops + tag setters + gorilla (noreturn)
            if name == "taida_retain"
                || name == "taida_release"
                || name == "taida_str_retain"
                || name == "taida_list_set_elem_tag"
                || name == "taida_hashmap_set_value_tag"
                || name == "taida_set_set_elem_tag"
                || name == "taida_error_ceiling_pop"
                || name == "taida_register_type_parent"
                || name == "taida_gorilla"
                || name == "taida_push_call_tags"
                || name == "taida_pop_call_tags"
            {
                write!(c, "{}{}(", indent, name).unwrap();
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(c, ", ").unwrap();
                    }
                    write!(c, "v_{}", arg).unwrap();
                }
                writeln!(c, ");").unwrap();
                writeln!(c, "{}v_{} = 0;", indent, dst).unwrap();
            } else {
                write!(c, "{}v_{} = {}(", indent, dst, name).unwrap();
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(c, ", ").unwrap();
                    }
                    write!(c, "v_{}", arg).unwrap();
                }
                writeln!(c, ");").unwrap();
                // RCB-101 fix: In WASM, taida_throw sets a flag instead of
                // longjmp.  After taida_error_type_check_or_rethrow re-throws,
                // the handler body must not continue — return immediately so
                // the error propagates to the outer ceiling.
                if name == "taida_error_type_check_or_rethrow" {
                    writeln!(c, "{}if (taida_is_error_thrown()) return 0;", indent).unwrap();
                }
            }
        }
        IrInst::CallUser(dst, name, args) => {
            write!(c, "{}v_{} = {}(", indent, dst, name).unwrap();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    write!(c, ", ").unwrap();
                }
                write!(c, "v_{}", arg).unwrap();
            }
            writeln!(c, ");").unwrap();
        }
        IrInst::CondBranch(result, arms) => {
            for (i, arm) in arms.iter().enumerate() {
                if i == 0 {
                    if let Some(cond) = arm.condition {
                        writeln!(c, "{}if (v_{}) {{", indent, cond).unwrap();
                    } else {
                        writeln!(c, "{}{{", indent).unwrap();
                    }
                } else if arm.condition.is_some() {
                    writeln!(c, "{}}} else if (v_{}) {{", indent, arm.condition.unwrap()).unwrap();
                } else {
                    writeln!(c, "{}}} else {{", indent).unwrap();
                }

                let inner_indent = format!("{}    ", indent);
                emit_insts(c, &arm.body, &inner_indent, fctx)?;
                writeln!(c, "{}    v_{} = v_{};", indent, result, arm.result).unwrap();
            }
            writeln!(c, "{}}}", indent).unwrap();
        }
        IrInst::Return(var) => {
            writeln!(c, "{}return v_{};", indent, var).unwrap();
        }
        // wasm-min で未サポートの命令
        IrInst::Retain(_) | IrInst::Release(_) => {
            // RC 操作は wasm-min では無視（ヒープなし）
            writeln!(c, "{}/* retain/release skipped (wasm-min) */", indent).unwrap();
        }
        // F-4: グローバル変数を名前ベースの C 変数で読み書き
        IrInst::GlobalSet(name_hash, value_var) => {
            let var_name = fctx
                .global_map
                .get(name_hash)
                .expect("global hash not in map");
            writeln!(c, "{}{} = v_{};", indent, var_name, value_var).unwrap();
        }
        IrInst::GlobalGet(dst, name_hash) => {
            let var_name = fctx
                .global_map
                .get(name_hash)
                .expect("global hash not in map");
            writeln!(c, "{}v_{} = {};", indent, dst, var_name).unwrap();
        }
        // W-4: BuchiPack operations
        IrInst::PackNew(dst, field_count) => {
            writeln!(
                c,
                "{}v_{} = taida_pack_new({}LL);",
                indent, dst, field_count
            )
            .unwrap();
        }
        IrInst::PackSet(pack_var, index, value_var) => {
            writeln!(
                c,
                "{}taida_pack_set(v_{}, {}LL, v_{});",
                indent, pack_var, index, value_var
            )
            .unwrap();
        }
        IrInst::PackSetTag(pack_var, index, tag) => {
            writeln!(
                c,
                "{}taida_pack_set_tag(v_{}, {}LL, {}LL);",
                indent, pack_var, index, tag
            )
            .unwrap();
        }
        IrInst::PackGet(dst, pack_var, index) => {
            writeln!(
                c,
                "{}v_{} = taida_pack_get_idx(v_{}, {}LL);",
                indent, dst, pack_var, index
            )
            .unwrap();
        }
        // W-5: FuncAddr — get a function pointer as int64_t
        IrInst::FuncAddr(dst, func_name) => {
            writeln!(
                c,
                "{}v_{} = (int64_t)(intptr_t)&{};",
                indent, dst, func_name
            )
            .unwrap();
        }
        // W-5: MakeClosure — create a closure (env pack + function pointer)
        IrInst::MakeClosure(dst, func_name, captures) => {
            // 1. Create environment pack with captured variables
            let env_var = format!("_env_{}", dst);
            writeln!(
                c,
                "{}int64_t {} = taida_pack_new({}LL);",
                indent,
                env_var,
                captures.len()
            )
            .unwrap();
            for (i, cap_name) in captures.iter().enumerate() {
                // Set hash to 0 (not needed for index-based access)
                writeln!(
                    c,
                    "{}taida_pack_set({}, {}LL, nv_{});",
                    indent,
                    env_var,
                    i,
                    sanitize_name(cap_name)
                )
                .unwrap();
            }
            // 2. Create closure: taida_closure_new(fn_ptr, env_ptr, user_arity)
            // W-5g: user_arity is needed for WASM indirect call type matching
            let user_arity = fctx
                .func_user_arity
                .get(func_name.as_str())
                .copied()
                .unwrap_or(0);
            writeln!(
                c,
                "{}v_{} = taida_closure_new((int64_t)(intptr_t)&{}, {}, {}LL);",
                indent, dst, func_name, env_var, user_arity
            )
            .unwrap();
        }
        // W-5: CallIndirect — indirect function call (closure or plain function pointer)
        IrInst::CallIndirect(dst, fn_var, args) => {
            // Check if it's a closure or a plain function pointer
            writeln!(c, "{}if (taida_is_closure_value(v_{})) {{", indent, fn_var).unwrap();
            // Closure path: extract fn_ptr and env_ptr, call with env as first arg
            writeln!(
                c,
                "{}    int64_t _ci_fn = taida_closure_get_fn(v_{});",
                indent, fn_var
            )
            .unwrap();
            writeln!(
                c,
                "{}    int64_t _ci_env = taida_closure_get_env(v_{});",
                indent, fn_var
            )
            .unwrap();
            // Build closure call: fn(env, arg0, arg1, ...)
            let closure_argc = args.len() + 1; // env + user args
            write!(c, "{}    v_{} = ((int64_t (*)(", indent, dst).unwrap();
            for i in 0..closure_argc {
                if i > 0 {
                    write!(c, ", ").unwrap();
                }
                write!(c, "int64_t").unwrap();
            }
            write!(c, "))(intptr_t)_ci_fn)(_ci_env").unwrap();
            for arg in args {
                write!(c, ", v_{}", arg).unwrap();
            }
            writeln!(c, ");").unwrap();
            writeln!(c, "{}}} else {{", indent).unwrap();
            // Plain function pointer path: call directly
            write!(c, "{}    v_{} = ((int64_t (*)(", indent, dst).unwrap();
            for (i, _) in args.iter().enumerate() {
                if i > 0 {
                    write!(c, ", ").unwrap();
                }
                write!(c, "int64_t").unwrap();
            }
            if args.is_empty() {
                write!(c, "void").unwrap();
            }
            write!(c, "))(intptr_t)v_{})(", fn_var).unwrap();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    write!(c, ", ").unwrap();
                }
                write!(c, "v_{}", arg).unwrap();
            }
            writeln!(c, ");").unwrap();
            writeln!(c, "{}}}", indent).unwrap();
        }
        IrInst::TailCall(args) => {
            // 末尾再帰: TailCall(args) の args を一時変数に評価してから
            // named variables を更新し、continue でループ先頭に戻る。
            // Cranelift emit.rs と同じく、全 args を先に評価してから代入する
            // （引数間の依存を回避するため）。
            for (i, arg) in args.iter().enumerate() {
                writeln!(c, "{}int64_t _tco_arg_{} = v_{};", indent, i, arg).unwrap();
            }
            for (i, param_name) in fctx.param_names.iter().enumerate() {
                if i < args.len() {
                    writeln!(
                        c,
                        "{}nv_{} = _tco_arg_{};",
                        indent,
                        sanitize_name(param_name),
                        i
                    )
                    .unwrap();
                }
            }
            writeln!(c, "{}continue;", indent).unwrap();
        }
    }
    Ok(())
}

/// IR 命令列に TailCall が含まれるかどうかを再帰的にチェック
fn contains_tail_call(insts: &[IrInst]) -> bool {
    for inst in insts {
        match inst {
            IrInst::TailCall(_) => return true,
            IrInst::CondBranch(_, arms) => {
                for arm in arms {
                    if contains_tail_call(&arm.body) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// パラメータ名からインデックスを生成（IrVar はパラメータ順に 0, 1, 2, ...）
fn param_to_var_idx(_name: &str, idx: usize) -> u32 {
    idx as u32
}

/// 変数名を C 識別子として安全な形に変換
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// NET-6: WASM capability gating tests for taida-lang/net HTTP API
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// NET-6: All net HTTP runtime function names.
    const NET_RUNTIME_FUNCS: &[&str] = &[
        "taida_net_http_serve",
        "taida_net_http_parse_request_head",
        "taida_net_http_encode_response",
        "taida_net_read_body",
    ];

    /// NET-6: Human-readable API names expected in error messages.
    const NET_API_NAMES: &[&str] = &[
        "httpServe",
        "httpParseRequestHead",
        "httpEncodeResponse",
        "readBody",
    ];

    /// NET-6b: wasm-min rejects all 3 net HTTP API functions with profile-specific error.
    #[test]
    fn test_wasm_min_rejects_net_http_api() {
        for (rt_name, api_name) in NET_RUNTIME_FUNCS.iter().zip(NET_API_NAMES.iter()) {
            let result = runtime_func_prototype(rt_name, WasmProfile::Min);
            assert!(result.is_err(), "wasm-min should reject {}", rt_name);
            let msg = result.unwrap_err().message;
            assert!(
                msg.contains("wasm-min"),
                "error for {} should contain 'wasm-min', got: {}",
                rt_name,
                msg
            );
            assert!(
                msg.contains(api_name),
                "error for {} should contain '{}', got: {}",
                rt_name,
                api_name,
                msg
            );
            assert!(
                msg.contains("taida-lang/net"),
                "error for {} should reference taida-lang/net, got: {}",
                rt_name,
                msg
            );
        }
    }

    /// NET-6c: wasm-wasi rejects all 3 net HTTP API functions with profile-specific error.
    #[test]
    fn test_wasm_wasi_rejects_net_http_api() {
        for (rt_name, api_name) in NET_RUNTIME_FUNCS.iter().zip(NET_API_NAMES.iter()) {
            let result = runtime_func_prototype(rt_name, WasmProfile::Wasi);
            assert!(result.is_err(), "wasm-wasi should reject {}", rt_name);
            let msg = result.unwrap_err().message;
            assert!(
                msg.contains("wasm-wasi"),
                "error for {} should contain 'wasm-wasi', got: {}",
                rt_name,
                msg
            );
            assert!(
                msg.contains(api_name),
                "error for {} should contain '{}', got: {}",
                rt_name,
                api_name,
                msg
            );
            assert!(
                msg.contains("taida-lang/net"),
                "error for {} should reference taida-lang/net, got: {}",
                rt_name,
                msg
            );
        }
    }

    /// NET-6d: wasm-edge rejects all 3 net HTTP API functions with profile-specific error.
    #[test]
    fn test_wasm_edge_rejects_net_http_api() {
        for (rt_name, api_name) in NET_RUNTIME_FUNCS.iter().zip(NET_API_NAMES.iter()) {
            let result = runtime_func_prototype(rt_name, WasmProfile::Edge);
            assert!(result.is_err(), "wasm-edge should reject {}", rt_name);
            let msg = result.unwrap_err().message;
            assert!(
                msg.contains("wasm-edge"),
                "error for {} should contain 'wasm-edge', got: {}",
                rt_name,
                msg
            );
            assert!(
                msg.contains(api_name),
                "error for {} should contain '{}', got: {}",
                rt_name,
                api_name,
                msg
            );
            assert!(
                msg.contains("taida-lang/net"),
                "error for {} should reference taida-lang/net, got: {}",
                rt_name,
                msg
            );
        }
    }

    /// NET-6e: wasm-full rejects all 3 net HTTP API functions with profile-specific error.
    #[test]
    fn test_wasm_full_rejects_net_http_api() {
        for (rt_name, api_name) in NET_RUNTIME_FUNCS.iter().zip(NET_API_NAMES.iter()) {
            let result = runtime_func_prototype(rt_name, WasmProfile::Full);
            assert!(result.is_err(), "wasm-full should reject {}", rt_name);
            let msg = result.unwrap_err().message;
            assert!(
                msg.contains("wasm-full"),
                "error for {} should contain 'wasm-full', got: {}",
                rt_name,
                msg
            );
            assert!(
                msg.contains(api_name),
                "error for {} should contain '{}', got: {}",
                rt_name,
                api_name,
                msg
            );
            assert!(
                msg.contains("taida-lang/net"),
                "error for {} should reference taida-lang/net, got: {}",
                rt_name,
                msg
            );
        }
    }

    /// NET-6f: error messages contain profile name for all 4 profiles x 3 functions.
    #[test]
    fn test_net_http_error_contains_profile_name() {
        let profiles = [
            (WasmProfile::Min, "wasm-min"),
            (WasmProfile::Wasi, "wasm-wasi"),
            (WasmProfile::Edge, "wasm-edge"),
            (WasmProfile::Full, "wasm-full"),
        ];
        for (profile, profile_name) in &profiles {
            for rt_name in NET_RUNTIME_FUNCS {
                let result = runtime_func_prototype(rt_name, *profile);
                assert!(
                    result.is_err(),
                    "{} should reject {}",
                    profile_name,
                    rt_name
                );
                let msg = result.unwrap_err().message;
                assert!(
                    msg.contains(profile_name),
                    "error for {} on {} should contain '{}', got: {}",
                    rt_name,
                    profile_name,
                    profile_name,
                    msg
                );
            }
        }
    }

    /// NET-6: net HTTP API errors suggest interpreter, JS, or native -- not other WASM profiles.
    #[test]
    fn test_net_http_error_suggests_correct_backends() {
        for rt_name in NET_RUNTIME_FUNCS {
            let result = runtime_func_prototype(rt_name, WasmProfile::Min);
            let msg = result.unwrap_err().message;
            // Should suggest non-WASM backends
            assert!(
                msg.contains("interpreter") || msg.contains("native") || msg.contains("JS"),
                "error should suggest non-WASM backends, got: {}",
                msg
            );
            // Should NOT suggest other WASM profiles (net is unsupported on ALL WASM)
            assert!(
                !msg.contains("wasm-wasi")
                    && !msg.contains("wasm-edge")
                    && !msg.contains("wasm-full"),
                "error should not suggest other WASM profiles for net API, got: {}",
                msg
            );
        }
    }

    /// NET-6: the net block is reached before wasm-full's catch-all wildcard.
    /// This verifies wasm-full doesn't silently accept net functions through wasm_full_runtime_prototype.
    #[test]
    fn test_wasm_full_net_not_in_full_runtime_prototype() {
        // wasm_full_runtime_prototype should NOT have entries for net functions
        for rt_name in NET_RUNTIME_FUNCS {
            assert!(
                wasm_full_runtime_prototype(rt_name).is_none(),
                "wasm_full_runtime_prototype should not accept {}",
                rt_name
            );
        }
    }

    /// NET-6 fix: validate_net_http_api_for_wasm fires before individual prototype checks.
    /// This ensures net-specific errors take priority over argument-level errors
    /// (e.g., Bytes mold hitting a generic unsupported error on wasm-edge).
    #[test]
    fn test_validate_net_http_api_early_out() {
        let profiles = [
            (WasmProfile::Min, "wasm-min"),
            (WasmProfile::Wasi, "wasm-wasi"),
            (WasmProfile::Edge, "wasm-edge"),
            (WasmProfile::Full, "wasm-full"),
        ];
        for (profile, profile_name) in &profiles {
            // A set containing both Bytes mold AND httpParseRequestHead
            // should produce a net-specific error, not a Bytes error
            let mut funcs = HashSet::new();
            funcs.insert("taida_bytes_mold".to_string());
            funcs.insert("taida_net_http_parse_request_head".to_string());

            let result = validate_net_http_api_for_wasm(&funcs, *profile);
            assert!(
                result.is_err(),
                "{} should reject net HTTP API",
                profile_name
            );
            let msg = result.unwrap_err().message;
            assert!(
                msg.contains("taida-lang/net"),
                "{}: error should reference taida-lang/net, got: {}",
                profile_name,
                msg
            );
            assert!(
                msg.contains("httpParseRequestHead"),
                "{}: error should name the API, got: {}",
                profile_name,
                msg
            );
            assert!(
                msg.contains(profile_name),
                "{}: error should contain profile name, got: {}",
                profile_name,
                msg
            );
        }
    }

    /// NET-6 fix: validate_net_http_api_for_wasm does NOT fire when no net functions are present.
    #[test]
    fn test_validate_net_http_api_no_false_positive() {
        let mut funcs = HashSet::new();
        funcs.insert("taida_bytes_mold".to_string());
        funcs.insert("taida_io_stdout".to_string());

        // No net functions → should pass
        for profile in &[
            WasmProfile::Min,
            WasmProfile::Wasi,
            WasmProfile::Edge,
            WasmProfile::Full,
        ] {
            let result = validate_net_http_api_for_wasm(&funcs, *profile);
            assert!(
                result.is_ok(),
                "should not reject when no net functions are present"
            );
        }
    }
}

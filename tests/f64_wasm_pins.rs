//! Executable coverage of WASM host contracts and runtime boundaries.
#![cfg(feature = "native")]
mod common;
use common::{taida_bin, unique_temp_dir, wasmtime_bin};
use std::process::{Command, Output};
use taida::codegen::driver::{WasmRuntimeCache, find_wasm_ld};
use taida::codegen::emit_wasm_c::{WasmProfile, emit_c};
use taida::codegen::ir::{IrFunction, IrInst, IrModule};

fn wasm_harness(source: &str) -> Option<Output> {
    let Some(wasmtime) = wasmtime_bin() else {
        eprintln!("SKIP: wasmtime is unavailable");
        return None;
    };
    let dir = unique_temp_dir("wasm_runtime_contract");
    let cache = WasmRuntimeCache::new(dir.join("cache")).unwrap();
    let core = cache.rt_core(WasmProfile::Min).unwrap();
    let src = dir.join("check.c");
    let obj = dir.join("check.o");
    let wasm = dir.join("check.wasm");
    std::fs::write(&src, source).unwrap();
    let compiled = Command::new(cache.clang())
        .args([
            "--target=wasm32-unknown-wasi",
            "-nostdlib",
            "-O2",
            "-fwrapv",
            "-c",
            "-I",
        ])
        .arg(cache.include_dir())
        .arg(&src)
        .arg("-o")
        .arg(&obj)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let linked = Command::new(find_wasm_ld().expect("WASM linker must be available"))
        .args(["--no-entry", "--export=_start", "--gc-sections"])
        .arg(&core)
        .arg(&obj)
        .arg("-o")
        .arg(&wasm)
        .output()
        .unwrap();
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let out = Command::new(wasmtime).arg(&wasm).output().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    Some(out)
}

fn assert_wasm_ok(source: &str) {
    if let Some(out) = wasm_harness(source) {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn wasi_errors_match_preview1_numbers() {
    let runtime = include_str!("../src/codegen/runtime_wasi_io.c");
    assert_wasm_ok(&format!(
        r#"{runtime}
static int equal(const char *a, const char *b) {{
    while (*a && *a == *b) {{ a++; b++; }}
    return *a == *b;
}}
int64_t _taida_main(void) {{
    if (WASI_ERRNO_NOTSOCK != 57) __builtin_trap();
    int errors[] = {{14, 15, 13, 64, 53, 54, 57, 61}};
    const char *kinds[] = {{"refused", "reset", "peer_closed", "peer_closed", "peer_closed", "other", "other", "other"}};
    for (int i = 0; i < 8; i++)
        if (!equal(wasi_error_kind(errors[i], 0), kinds[i])) __builtin_trap();
    return 0;
}}
"#
    ));
}

#[test]
fn response_headers_reject_non_lists_and_corrupt_lengths() {
    let runtime = include_str!("../src/codegen/runtime_abi_web_wasm.c");
    assert_wasm_ok(&format!(
        r#"{runtime}
int64_t _taida_main(void) {{
    if (abi_pair_list_copy(7) || abi_pair_list_copy(-1) ||
        abi_pair_list_copy(0x7fffffffffffffffLL) || abi_pair_list_copy(WSTR("bad"))) __builtin_trap();
    int64_t list = taida_list_new();
    int64_t *p = (int64_t *)(intptr_t)list;
    if (!abi_pair_list_copy(list)) __builtin_trap();
    p[1] = -1;
    if (abi_pair_list_copy(list)) __builtin_trap();
    p[1] = 1000000;
    if (abi_pair_list_copy(list)) __builtin_trap();
    int64_t response = abi_response_new(200, 7, abi_bytes_default());
    response = taida_abi_response_header(WSTR("x-test"), WSTR("ok"), response);
    if (taida_pack_get(response, abi_hash_cstr("status")) != 500) __builtin_trap();
    return 0;
}}
"#
    ));
}

#[test]
fn full_global_table_preserves_updates_and_traps_on_overflow() {
    let runtime = include_str!("../src/codegen/runtime_full_wasm.c");
    let source = format!(
        r#"{runtime}
int64_t _taida_main(void) {{
    for (int i = 0; i < 64; i++) taida_global_set(i, i + 10);
    for (int i = 0; i < 64; i++)
        if (taida_global_get(i) != i + 10) __builtin_trap();
    taida_global_set(63, 900);
    if (taida_global_get(63) != 900) __builtin_trap();
    OVERFLOW
    return 0;
}}
"#
    );
    assert_wasm_ok(&source.replace("OVERFLOW", ""));
    if let Some(out) = wasm_harness(&source.replace("OVERFLOW", "taida_global_set(64, 901);")) {
        assert!(
            !out.status.success(),
            "table overflow was silently accepted"
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("unreachable"));
    }
}

#[test]
fn nul_followed_by_octal_digit_keeps_both_bytes() {
    let mut module = IrModule::new();
    let mut func = IrFunction::new("nul_bytes".into());
    let value = func.alloc_var();
    func.push(IrInst::ConstStr(value, "\0".to_owned() + "5"));
    func.push(IrInst::Return(value));
    module.functions.push(func);
    let generated = emit_c(&module, WasmProfile::Min).unwrap();
    assert_wasm_ok(&format!(
        r#"{generated}
int64_t _taida_main(void) {{
    const unsigned char *p = (const unsigned char *)(intptr_t)nul_bytes();
    if (p[0] != 0 || p[1] != '5' || p[2] != 0) __builtin_trap();
    return 0;
}}
"#
    ));
}

#[test]
fn edge_env_distinguishes_missing_empty_and_boundary_values() {
    if Command::new("node").arg("--version").output().is_err() {
        return;
    }
    let dir = unique_temp_dir("edge_env_contract");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"empty <= EnvVar["EMPTY"]()
stdout(empty.hasValue())
empty >=> value
stdout(value.length())
missing <= EnvVar["MISSING"]()
stdout(missing.hasValue())
boundary <= EnvVar["BOUNDARY"]()
boundary >=> s
stdout(s.length())
large <= EnvVar["LARGE"]()
large >=> t
stdout(t.length())
"#,
    )
    .unwrap();
    let wasm = dir.join("main.wasm");
    let built = Command::new(taida_bin())
        .args(["build", "wasm-edge"])
        .arg(&td)
        .arg("-o")
        .arg(&wasm)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let glue = std::fs::read_to_string(dir.join("main.edge.js")).unwrap();
    let glue = glue.replace("import WASM from \"./main.wasm\";", "import fs from 'node:fs'; const WASM = new WebAssembly.Module(fs.readFileSync(new URL('./main.wasm', import.meta.url))); ");
    let script = dir.join("run.mjs");
    std::fs::write(&script, format!("{glue}\nconst response = await handleTaidaRequest(new Request('https://example.test/'), {{EMPTY: '', BOUNDARY: 'x'.repeat(256), LARGE: 'x'.repeat(4096)}}, {{}}); process.stdout.write(await response.text());")).unwrap();
    let out = Command::new("node").arg(script).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        "true\n0\nfalse\n256\n4096"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

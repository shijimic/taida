#![cfg(feature = "native")]
/// Integration tests for wasm-edge backend.
///
/// WE-2/WE-3: Validates that wasm-edge compiles correctly,
/// rejects unsupported APIs, and does not regress wasm-min/wasm-wasi.
///
/// Note: wasm-edge outputs require a JS glue host (or wasmtime with
/// taida_host imports provided). For compile-only tests we just verify
/// the compilation succeeds or fails as expected. For runtime tests,
/// we use wasmtime (which provides wasi_snapshot_preview1.fd_write)
/// for the basic stdout path -- the wasm-edge hello example only needs
/// fd_write and does NOT use taida_host imports.
mod common;

use common::{taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::Command;

/// Compile a .td file with wasm-edge and return the wasm path (or None on failure).
fn compile_wasm_edge(td_path: &Path, wasm_path: &Path) -> Option<String> {
    compile_wasm_profile("wasm-edge", td_path, wasm_path)
}

fn compile_wasm_profile(target: &str, td_path: &Path, wasm_path: &Path) -> Option<String> {
    let output = Command::new(taida_bin())
        .args(["build", target])
        .arg(td_path)
        .arg("-o")
        .arg(wasm_path)
        .output()
        .ok()?;

    if output.status.success() {
        None // no error
    } else {
        Some(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn compile_wasm_edge_handler(td_path: &Path, wasm_path: &Path, handler: &str) -> Option<String> {
    let output = Command::new(taida_bin())
        .args(["build", "wasm-edge", "--no-cache", "--handler", handler])
        .arg(td_path)
        .arg("-o")
        .arg(wasm_path)
        .output()
        .ok()?;

    if output.status.success() {
        None
    } else {
        Some(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

// ---------------------------------------------------------------------------
// WE-3a: Smoke tests
// ---------------------------------------------------------------------------

/// Test: wasm-edge compiles the hello example.
/// The resulting .wasm only uses wasi_snapshot_preview1.fd_write (no taida_host),
/// so wasmtime can run it directly.
#[test]
fn wasm_edge_hello_compiles() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_edge_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_test_hello.wasm");

    let err = compile_wasm_edge(&td_path, &wasm_path);
    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        err.is_none(),
        "wasm-edge hello should compile, got: {:?}",
        err
    );
}

/// Test: wasm-edge hello produces correct output when run with wasmtime.
/// wasmtime provides wasi_snapshot_preview1.fd_write which is all hello needs.
#[test]
fn wasm_edge_hello_runs() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-edge runtime test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_edge_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_test_hello_run.wasm");

    let err = compile_wasm_edge(&td_path, &wasm_path);
    assert!(err.is_none(), "compile failed: {:?}", err);

    let run = Command::new(&wasmtime)
        .arg("run")
        .arg("--")
        .arg(&wasm_path)
        .output()
        .expect("wasmtime should run");
    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        run.status.success(),
        "wasmtime failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();
    assert_eq!(stdout, "Hello from edge!");
}

/// Test: wasm-edge can run the pure `taida-lang/crypto` SHA-256 subset
/// without a host capability bridge.
#[test]
fn wasm_edge_crypto_sha256_runs() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-edge crypto runtime test");
            return;
        }
    };

    let stem = format!("taida_wasm_edge_crypto_sha256_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("{}.wasm", stem));
    let source = r#">>> taida-lang/crypto => @(sha256)
stdout(sha256(""))
stdout(sha256("abc"))
stdout(sha256("\x01hello"))
stdout(sha256("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
"#;

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
    std::fs::write(&td_path, source).expect("write crypto fixture");

    let err = compile_wasm_edge(&td_path, &wasm_path);
    assert!(err.is_none(), "compile failed: {:?}", err);

    let run = Command::new(&wasmtime)
        .arg("run")
        .arg("--")
        .arg(&wasm_path)
        .output()
        .expect("wasmtime should run");
    assert!(
        run.status.success(),
        "wasmtime failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();
    assert_eq!(
        stdout,
        [
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "cceeb7a985ecc3dabcb4c8f666cd637f16f008e3c963db6aa6f83a7b288c54ef",
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
        ]
        .join("\n")
    );

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
}

/// Test: wasm-full hashes the concrete Bytes runtime layout directly.
#[test]
fn wasm_full_crypto_sha256_bytes_runs() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full crypto Bytes runtime test");
            return;
        }
    };

    let stem = format!("taida_wasm_full_crypto_sha256_bytes_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("{}.wasm", stem));
    let source = r#">>> taida-lang/crypto => @(sha256)
emptyLax <= Bytes[@[]]()
emptyLax >=> emptyBytes
abcLax <= Bytes[@[65, 66, 67]]()
abcLax >=> abcBytes
nulLax <= Bytes[@[72, 0, 73]]()
nulLax >=> nulBytes
zeroLax <= Bytes[@[0, 0, 0, 0]]()
zeroLax >=> zeroBytes
stdout(sha256(emptyBytes))
stdout(sha256(abcBytes))
stdout(sha256(nulBytes))
stdout(sha256(zeroBytes))
"#;

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
    std::fs::write(&td_path, source).expect("write wasm-full crypto Bytes fixture");

    let err = compile_wasm_profile("wasm-full", &td_path, &wasm_path);
    assert!(err.is_none(), "compile failed: {:?}", err);

    let run = Command::new(&wasmtime)
        .arg("run")
        .arg("--")
        .arg(&wasm_path)
        .output()
        .expect("wasmtime should run");
    assert!(
        run.status.success(),
        "wasmtime failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();
    assert_eq!(
        stdout,
        [
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "b5d4045c3f466fa91fe2cc6abe79232a1a57cdf104f7a26e716e0a1e2789df78",
            "827768ed493ac4f0c3f0374242e98408ced57830e9d0bf65b99d730502255776",
            "df3f619804a92fdb4057192dc43dd748ea778adc52bc498ce80524c014b81119",
        ]
        .join("\n")
    );

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
}

#[test]
fn wasm_crypto_runtime_uses_sha256_specific_input_limit() {
    let runtime = include_str!("../src/codegen/runtime_core_wasm/02_containers.inc.c");
    assert!(runtime.contains("TAIDA_WASM_SHA256_MAX_INPUT_BYTES"));
    assert!(!runtime.contains("len <= 0x1000000"));
}

/// Test: wasm-edge env example compiles (runtime test skipped -- needs taida_host imports).
///
/// Note: The wasm-edge env example uses `taida_host.env_get` and `taida_host.env_get_all`
/// imports. wasmtime does not know about the `taida_host` module, so runtime execution
/// is not possible without a JS glue host or a custom wasmtime host stub.
/// This compile-only test verifies the ABI is correctly emitted.
/// Full runtime validation of the ptr/len ABI requires the JS glue (WE-2d).
#[test]
fn wasm_edge_env_compiles() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_edge_env.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_test_env.wasm");

    let err = compile_wasm_edge(&td_path, &wasm_path);
    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        err.is_none(),
        "wasm-edge env should compile, got: {:?}",
        err
    );
}

/// Test: wasm-edge hello runs correctly via wasmtime, verifying basic stdout ABI.
///
/// This is the runtime baseline: code that does NOT use taida_host imports
/// (only wasi_snapshot_preview1.fd_write) can be executed with wasmtime.
/// This implicitly validates the wasm binary structure, memory layout,
/// and the fd_write ABI contract.
///
/// For env API runtime tests, the JS glue host (WE-2d) is required because
/// wasmtime cannot provide the `taida_host` module imports.
#[test]
fn wasm_edge_env_runtime_requires_host() {
    // This test documents the constraint: env-using .td files compile but
    // cannot be run with wasmtime alone because taida_host imports are missing.
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-edge env runtime constraint test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_edge_env.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_test_env_run.wasm");

    let err = compile_wasm_edge(&td_path, &wasm_path);
    assert!(err.is_none(), "compile failed: {:?}", err);

    // wasmtime should fail because taida_host imports are not provided
    let run = Command::new(&wasmtime)
        .arg("run")
        .arg("--")
        .arg(&wasm_path)
        .output()
        .expect("wasmtime should execute");
    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        !run.status.success(),
        "wasmtime should fail for env example (missing taida_host imports), but succeeded"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("taida_host") || stderr.contains("unknown import"),
        "error should mention taida_host or unknown import, got: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// WE-3b: Negative tests
// ---------------------------------------------------------------------------

/// Test: wasm-edge rejects file I/O APIs with clear error message.
#[test]
fn wasm_edge_rejects_file_io() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_wasi_file_io.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_test_file_reject.wasm");

    let err = compile_wasm_edge(&td_path, &wasm_path);
    let _ = std::fs::remove_file(&wasm_path);

    assert!(err.is_some(), "wasm-edge should reject file I/O");
    let msg = err.unwrap();
    assert!(
        msg.contains("wasm-edge does not support"),
        "error should mention wasm-edge, got: {}",
        msg
    );
}

/// Test: wasm-edge rejects process APIs (execShell) with clear error message.
#[test]
fn wasm_edge_rejects_process() {
    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_edge/reject_process.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_test_process_reject.wasm");

    let err = compile_wasm_edge(&td_path, &wasm_path);
    let _ = std::fs::remove_file(&wasm_path);

    assert!(err.is_some(), "wasm-edge should reject process API");
    let msg = err.unwrap();
    assert!(
        msg.contains("wasm-edge does not support") && msg.contains("taida_os_exec_shell"),
        "error should mention wasm-edge and taida_os_exec_shell, got: {}",
        msg
    );
}

/// Test: wasm-edge rejects socket APIs (tcpConnect) with clear error message.
#[test]
fn wasm_edge_rejects_socket() {
    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_edge/reject_socket.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_test_socket_reject.wasm");

    let err = compile_wasm_edge(&td_path, &wasm_path);
    let _ = std::fs::remove_file(&wasm_path);

    assert!(err.is_some(), "wasm-edge should reject socket API");
    let msg = err.unwrap();
    assert!(
        msg.contains("wasm-edge does not support") && msg.contains("taida_os_tcp_connect"),
        "error should mention wasm-edge and taida_os_tcp_connect, got: {}",
        msg
    );
}

/// Test: wasm-edge supports local module imports via inline expansion.
#[test]
fn wasm_edge_supports_module_import() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm_edge/reject_module_import.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_test_module_import.wasm");

    let err = compile_wasm_edge(&td_path, &wasm_path);
    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        err.is_none(),
        "wasm-edge should support module imports via inline expansion, got error: {:?}",
        err
    );
}

// ---------------------------------------------------------------------------
// WE-3c: Non-regression
// ---------------------------------------------------------------------------

/// Test: wasm-min still works after wasm-edge additions.
#[test]
fn wasm_edge_does_not_break_wasm_min() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_nonreg_min.wasm");

    let compile = Command::new(taida_bin())
        .args(["build", "wasm-min"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("compile should run");
    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        compile.status.success(),
        "wasm-min should still compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
}

/// Test: wasm-wasi still works after wasm-edge additions.
#[test]
fn wasm_edge_does_not_break_wasm_wasi() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_nonreg_wasi.wasm");

    let compile = Command::new(taida_bin())
        .args(["build", "wasm-wasi"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("compile should run");
    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        compile.status.success(),
        "wasm-wasi should still compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
}

// ---------------------------------------------------------------------------
// WE-2d: JS glue generation tests
// ---------------------------------------------------------------------------

/// Test: wasm-edge build produces a JS glue file alongside the .wasm.
#[test]
fn wasm_edge_generates_js_glue() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_edge_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_glue_test.wasm");
    let glue_path = std::env::temp_dir().join("taida_wasm_edge_glue_test.edge.js");

    // Clean up any previous files
    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&glue_path);

    let output = Command::new(taida_bin())
        .args(["build", "wasm-edge"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("compile should run");

    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Both .wasm and .edge.js should exist
    assert!(wasm_path.exists(), ".wasm file should exist");
    assert!(glue_path.exists(), ".edge.js glue file should exist");

    let glue_content = std::fs::read_to_string(&glue_path).expect("should read glue");

    // Verify key structural elements of the glue
    assert!(
        glue_content.contains("export default"),
        "glue should have Workers export default"
    );
    assert!(
        glue_content.contains("async fetch(request, env, ctx)"),
        "glue should have Workers fetch handler"
    );
    assert!(
        glue_content.contains("export async function handleTaidaRequest(request, env, ctx)"),
        "glue should expose an importable Taida request adapter"
    );
    assert!(
        glue_content.contains("wasi_snapshot_preview1"),
        "glue should provide wasi_snapshot_preview1"
    );
    assert!(
        glue_content.contains("fd_write"),
        "glue should implement fd_write"
    );
    assert!(
        glue_content.contains("taida_host"),
        "glue should provide taida_host module"
    );
    assert!(
        glue_content.contains("env_get"),
        "glue should implement env_get"
    );
    assert!(
        glue_content.contains("env_get_all"),
        "glue should implement env_get_all"
    );
    assert!(
        glue_content.contains("WebAssembly.instantiate"),
        "glue should instantiate the wasm module"
    );
    assert!(
        glue_content.contains("new Response"),
        "glue should return a Response"
    );

    // Verify it references the correct wasm filename
    assert!(
        glue_content.contains("taida_wasm_edge_glue_test.wasm"),
        "glue should reference the correct wasm filename"
    );

    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&glue_path);
}

/// Test: JS glue has valid JS syntax (no obvious template errors).
#[test]
fn wasm_edge_glue_syntax_valid() {
    // Use the public generate function directly to test the template
    // We verify balanced braces and no raw format specifiers remain
    let glue = taida::codegen::driver::generate_edge_js_source("test", "test.wasm");

    // No leftover Rust format specifiers
    assert!(
        !glue.contains("{stem}") && !glue.contains("{wasm_filename}"),
        "glue should not contain raw format specifiers"
    );

    // Balanced braces (basic check)
    let opens: usize = glue.chars().filter(|&c| c == '{').count();
    let closes: usize = glue.chars().filter(|&c| c == '}').count();
    assert_eq!(
        opens, closes,
        "glue should have balanced braces: {} opens, {} closes",
        opens, closes
    );

    // Should start with a comment
    assert!(
        glue.starts_with("//"),
        "glue should start with a JS comment"
    );

    // Node.js syntax check: write to .mjs temp file and run `node --check`
    // This catches real JS errors (e.g., const re-assignment) that brace-counting misses.
    if let Ok(node_output) = Command::new("node").arg("--version").output()
        && node_output.status.success()
    {
        let mjs_path = std::env::temp_dir().join("taida_wasm_edge_glue_syntax_check.mjs");
        std::fs::write(&mjs_path, &glue).expect("should write temp .mjs");

        let check = Command::new("node")
            .arg("--check")
            .arg(&mjs_path)
            .output()
            .expect("node --check should run");
        let _ = std::fs::remove_file(&mjs_path);

        assert!(
            check.status.success(),
            "JS glue has syntax errors (node --check failed): {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

/// Test: current Workers glue mode is explicitly the stdout `_start` adapter.
#[test]
fn wasm_edge_glue_current_mode_is_stdout_start_adapter() {
    let glue = taida::codegen::edge_glue::generate_edge_js_source(
        taida::codegen::edge_glue::EdgeGlueConfig::stdout("test", "test.wasm"),
    );

    assert!(
        glue.contains("instance.exports._start()"),
        "stdout adapter should invoke the wasm _start export"
    );
    assert!(
        !glue.contains("taida_abi_web_handle"),
        "stdout adapter should not pretend to expose the request handler ABI"
    );
}

/// Test: explicit handler mode generates Workers glue for the request ABI.
#[test]
fn wasm_edge_handler_glue_uses_request_abi() {
    let glue = taida::codegen::edge_glue::generate_edge_js_source(
        taida::codegen::edge_glue::EdgeGlueConfig::handler("test", "test.wasm"),
    );

    assert!(
        glue.contains("taida_abi_web_start") && glue.contains("taida_abi_web_poll"),
        "handler adapter should drive the request ABI session loop"
    );
    assert!(
        glue.contains("taida_abi_web_resume"),
        "handler adapter should be prepared to resume host calls"
    );
    assert!(
        glue.contains("bodyBase64"),
        "handler adapter should marshal request and response bodies"
    );
    assert!(
        glue.contains("new Headers"),
        "handler adapter should rebuild response headers"
    );
    assert!(
        !glue.contains("instance.exports._start()"),
        "handler adapter should not execute the stdout _start path"
    );

    if let Ok(node_output) = Command::new("node").arg("--version").output()
        && node_output.status.success()
    {
        let mjs_path = std::env::temp_dir().join("taida_wasm_edge_handler_glue_check.mjs");
        std::fs::write(&mjs_path, &glue).expect("should write handler glue temp .mjs");
        let check = Command::new("node")
            .arg("--check")
            .arg(&mjs_path)
            .output()
            .expect("node --check should run");
        let _ = std::fs::remove_file(&mjs_path);
        assert!(
            check.status.success(),
            "handler JS glue has syntax errors: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

/// Test: host-call dispatch stays mechanical and does not encode capability policy.
#[test]
fn wasm_edge_handler_glue_keeps_host_dispatch_policy_free() {
    let glue = taida::codegen::edge_glue::generate_edge_js_source(
        taida::codegen::edge_glue::EdgeGlueConfig::handler("test", "test.wasm"),
    );
    let dispatch_start = glue
        .find("async function dispatchTaidaHostCall")
        .expect("handler glue should include host-call dispatcher");
    let dispatch_end = glue[dispatch_start..]
        .find("\n}\n\nfunction clampStatus")
        .map(|offset| dispatch_start + offset)
        .expect("dispatcher should end before response helpers");
    let dispatch = &glue[dispatch_start..dispatch_end];

    // Resolution is a fixed table: the well-known ambient capabilities
    // (fetch, crypto), then env binding lookup. Anything beyond name-based
    // resolution (kind switches, schema checks, allow-lists) stays out.
    assert!(
        dispatch.contains("envelope.capability === \"fetch\"")
            && dispatch.contains("envelope.capability === \"crypto\"")
            && dispatch.contains("env[envelope.capability]"),
        "dispatcher should resolve the well-known capabilities and then the binding name"
    );
    assert!(
        dispatch.contains("target = await target[step.method](...step.args);"),
        "dispatcher should mechanically invoke each requested method"
    );
    for forbidden in [
        "envelope.kind",
        "Array.isArray",
        "typeof fn",
        "switch",
        "case ",
        "cloudflare/d1",
        "cloudflare/kv",
        "schema",
        "unsupported host call",
        "host capability unavailable",
        "host method unavailable",
    ] {
        assert!(
            !dispatch.contains(forbidden),
            "dispatcher must not contain adapter policy token `{}`:\n{}",
            forbidden,
            dispatch
        );
    }
}

/// Test: the generated adapter dispatcher turns host-side failures into
/// mechanical resume errors without classifying the capability or method.
#[test]
fn wasm_edge_handler_glue_dispatcher_maps_host_failures_node() {
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("node not found, skipping dispatcher failure mapping test");
        return;
    }

    let glue = taida::codegen::edge_glue::generate_edge_js_source(
        taida::codegen::edge_glue::EdgeGlueConfig::handler("test", "test.wasm"),
    );
    let dispatch_start = glue
        .find("async function dispatchTaidaHostCall")
        .expect("handler glue should include host-call dispatcher");
    let dispatch_end = glue[dispatch_start..]
        .find("\n}\n\nfunction clampStatus")
        .map(|offset| dispatch_start + offset + 3)
        .expect("dispatcher should end before response helpers");
    let dispatch = &glue[dispatch_start..dispatch_end];
    let script = format!(
        r#"
{dispatch}

(async () => {{
  const ok = await dispatchTaidaHostCall({{
    id: 1,
    capability: "CAP",
    steps: [
      {{ method: "first", args: ["k"] }},
      {{ method: "second", args: [] }},
    ],
  }}, {{
    CAP: {{
      first(key) {{ return {{ key, second() {{ return "value"; }} }}; }},
    }},
  }});
  if (!ok.ok || ok.value !== "value") throw new Error("chained dispatch failed: " + JSON.stringify(ok));

  const missing = await dispatchTaidaHostCall({{
    id: 2,
    capability: "CAP",
    steps: [{{ method: "missing", args: [] }}],
  }}, {{ CAP: {{}} }});
  if (missing.ok || !String(missing.error).includes("not a function")) {{
    throw new Error("missing method should become resume error: " + JSON.stringify(missing));
  }}

  const thrown = await dispatchTaidaHostCall({{
    id: 3,
    capability: "CAP",
    steps: [{{ method: "explode", args: [] }}],
  }}, {{ CAP: {{ explode() {{ throw new Error("boom"); }} }} }});
  if (thrown.ok || thrown.error !== "boom") {{
    throw new Error("throw should become resume error: " + JSON.stringify(thrown));
  }}

  const rejected = await dispatchTaidaHostCall({{
    id: 4,
    capability: "CAP",
    steps: [{{ method: "reject", args: [] }}],
  }}, {{ CAP: {{ reject() {{ return Promise.reject(new Error("later")); }} }} }});
  if (rejected.ok || rejected.error !== "later") {{
    throw new Error("promise rejection should become resume error: " + JSON.stringify(rejected));
  }}

  console.log("dispatcher failures mapped");
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    let script_path = std::env::temp_dir().join(format!(
        "taida_wasm_edge_dispatcher_failures_{}.mjs",
        std::process::id()
    ));
    std::fs::write(&script_path, script).expect("write dispatcher failure script");
    let run = Command::new("node")
        .arg(&script_path)
        .output()
        .expect("node dispatcher failure script should run");
    let _ = std::fs::remove_file(&script_path);
    assert!(
        run.status.success(),
        "dispatcher failure script failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("dispatcher failures mapped"),
        "dispatcher failure script did not complete"
    );
}

/// Test: generated glue can be imported from user-authored Workers JS.
#[test]
fn wasm_edge_glue_exposes_importable_request_adapter() {
    let glue = taida::codegen::edge_glue::generate_edge_js_source(
        taida::codegen::edge_glue::EdgeGlueConfig::stdout("test", "test.wasm"),
    );

    assert!(
        glue.contains("export async function handleTaidaRequest(request, env, ctx)"),
        "glue should expose a named adapter for custom JS routing"
    );
    assert!(
        glue.contains("return handleTaidaRequest(request, env, ctx);"),
        "default Workers fetch should delegate to the named adapter"
    );
}

/// Test: wasm-edge env example also produces JS glue.
#[test]
fn wasm_edge_env_generates_js_glue() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_edge_env.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_env_glue_test.wasm");
    let glue_path = std::env::temp_dir().join("taida_wasm_edge_env_glue_test.edge.js");

    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&glue_path);

    let output = Command::new(taida_bin())
        .args(["build", "wasm-edge"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("compile should run");

    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        glue_path.exists(),
        ".edge.js glue file should exist for env example"
    );

    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&glue_path);
}

/// Test: handler mode builds a wasm module and a handler-specific glue file.
#[test]
fn wasm_edge_handler_build_generates_handler_glue() {
    let stem = format!("taida_wasm_edge_handler_glue_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("{}.wasm", stem));
    let glue_path = std::env::temp_dir().join(format!("{}.edge.js", stem));
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)

handle req: WebRequest = text(req.method + ":" + req.path) => :WebResponse
"#;

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&glue_path);
    std::fs::write(&td_path, source).expect("write handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(err.is_none(), "handler build should compile: {:?}", err);
    assert!(wasm_path.exists(), ".wasm file should exist");
    assert!(glue_path.exists(), ".edge.js glue file should exist");

    let glue_content = std::fs::read_to_string(&glue_path).expect("should read handler glue");
    assert!(
        glue_content.contains("taida_abi_web_start") && glue_content.contains("taida_abi_web_poll"),
        "handler glue should drive the wasm handler session loop"
    );
    assert!(
        !glue_content.contains("instance.exports._start()"),
        "handler glue should not execute the stdout adapter"
    );

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&glue_path);
}

/// Test: handler mode rejects mismatched WebRequest/WebResponse annotations.
#[test]
fn wasm_edge_handler_rejects_bad_signature() {
    let stem = format!("taida_wasm_edge_handler_bad_sig_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("{}.wasm", stem));
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)

handle req: Str = text(req) => :WebResponse
"#;

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
    std::fs::write(&td_path, source).expect("write bad handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle")
        .expect("bad handler signature should fail");
    assert!(
        err.contains("[E1961]") && err.contains("WebRequest"),
        "handler signature diagnostic should mention E1961 and WebRequest, got: {}",
        err
    );

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
}

/// Test: wasm-edge build reads Cloudflare bindings from wrangler JSONC before type checking.
#[test]
fn wasm_edge_handler_injects_wrangler_host_capability_manifest() {
    let dir = unique_temp_dir("taida_wasm_edge_wrangler_manifest");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCapability)

D1Database <= "cloudflare/d1"

handle req: WebRequest =
  db <= HostCapability["DB", D1Database]()
  text(req.path)
=> :WebResponse
"#;
    let wrangler = r#"
{
  // This manifest is active, but it does not declare DB as a D1 binding.
  "kv_namespaces": [
    { "binding": "CACHE" },
  ],
}
"#;

    std::fs::write(dir.join("wrangler.jsonc"), wrangler).expect("write wrangler manifest");
    std::fs::write(&td_path, source).expect("write handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle")
        .expect("undeclared HostCapability should fail before codegen");
    assert!(
        err.contains("[E3603]") && err.contains("DB") && err.contains("cloudflare/d1"),
        "diagnostic should report the missing wrangler capability, got: {}",
        err
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Test: handler mode rejects missing symbols, wrong arity, and bad returns.
#[test]
fn wasm_edge_handler_rejects_entry_shape_errors() {
    let cases = [
        (
            "missing",
            "missing",
            r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)

handle req: WebRequest = text("ok") => :WebResponse
"#,
            "was not found",
        ),
        (
            "arity",
            "handle",
            r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)

handle req: WebRequest other: WebRequest = text("ok") => :WebResponse
"#,
            "exactly one WebRequest parameter",
        ),
        (
            "return",
            "handle",
            r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)

handle req: WebRequest = "ok" => :Str
"#,
            "WebResponse",
        ),
    ];

    for (idx, (name, handler, source, expected)) in cases.iter().enumerate() {
        let stem = format!(
            "taida_wasm_edge_handler_shape_{}_{}_{}",
            name,
            idx,
            std::process::id()
        );
        let td_path = std::env::temp_dir().join(format!("{}.td", stem));
        let wasm_path = std::env::temp_dir().join(format!("{}.wasm", stem));
        let _ = std::fs::remove_file(&td_path);
        let _ = std::fs::remove_file(&wasm_path);
        std::fs::write(&td_path, source).expect("write handler shape fixture");

        let err = compile_wasm_edge_handler(&td_path, &wasm_path, handler)
            .expect("invalid handler shape should fail");
        assert!(
            err.contains("[E1961]") && err.contains(expected),
            "{} handler diagnostic should mention E1961 and {:?}, got: {}",
            name,
            expected,
            err
        );

        let _ = std::fs::remove_file(&td_path);
        let _ = std::fs::remove_file(&wasm_path);
    }
}

/// Test: the exported handler ABI can be invoked from a JS host.
#[test]
fn wasm_edge_handler_roundtrip_node() {
    if !Command::new("node")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
    {
        eprintln!("node not found, skipping wasm-edge handler ABI runtime test");
        return;
    }

    let stem = format!("taida_wasm_edge_handler_node_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("{}.wasm", stem));
    let js_path = std::env::temp_dir().join(format!("{}.js", stem));
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)

handle req: WebRequest = text(req.method + ":" + req.path) => :WebResponse
"#;

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&js_path);
    std::fs::write(&td_path, source).expect("write handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(err.is_none(), "handler build should compile: {:?}", err);

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  if (typeof instance.exports.taida_abi_web_start !== "function") throw new Error("missing start export");
  if (typeof instance.exports.taida_abi_web_poll !== "function") throw new Error("missing poll export");
  if (typeof instance.exports.taida_abi_web_resume !== "function") throw new Error("missing resume export");
  const payload = encoder.encode(JSON.stringify({{
    method: "POST",
    path: "/node",
    rawQuery: "",
    query: [],
    headers: [],
    bodyBase64: "",
  }}));
  const inPtr = instance.exports.taida_abi_web_alloc(payload.length);
  new Uint8Array(memory.buffer, inPtr, payload.length).set(payload);
  const handle = instance.exports.taida_abi_web_start(inPtr, payload.length);
  const poll = instance.exports.taida_abi_web_poll(handle);
  if (poll !== 0) throw new Error("unexpected poll state " + poll);
  const outPtr = instance.exports.taida_abi_web_out_ptr(handle);
  const outLen = instance.exports.taida_abi_web_out_len(handle);
  const raw = decoder.decode(new Uint8Array(memory.buffer, outPtr, outLen));
  instance.exports.taida_abi_web_free(handle);
  console.log(raw);
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node handler harness should run");
    assert!(
        run.status.success(),
        "node handler harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains(r#""status":200"#) && stdout.contains(r#""bodyBase64":"UE9TVDovbm9kZQ==""#),
        "handler ABI output should encode 200 response with request fields, got: {}",
        stdout
    );

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&js_path);
}

/// Test: handler request bodies are valid `Bytes` inputs for the wasm-edge
/// crypto subset, including embedded NUL bytes.
#[test]
fn wasm_edge_handler_crypto_sha256_body_bytes_node() {
    if !Command::new("node")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
    {
        eprintln!("node not found, skipping wasm-edge handler crypto runtime test");
        return;
    }

    let stem = format!("taida_wasm_edge_handler_crypto_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("{}.wasm", stem));
    let js_path = std::env::temp_dir().join(format!("{}.js", stem));
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)
>>> taida-lang/crypto => @(sha256)

handle req: WebRequest = text(sha256(req.body)) => :WebResponse
"#;

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&js_path);
    std::fs::write(&td_path, source).expect("write handler crypto fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "handler crypto build should compile: {:?}",
        err
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");
const nodeCrypto = require("crypto");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  function digest(bytes) {{
    return nodeCrypto.createHash("sha256").update(bytes).digest("hex");
  }}
  function invoke(bodyBase64) {{
    const request = encoder.encode(JSON.stringify({{
      method: "POST",
      path: "/digest",
      rawQuery: "",
      query: [],
      headers: [],
      bodyBase64,
    }}));
    const inPtr = instance.exports.taida_abi_web_alloc(request.length);
    if (!inPtr) throw new Error("alloc failed for request length " + request.length);
    new Uint8Array(memory.buffer, inPtr, request.length).set(request);
    const handle = instance.exports.taida_abi_web_start(inPtr, request.length);
    if (instance.exports.taida_abi_web_poll(handle) !== 0) throw new Error("expected ready response");
    const raw = decoder.decode(new Uint8Array(
      memory.buffer,
      instance.exports.taida_abi_web_out_ptr(handle),
      instance.exports.taida_abi_web_out_len(handle)
    ));
    instance.exports.taida_abi_web_free(handle);
    return JSON.parse(raw);
  }}
  const vectors = [
    Buffer.alloc(0),
    Buffer.from([0x41, 0x00, 0x42]),
    Buffer.from([0x00, 0x00, 0x00, 0x00]),
  ];
  for (const bytes of vectors) {{
    const response = invoke(bytes.toString("base64"));
    const body = Buffer.from(response.bodyBase64 || "", "base64").toString("utf8");
    const expected = digest(bytes);
    if (response.status !== 200 || body !== expected) {{
      throw new Error("digest mismatch: got " + JSON.stringify(response) + " expected " + expected);
    }}
  }}
  console.log("ok");
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write handler crypto node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node handler crypto harness should run");
    assert!(
        run.status.success(),
        "node handler crypto harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();
    assert_eq!(stdout, "ok", "handler crypto digest vectors failed");

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);
    let _ = std::fs::remove_file(&js_path);
}

/// Test: one HostCall envelope can suspend a handler and resume with a value.
#[test]
fn wasm_edge_handler_host_call_poll_resume_node() {
    if !Command::new("node")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
    {
        eprintln!("node not found, skipping wasm-edge HostCall ABI runtime test");
        return;
    }

    let dir = unique_temp_dir("taida_wasm_edge_host_call_resume");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let js_path = dir.join("host_call.js");
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCall, HostStep, HostCapability)

KV <= "cloudflare/kv"

handle req: WebRequest =
  cache <= HostCapability["CACHE", KV]()
  value <=< Cage[cache, HostCall[@[HostStep["get", @["answer"]]()], Str]()]()
  text(value)
=> :WebResponse
"#;
    let wrangler = r#"{ "kv_namespaces": [{ "binding": "CACHE" }] }"#;

    std::fs::write(dir.join("wrangler.jsonc"), wrangler).expect("write wrangler manifest");
    std::fs::write(&td_path, source).expect("write HostCall handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "HostCall handler build should compile: {:?}",
        err
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const request = encoder.encode(JSON.stringify({{
    method: "GET",
    path: "/host",
    rawQuery: "",
    query: [],
    headers: [],
    bodyBase64: "",
  }}));
  const reqPtr = instance.exports.taida_abi_web_alloc(request.length);
  new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
  const handle = instance.exports.taida_abi_web_start(reqPtr, request.length);
  const firstPoll = instance.exports.taida_abi_web_poll(handle);
  if (firstPoll !== 1) throw new Error("expected host_call_pending, got " + firstPoll);
  const pendingPtr = instance.exports.taida_abi_web_out_ptr(handle);
  const pendingLen = instance.exports.taida_abi_web_out_len(handle);
  const pending = JSON.parse(decoder.decode(new Uint8Array(memory.buffer, pendingPtr, pendingLen)));
  if (pending.kind !== "host_call") throw new Error("bad host call kind");
  if (pending.capability !== "CACHE") throw new Error("bad capability " + pending.capability);
  if (pending.steps.length !== 1) throw new Error("bad step count");
  if (pending.steps[0].method !== "get") throw new Error("bad method");
  if (pending.steps[0].args[0] !== "answer") throw new Error("bad args");

  const resume = encoder.encode(JSON.stringify({{ id: pending.id, ok: true, value: "hit" }}));
  const resumePtr = instance.exports.taida_abi_web_alloc(resume.length);
  new Uint8Array(memory.buffer, resumePtr, resume.length).set(resume);
  instance.exports.taida_abi_web_resume(handle, resumePtr, resume.length);
  const secondPoll = instance.exports.taida_abi_web_poll(handle);
  if (secondPoll !== 0) throw new Error("expected response_ready, got " + secondPoll);
  const outPtr = instance.exports.taida_abi_web_out_ptr(handle);
  const outLen = instance.exports.taida_abi_web_out_len(handle);
  const raw = decoder.decode(new Uint8Array(memory.buffer, outPtr, outLen));
  instance.exports.taida_abi_web_free(handle);
  console.log(raw);
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write HostCall node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node HostCall handler harness should run");
    assert!(
        run.status.success(),
        "node HostCall harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains(r#""status":200"#) && stdout.contains(r#""bodyBase64":"aGl0""#),
        "HostCall resume should produce response body, got: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Test: a Cage reached on the dead path after a pending suspend must be
/// inert. The wasm error model is flag-based (a throw does not unwind), so
/// the statements after the first suspending Cage still execute with dummy
/// values; before the dead-path guard the second Cage overwrote the pending
/// envelope (the host saw the WRONG call) and consumed replay payloads out
/// of order (re-running non-idempotent host calls). Two sequential
/// awaiting-helper calls are the minimal trigger.
#[test]
fn wasm_edge_handler_dead_path_cage_keeps_first_pending_node() {
    if !Command::new("node")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
    {
        eprintln!("node not found, skipping wasm-edge dead-path Cage runtime test");
        return;
    }

    let dir = unique_temp_dir("taida_wasm_edge_dead_path_cage");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let js_path = dir.join("dead_path.js");
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCall, HostStep, HostCapability)

KV <= "cloudflare/kv"

getOne key: Str =
  cache <= HostCapability["CACHE", KV]()
  value <=< Cage[cache, HostCall[@[HostStep["get", @[key]]()], Str]()]()
  value
=> :Str

handle req: WebRequest =
  a <= getOne("first")
  b <= getOne("second")
  text(a + "/" + b)
=> :WebResponse
"#;
    let wrangler = r#"{ "kv_namespaces": [{ "binding": "CACHE" }] }"#;

    std::fs::write(dir.join("wrangler.jsonc"), wrangler).expect("write wrangler manifest");
    std::fs::write(&td_path, source).expect("write dead-path handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "dead-path handler build should compile: {:?}",
        err
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const request = encoder.encode(JSON.stringify({{
    method: "GET",
    path: "/dead-path",
    rawQuery: "",
    query: [],
    headers: [],
    bodyBase64: "",
  }}));
  const reqPtr = instance.exports.taida_abi_web_alloc(request.length);
  new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
  const handle = instance.exports.taida_abi_web_start(reqPtr, request.length);
  const seen = [];
  const answers = {{ first: "A", second: "B" }};
  for (let i = 0; i < 8; i++) {{
    const state = instance.exports.taida_abi_web_poll(handle);
    if (state === 0) {{
      const outPtr = instance.exports.taida_abi_web_out_ptr(handle);
      const outLen = instance.exports.taida_abi_web_out_len(handle);
      const raw = decoder.decode(new Uint8Array(memory.buffer, outPtr, outLen));
      instance.exports.taida_abi_web_free(handle);
      console.log(JSON.stringify({{ seen, raw: JSON.parse(raw) }}));
      return;
    }}
    if (state !== 1) throw new Error("unexpected poll state " + state);
    const pendingPtr = instance.exports.taida_abi_web_out_ptr(handle);
    const pendingLen = instance.exports.taida_abi_web_out_len(handle);
    const pending = JSON.parse(decoder.decode(new Uint8Array(memory.buffer, pendingPtr, pendingLen)));
    const key = pending.steps[0].args[0];
    seen.push(key);
    if (!(key in answers)) throw new Error("unmocked key " + key);
    const resume = encoder.encode(JSON.stringify({{ id: pending.id, ok: true, value: answers[key] }}));
    const resumePtr = instance.exports.taida_abi_web_alloc(resume.length);
    new Uint8Array(memory.buffer, resumePtr, resume.length).set(resume);
    instance.exports.taida_abi_web_resume(handle, resumePtr, resume.length);
  }}
  throw new Error("poll limit exceeded");
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write dead-path node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node dead-path harness should run");
    assert!(
        run.status.success(),
        "node dead-path harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("harness should print JSON");
    assert_eq!(
        parsed["seen"],
        serde_json::json!(["first", "second"]),
        "the FIRST pending envelope must reach the host first (no dead-path overwrite), got: {}",
        stdout
    );
    assert_eq!(
        parsed["raw"]["status"], 200,
        "handler should complete after both resumes, got: {}",
        stdout
    );
    // "A/B" — each call got its own recorded payload, in order.
    assert_eq!(
        parsed["raw"]["bodyBase64"], "QS9C",
        "replay payloads must map to their own calls, got: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Test: HostCall encodes Bytes arguments with the same base64 wire rule as the
/// request/response ABI instead of exposing the internal list-of-Int layout.
#[test]
fn wasm_edge_handler_host_call_bytes_arg_is_base64_node() {
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("node not found, skipping wasm-edge HostCall Bytes ABI runtime test");
        return;
    }

    let dir = unique_temp_dir("taida_wasm_edge_host_call_bytes_arg");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let js_path = dir.join("host_call_bytes.js");
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCall, HostStep, HostCapability)

KV <= "cloudflare/kv"

handle req: WebRequest =
  cache <= HostCapability["CACHE", KV]()
  value <=< Cage[cache, HostCall[@[HostStep["put", @[req.body]]()], Str]()]()
  text(value)
=> :WebResponse
"#;
    let wrangler = r#"{ "kv_namespaces": [{ "binding": "CACHE" }] }"#;

    std::fs::write(dir.join("wrangler.jsonc"), wrangler).expect("write wrangler manifest");
    std::fs::write(&td_path, source).expect("write HostCall Bytes handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "HostCall Bytes handler build should compile: {:?}",
        err
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const request = encoder.encode(JSON.stringify({{
    method: "POST",
    path: "/host",
    rawQuery: "",
    query: [],
    headers: [],
    bodyBase64: "QQBC",
  }}));
  const reqPtr = instance.exports.taida_abi_web_alloc(request.length);
  new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
  const handle = instance.exports.taida_abi_web_start(reqPtr, request.length);
  if (instance.exports.taida_abi_web_poll(handle) !== 1) throw new Error("expected host_call_pending");
  const pendingPtr = instance.exports.taida_abi_web_out_ptr(handle);
  const pendingLen = instance.exports.taida_abi_web_out_len(handle);
  const pending = JSON.parse(decoder.decode(new Uint8Array(memory.buffer, pendingPtr, pendingLen)));
  if (pending.steps[0].args[0] !== "QQBC") {{
    throw new Error("Bytes arg was not base64: " + JSON.stringify(pending.steps[0].args[0]));
  }}

  const resume = encoder.encode(JSON.stringify({{ id: pending.id, ok: true, value: "stored" }}));
  const resumePtr = instance.exports.taida_abi_web_alloc(resume.length);
  new Uint8Array(memory.buffer, resumePtr, resume.length).set(resume);
  instance.exports.taida_abi_web_resume(handle, resumePtr, resume.length);
  const raw = decoder.decode(new Uint8Array(
    memory.buffer,
    instance.exports.taida_abi_web_out_ptr(handle),
    instance.exports.taida_abi_web_out_len(handle)
  ));
  instance.exports.taida_abi_web_free(handle);
  console.log(raw);
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write HostCall Bytes node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node HostCall Bytes handler harness should run");
    assert!(
        run.status.success(),
        "node HostCall Bytes harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains(r#""status":200"#) && stdout.contains(r#""bodyBase64":"c3RvcmVk""#),
        "HostCall Bytes resume should produce response body, got: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Test: invalid base64 in a HostCall `Bytes` resume value rejects the Async
/// rather than silently decoding to an empty byte sequence.
#[test]
fn wasm_edge_handler_host_call_invalid_bytes_resume_rejects_node() {
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("node not found, skipping wasm-edge HostCall Bytes reject runtime test");
        return;
    }

    let dir = unique_temp_dir("taida_wasm_edge_host_call_invalid_bytes");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let js_path = dir.join("host_call_invalid_bytes.js");
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCall, HostStep, HostCapability)

KV <= "cloudflare/kv"

handle req: WebRequest =
  |== error: Error =
    text("caught:" + error.message)
  => :WebResponse
  cache <= HostCapability["CACHE", KV]()
  value <=< Cage[cache, HostCall[@[HostStep["getBytes", @["key"]]()], Bytes]()]()
  text("uncaught")
=> :WebResponse
"#;
    let wrangler = r#"{ "kv_namespaces": [{ "binding": "CACHE" }] }"#;

    std::fs::write(dir.join("wrangler.jsonc"), wrangler).expect("write wrangler manifest");
    std::fs::write(&td_path, source).expect("write invalid Bytes handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "HostCall invalid Bytes handler build should compile: {:?}",
        err
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  async function assertRejected(value) {{
    const request = encoder.encode(JSON.stringify({{
      method: "GET",
      path: "/host",
      rawQuery: "",
      query: [],
      headers: [],
      bodyBase64: "",
    }}));
    const reqPtr = instance.exports.taida_abi_web_alloc(request.length);
    new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
    const handle = instance.exports.taida_abi_web_start(reqPtr, request.length);
    if (instance.exports.taida_abi_web_poll(handle) !== 1) throw new Error("expected host_call_pending");
    const pending = JSON.parse(decoder.decode(new Uint8Array(
      memory.buffer,
      instance.exports.taida_abi_web_out_ptr(handle),
      instance.exports.taida_abi_web_out_len(handle)
    )));
    const resume = encoder.encode(JSON.stringify({{ id: pending.id, ok: true, value }}));
    const resumePtr = instance.exports.taida_abi_web_alloc(resume.length);
    new Uint8Array(memory.buffer, resumePtr, resume.length).set(resume);
    instance.exports.taida_abi_web_resume(handle, resumePtr, resume.length);
    if (instance.exports.taida_abi_web_poll(handle) !== 0) throw new Error("expected response after rejected Async");
    const raw = decoder.decode(new Uint8Array(
      memory.buffer,
      instance.exports.taida_abi_web_out_ptr(handle),
      instance.exports.taida_abi_web_out_len(handle)
    ));
    instance.exports.taida_abi_web_free(handle);
    const response = JSON.parse(raw);
    const body = Buffer.from(response.bodyBase64 || "", "base64").toString("utf8");
    if (response.status !== 200 || !body.includes("caught:")) {{
      throw new Error("invalid base64 should reject into |==, got " + raw + " body=" + body);
    }}
  }}

  await assertRejected("A");
  await assertRejected("@@@=");
  console.log("rejected");
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write invalid Bytes node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node invalid Bytes handler harness should run");
    assert!(
        run.status.success(),
        "node invalid Bytes harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("rejected"),
        "invalid Bytes harness did not complete"
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Test: adapter-provided `ok:false` resume payloads become rejected Async
/// values that user code can catch with an error ceiling.
#[test]
fn wasm_edge_handler_host_call_resume_error_is_catchable_node() {
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("node not found, skipping wasm-edge HostCall resume error test");
        return;
    }

    let dir = unique_temp_dir("taida_wasm_edge_host_call_resume_error");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let js_path = dir.join("host_call_resume_error.js");
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCall, HostStep, HostCapability)

KV <= "cloudflare/kv"

handle req: WebRequest =
  |== error: Error =
    text("caught:" + error.message)
  => :WebResponse
  cache <= HostCapability["CACHE", KV]()
  value <=< Cage[cache, HostCall[@[HostStep["get", @["answer"]]()], Str]()]()
  text(value)
=> :WebResponse
"#;
    let wrangler = r#"{ "kv_namespaces": [{ "binding": "CACHE" }] }"#;

    std::fs::write(dir.join("wrangler.jsonc"), wrangler).expect("write wrangler manifest");
    std::fs::write(&td_path, source).expect("write resume error handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "HostCall resume error handler should compile: {:?}",
        err
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const request = encoder.encode(JSON.stringify({{
    method: "GET",
    path: "/host",
    rawQuery: "",
    query: [],
    headers: [],
    bodyBase64: "",
  }}));
  const reqPtr = instance.exports.taida_abi_web_alloc(request.length);
  new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
  const handle = instance.exports.taida_abi_web_start(reqPtr, request.length);
  if (instance.exports.taida_abi_web_poll(handle) !== 1) throw new Error("expected host_call_pending");
  const pending = JSON.parse(decoder.decode(new Uint8Array(
    memory.buffer,
    instance.exports.taida_abi_web_out_ptr(handle),
    instance.exports.taida_abi_web_out_len(handle)
  )));
  const resume = encoder.encode(JSON.stringify({{ id: pending.id, ok: false, error: "host boom" }}));
  const resumePtr = instance.exports.taida_abi_web_alloc(resume.length);
  new Uint8Array(memory.buffer, resumePtr, resume.length).set(resume);
  instance.exports.taida_abi_web_resume(handle, resumePtr, resume.length);
  if (instance.exports.taida_abi_web_poll(handle) !== 0) throw new Error("expected response_ready");
  const raw = decoder.decode(new Uint8Array(
    memory.buffer,
    instance.exports.taida_abi_web_out_ptr(handle),
    instance.exports.taida_abi_web_out_len(handle)
  ));
  instance.exports.taida_abi_web_free(handle);
  const response = JSON.parse(raw);
  const body = Buffer.from(response.bodyBase64 || "", "base64").toString("utf8");
  if (response.status !== 200 || !body.includes("caught:host boom")) {{
    throw new Error("resume error should be caught, got " + raw + " body=" + body);
  }}
  console.log("caught");
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write resume error node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node resume error harness should run");
    assert!(
        run.status.success(),
        "node resume error harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("caught"),
        "resume error harness did not complete"
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Test: a D1-style multi-step HostCall can suspend once and resume with a
/// typed row decoded by the guest runtime.
#[test]
fn wasm_edge_handler_host_call_d1_mock_node() {
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("node not found, skipping wasm-edge D1 HostCall runtime test");
        return;
    }

    let dir = unique_temp_dir("taida_wasm_edge_host_call_d1");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let js_path = dir.join("host_call_d1.js");
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCall, HostStep, HostCapability)

Row = @(name: Str)
D1 <= "cloudflare/d1"

handle req: WebRequest =
  db <= HostCapability["DB", D1]()
  row <=< Cage[db, HostCall[@[HostStep["prepare", @["select name from users where id = ?"]](), HostStep["bind", @["42"]](), HostStep["first", @[]]()], Row]()]()
  text(row.name)
=> :WebResponse
"#;
    let wrangler = r#"
{
  "d1_databases": [
    { "binding": "DB", "database_name": "test", "database_id": "local" }
  ]
}
"#;

    std::fs::write(dir.join("wrangler.jsonc"), wrangler).expect("write wrangler manifest");
    std::fs::write(&td_path, source).expect("write D1 HostCall handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "D1 HostCall handler should compile: {:?}",
        err
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const request = encoder.encode(JSON.stringify({{
    method: "GET",
    path: "/user",
    rawQuery: "",
    query: [],
    headers: [],
    bodyBase64: "",
  }}));
  const reqPtr = instance.exports.taida_abi_web_alloc(request.length);
  new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
  const handle = instance.exports.taida_abi_web_start(reqPtr, request.length);
  if (instance.exports.taida_abi_web_poll(handle) !== 1) throw new Error("expected host_call_pending");
  const pending = JSON.parse(decoder.decode(new Uint8Array(
    memory.buffer,
    instance.exports.taida_abi_web_out_ptr(handle),
    instance.exports.taida_abi_web_out_len(handle)
  )));
  if (pending.capability !== "DB") throw new Error("bad capability " + pending.capability);
  if (pending.steps.length !== 3) throw new Error("bad step count " + pending.steps.length);
  if (pending.steps[0].method !== "prepare") throw new Error("missing prepare");
  if (pending.steps[1].method !== "bind") throw new Error("missing bind");
  if (pending.steps[2].method !== "first") throw new Error("missing first");
  if (pending.steps[1].args[0] !== "42") throw new Error("bad bind arg");

  const resume = encoder.encode(JSON.stringify({{ id: pending.id, ok: true, value: {{ name: "Ada" }} }}));
  const resumePtr = instance.exports.taida_abi_web_alloc(resume.length);
  new Uint8Array(memory.buffer, resumePtr, resume.length).set(resume);
  instance.exports.taida_abi_web_resume(handle, resumePtr, resume.length);
  if (instance.exports.taida_abi_web_poll(handle) !== 0) throw new Error("expected response_ready");
  const raw = decoder.decode(new Uint8Array(
    memory.buffer,
    instance.exports.taida_abi_web_out_ptr(handle),
    instance.exports.taida_abi_web_out_len(handle)
  ));
  instance.exports.taida_abi_web_free(handle);
  console.log(raw);
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write D1 node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node D1 HostCall harness should run");
    assert!(
        run.status.success(),
        "node D1 HostCall harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains(r#""status":200"#) && stdout.contains(r#""bodyBase64":"QWRh""#),
        "D1 HostCall resume should produce row response, got: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Test: a Durable Object style HostCall can pass the full WebRequest as an
/// argument and resume with a WebResponse decoded back into the ABI shape.
#[test]
fn wasm_edge_handler_host_call_do_mock_webrequest_webresponse_node() {
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("node not found, skipping wasm-edge DO HostCall runtime test");
        return;
    }

    let dir = unique_temp_dir("taida_wasm_edge_host_call_do");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let js_path = dir.join("host_call_do.js");
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, HostCall, HostStep, HostCapability)

DONamespace <= "cloudflare/do_namespace"

handle req: WebRequest =
  ns <= HostCapability["COUNTER", DONamespace]()
  resp <=< Cage[ns, HostCall[@[HostStep["idFromName", @["main"]](), HostStep["get", @[]](), HostStep["fetch", @[req]]()], WebResponse]()]()
  resp
=> :WebResponse
"#;
    let wrangler = r#"
{
  "durable_objects": {
    "bindings": [
      { "name": "COUNTER", "class_name": "Counter" }
    ]
  }
}
"#;

    std::fs::write(dir.join("wrangler.jsonc"), wrangler).expect("write wrangler manifest");
    std::fs::write(&td_path, source).expect("write DO HostCall handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "DO HostCall handler should compile: {:?}",
        err
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const request = encoder.encode(JSON.stringify({{
    method: "POST",
    path: "/counter",
    rawQuery: "tag=a&tag=b",
    query: [{{ name: "tag", value: "a" }}, {{ name: "tag", value: "b" }}],
    headers: [{{ name: "x-repeat", value: "one" }}],
    bodyBase64: "ZG8=",
  }}));
  const reqPtr = instance.exports.taida_abi_web_alloc(request.length);
  new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
  const handle = instance.exports.taida_abi_web_start(reqPtr, request.length);
  if (instance.exports.taida_abi_web_poll(handle) !== 1) throw new Error("expected host_call_pending");
  const pending = JSON.parse(decoder.decode(new Uint8Array(
    memory.buffer,
    instance.exports.taida_abi_web_out_ptr(handle),
    instance.exports.taida_abi_web_out_len(handle)
  )));
  if (pending.capability !== "COUNTER") throw new Error("bad capability " + pending.capability);
  if (pending.steps.length !== 3) throw new Error("bad step count " + pending.steps.length);
  if (pending.steps[0].method !== "idFromName") throw new Error("missing idFromName");
  if (pending.steps[1].method !== "get") throw new Error("missing get");
  if (pending.steps[2].method !== "fetch") throw new Error("missing fetch");
  const reqArg = pending.steps[2].args[0];
  if (reqArg.method !== "POST" || reqArg.path !== "/counter") throw new Error("bad WebRequest arg");
  if (reqArg.bodyBase64 !== "ZG8=") throw new Error("bad WebRequest body");
  if (reqArg.query.length !== 2 || reqArg.query[1].value !== "b") throw new Error("bad WebRequest query pairs");

  const resume = encoder.encode(JSON.stringify({{
    id: pending.id,
    ok: true,
    value: {{
      status: 202,
      headers: [{{ name: "x-do", value: "ok" }}],
      bodyBase64: "ZG9uZQ==",
    }},
  }}));
  const resumePtr = instance.exports.taida_abi_web_alloc(resume.length);
  new Uint8Array(memory.buffer, resumePtr, resume.length).set(resume);
  instance.exports.taida_abi_web_resume(handle, resumePtr, resume.length);
  if (instance.exports.taida_abi_web_poll(handle) !== 0) throw new Error("expected response_ready");
  const raw = decoder.decode(new Uint8Array(
    memory.buffer,
    instance.exports.taida_abi_web_out_ptr(handle),
    instance.exports.taida_abi_web_out_len(handle)
  ));
  instance.exports.taida_abi_web_free(handle);
  console.log(raw);
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write DO node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node DO HostCall harness should run");
    assert!(
        run.status.success(),
        "node DO HostCall harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains(r#""status":202"#)
            && stdout.contains(r#""name":"x-do""#)
            && stdout.contains(r#""bodyBase64":"ZG9uZQ==""#),
        "DO HostCall resume should produce WebResponse, got: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Test: wasm-edge binary size is bounded.
#[test]
fn wasm_edge_size_check() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_edge_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_size_hello.wasm");

    let compile = Command::new(taida_bin())
        .args(["build", "wasm-edge"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("compile should run");
    assert!(
        compile.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let size = std::fs::metadata(&wasm_path).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&wasm_path);

    eprintln!("WE-3: wasm-edge hello size = {} bytes", size);

    // C12-7 (FB-26): Restored from 16KB -> 4KB. With the B11-2f fix that
    // removed `taida_polymorphic_to_string` from the `taida_io_stdout_with_tag`
    // reference chain, the codegen lightweight path for `stdout("literal")`
    // links only `taida_io_stdout` + `_start` + `write_stdout` + `wasm_strlen`,
    // producing ~351B. The 4KB budget keeps plenty of headroom for static
    // ASCII payload growth while ensuring the hello-world never regresses
    // back into the polymorphic display chain.
    assert!(
        size <= 4096,
        "wasm-edge hello should be <= 4KB, got {} bytes",
        size
    );
}

/// wasm-edge hello world must remain in the stdout("literal")
/// lightweight path (no `taida_io_stdout_with_tag` / no polymorphic_to_string
/// DCE chain). We anchor the observed size to a tight 1KB upper bound: the
/// current output is ~351B and any regression into the tagged runtime path
/// would blow past 1KB. Keep this separate from `wasm_edge_size_check` so
/// the 4KB hard gate remains comfortable for future ASCII payload growth
/// while this regression gate catches unintended runtime-link expansion.
#[test]
fn wasm_edge_hello_no_polymorphic_regression() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_edge_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_edge_c12_7_no_poly_regression.wasm");

    let compile = Command::new(taida_bin())
        .args(["build", "wasm-edge"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("compile should run");
    assert!(
        compile.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let size = std::fs::metadata(&wasm_path).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&wasm_path);

    eprintln!(
        "C12-7: wasm-edge hello (lightweight stdout path) = {} bytes",
        size
    );

    assert!(
        size <= 1024,
        "C12-7 regression: wasm-edge hello (stdout(\"literal\")) linked a heavy \
         path. Expected <= 1KB for the DCE-eliminated tagged runtime chain, \
         got {} bytes. If `taida_io_stdout_with_tag` is now on the reference \
         chain for a static-string stdout call, the B11-2f fix has regressed.",
        size
    );
}

// ── C12B-023: Regex on wasm-edge must produce compile error ──────────
//
// PHILOSOPHY I (silent-undefined 禁止): wasm-edge shares the
// runtime_core_wasm Regex stubs with min / wasi / full, so the same
// compile-time reject path must fire.

fn assert_edge_regex_rejected(stem: &str, source: &str, candidates: &[&str]) {
    let td_path = std::env::temp_dir().join(format!("taida_c12b_023_edge_{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("taida_c12b_023_edge_{}.wasm", stem));
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("wasm-edge")
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida build");

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "C12B-023: wasm-edge should reject Regex usage, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("[E1617]"),
        "C12B-023: wasm-edge Regex rejection must emit [E1617], got: {}",
        stderr
    );
    assert!(
        candidates.iter().any(|l| stderr.contains(l)),
        "C12B-023: wasm-edge [E1617] message should mention one of {:?}, got: {}",
        candidates,
        stderr
    );
}

#[test]
fn test_c12b_023_wasm_edge_rejects_regex_ctor() {
    assert_edge_regex_rejected(
        "ctor",
        "re <= Regex(\"\\\\d+\", \"\")\nstdout(\"built\")\n",
        &["Regex"],
    );
}

#[test]
fn test_c12b_023_wasm_edge_rejects_str_match() {
    assert_edge_regex_rejected(
        "match",
        "re <= Regex(\"\\\\d+\", \"\")\ns <= \"abc 123\"\nresult <= s.match(re)\nstdout(result)\n",
        &["Regex", "Str.match"],
    );
}

#[test]
fn test_c12b_023_wasm_edge_rejects_str_search() {
    assert_edge_regex_rejected(
        "search",
        "re <= Regex(\"\\\\d+\", \"\")\ns <= \"abc 123\"\ni <= s.search(re)\nstdout(i)\n",
        &["Regex", "Str.search"],
    );
}

// ── C12B-023 bypass closure (2026-04-15 external review fix) ─────────
#[test]
fn test_c12b_023_wasm_edge_rejects_manual_pack_replaceall() {
    assert_edge_regex_rejected(
        "bypass_replaceall",
        "main =\n  re <= @(__type <= \"Regex\", pattern <= \"a\", flags <= \"\")\n  stdout(\"aba\".replaceAll(re, \"x\"))\n=> :Str\n",
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_edge_rejects_manual_pack_match() {
    assert_edge_regex_rejected(
        "bypass_match",
        "re <= @(__type <= \"Regex\", pattern <= \"a\", flags <= \"\")\nstdout(\"abc\".match(re))\n",
        &["reserved for compiler-internal use"],
    );
}

// C12B-023 root fix (2026-04-15 v2): indirect bypass routes.

#[test]
fn test_c12b_023_wasm_edge_rejects_variable_bound_tag() {
    assert_edge_regex_rejected(
        "bypass_var_tag",
        "main =\n  tag <= \"Regex\"\n  re <= @(__type <= tag, pattern <= \"a\", flags <= \"\")\n  stdout(\"aba\".replaceAll(re, \"x\"))\n=> :Str\n",
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_edge_rejects_concat_tag() {
    assert_edge_regex_rejected(
        "bypass_concat",
        "re <= @(__type <= \"Re\" + \"gex\", pattern <= \"a\", flags <= \"\")\nstdout(\"aba\".replaceAll(re, \"x\"))\n",
        &["reserved for compiler-internal use"],
    );
}

/// Test: the well-known `fetch` capability (kind cloudflare/fetch) is
/// available without any wrangler manifest, emits the fetch/send step
/// chain over the wire, and decodes the resumed WebResponse-shaped value.
#[test]
fn wasm_edge_handler_host_call_fetch_capability_node() {
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("node not found, skipping wasm-edge fetch HostCall runtime test");
        return;
    }

    let dir = unique_temp_dir("taida_wasm_edge_host_call_fetch");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let js_path = dir.join("host_call_fetch.js");
    // No wrangler.jsonc on purpose: the fetch capability is injected by the
    // manifest reader unconditionally (Workers always expose global fetch).
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCall, HostStep, HostCapability)

CFFETCH <= "cloudflare/fetch"

handle req: WebRequest =
  fetcher <= HostCapability["fetch", CFFETCH]()
  out: WebRequest <= @(
    method <= "POST",
    path <= "/exchange",
    rawQuery <= "",
    query <= req.query,
    headers <= req.headers,
    body <= req.body
  )
  resp <=< Cage[fetcher, HostCall[@[HostStep["fetch", @["https://api.example.com/token"]](), HostStep["send", @[out]]()], WebResponse]()]()
  text("upstream=" + resp.status.toString())
=> :WebResponse
"#;
    std::fs::write(&td_path, source).expect("write fetch HostCall handler fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "fetch HostCall handler should compile without a wrangler manifest: {:?}",
        err
    );

    // The generated glue must carry the well-known fetch bridge.
    let glue_path = wasm_path.with_extension("edge.js");
    let glue = std::fs::read_to_string(&glue_path).expect("read generated glue");
    assert!(
        glue.contains("taidaFetchCapability"),
        "generated glue must define the well-known fetch capability bridge"
    );
    assert!(
        glue.contains("envelope.capability === \"fetch\""),
        "dispatchTaidaHostCall must resolve the fetch capability before env lookup"
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const request = encoder.encode(JSON.stringify({{
    method: "POST",
    path: "/auth/callback",
    rawQuery: "",
    query: [],
    headers: [{{ name: "accept", value: "application/json" }}],
    bodyBase64: "Y29kZT14",
  }}));
  const reqPtr = instance.exports.taida_abi_web_alloc(request.length);
  new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
  const handle = instance.exports.taida_abi_web_start(reqPtr, request.length);
  if (instance.exports.taida_abi_web_poll(handle) !== 1) throw new Error("expected host_call_pending");
  const pending = JSON.parse(decoder.decode(new Uint8Array(
    memory.buffer,
    instance.exports.taida_abi_web_out_ptr(handle),
    instance.exports.taida_abi_web_out_len(handle)
  )));
  if (pending.capability !== "fetch") throw new Error("bad capability " + pending.capability);
  if (pending.steps.length !== 2) throw new Error("bad step count " + pending.steps.length);
  if (pending.steps[0].method !== "fetch") throw new Error("missing fetch step");
  if (pending.steps[0].args[0] !== "https://api.example.com/token") throw new Error("bad url arg");
  if (pending.steps[1].method !== "send") throw new Error("missing send step");
  const sent = pending.steps[1].args[0];
  if (sent.method !== "POST") throw new Error("outbound method lost");
  if (sent.bodyBase64 !== "Y29kZT14") throw new Error("outbound body lost");
  if (!sent.headers.some((h) => h.name === "accept")) throw new Error("outbound headers lost");

  const resume = encoder.encode(JSON.stringify({{
    id: pending.id,
    ok: true,
    value: {{
      status: 203,
      headers: [{{ name: "content-type", value: "application/json" }}],
      bodyBase64: "e30=",
    }},
  }}));
  const resumePtr = instance.exports.taida_abi_web_alloc(resume.length);
  new Uint8Array(memory.buffer, resumePtr, resume.length).set(resume);
  instance.exports.taida_abi_web_resume(handle, resumePtr, resume.length);
  if (instance.exports.taida_abi_web_poll(handle) !== 0) throw new Error("expected response_ready");
  const raw = decoder.decode(new Uint8Array(
    memory.buffer,
    instance.exports.taida_abi_web_out_ptr(handle),
    instance.exports.taida_abi_web_out_len(handle)
  ));
  instance.exports.taida_abi_web_free(handle);
  console.log(raw);
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write fetch node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node fetch HostCall harness should run");
    assert!(
        run.status.success(),
        "node fetch HostCall harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    // "upstream=203" base64 is dXBzdHJlYW09MjAz
    assert!(
        stdout.contains(r#""status":200"#) && stdout.contains("dXBzdHJlYW09MjAz"),
        "fetch HostCall resume should decode the upstream status, got: {}",
        stdout
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Test: a handler that performs TWO sequential host calls re-executes with
/// the recorded resume payloads replaying in order. Guards the replay-table
/// regression where the latest resume value fed the first Cage of every
/// re-execution (corrupting and live-locking multi-call handlers).
#[test]
fn wasm_edge_handler_host_call_two_sequential_calls_node() {
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("node not found, skipping sequential HostCall runtime test");
        return;
    }

    let dir = unique_temp_dir("taida_wasm_edge_host_call_seq");
    let td_path = dir.join("handler.td");
    let wasm_path = dir.join("handler.wasm");
    let js_path = dir.join("host_call_seq.js");
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCall, HostStep, HostCapability)

KV <= "cloudflare/kv"

handle req: WebRequest =
  store <= HostCapability["STORE", KV]()
  first <=< Cage[store, HostCall[@[HostStep["get", @["alpha"]]()], Str]()]()
  second <=< Cage[store, HostCall[@[HostStep["get", @[first]]()], Str]()]()
  text(first + "/" + second)
=> :WebResponse
"#;
    let wrangler = r#"{ "kv_namespaces": [ { "binding": "STORE", "id": "x" } ] }"#;
    std::fs::write(dir.join("wrangler.jsonc"), wrangler).expect("write wrangler manifest");
    std::fs::write(&td_path, source).expect("write sequential HostCall fixture");

    let err = compile_wasm_edge_handler(&td_path, &wasm_path, "handle");
    assert!(
        err.is_none(),
        "sequential handler should compile: {:?}",
        err
    );

    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const wasm = fs.readFileSync("{wasm_for_js}");
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: {{
      fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {{
        new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
        return 0;
      }},
    }},
    taida_host: {{ env_get() {{ return 0; }}, env_get_all() {{ return 0; }} }},
  }};
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) memory = instance.exports.memory;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const request = encoder.encode(JSON.stringify({{
    method: "GET", path: "/seq", rawQuery: "", query: [], headers: [], bodyBase64: "",
  }}));
  const reqPtr = instance.exports.taida_abi_web_alloc(request.length);
  new Uint8Array(memory.buffer, reqPtr, request.length).set(request);
  const handle = instance.exports.taida_abi_web_start(reqPtr, request.length);

  const seen = [];
  for (let i = 0; i < 8; i++) {{
    const state = instance.exports.taida_abi_web_poll(handle);
    if (state === 0) {{
      const raw = decoder.decode(new Uint8Array(
        memory.buffer,
        instance.exports.taida_abi_web_out_ptr(handle),
        instance.exports.taida_abi_web_out_len(handle)
      ));
      instance.exports.taida_abi_web_free(handle);
      console.log(JSON.stringify({{ seen, response: JSON.parse(raw) }}));
      return;
    }}
    if (state !== 1) throw new Error("unexpected state " + state);
    const pending = JSON.parse(decoder.decode(new Uint8Array(
      memory.buffer,
      instance.exports.taida_abi_web_out_ptr(handle),
      instance.exports.taida_abi_web_out_len(handle)
    )));
    const arg = pending.steps[0].args[0];
    seen.push(arg);
    const value = arg === "alpha" ? "beta" : "gamma(" + arg + ")";
    const resume = encoder.encode(JSON.stringify({{ id: pending.id, ok: true, value }}));
    const resumePtr = instance.exports.taida_abi_web_alloc(resume.length);
    new Uint8Array(memory.buffer, resumePtr, resume.length).set(resume);
    instance.exports.taida_abi_web_resume(handle, resumePtr, resume.length);
  }}
  throw new Error("poll limit exceeded (live-lock)");
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).expect("write sequential node harness");

    let run = Command::new("node")
        .arg(&js_path)
        .output()
        .expect("node sequential harness should run");
    assert!(
        run.status.success(),
        "sequential HostCall harness failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("harness output should be JSON");
    // Call 1 asks for "alpha"; the replayed first result ("beta") must feed
    // call 2's argument. Response = "beta/gamma(beta)".
    assert_eq!(
        parsed["seen"],
        serde_json::json!(["alpha", "beta"]),
        "second call must receive the FIRST call's replayed result"
    );
    let body = parsed["response"]["bodyBase64"].as_str().unwrap_or("");
    // "beta/gamma(beta)" base64
    assert_eq!(
        body, "YmV0YS9nYW1tYShiZXRhKQ==",
        "response must combine both replayed results in order"
    );

    let _ = std::fs::remove_dir_all(dir);
}

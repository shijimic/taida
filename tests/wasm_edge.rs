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

use common::{taida_bin, wasmtime_bin};
use std::path::Path;
use std::process::Command;

/// Compile a .td file with wasm-edge and return the wasm path (or None on failure).
fn compile_wasm_edge(td_path: &Path, wasm_path: &Path) -> Option<String> {
    let output = Command::new(taida_bin())
        .args(["build", "wasm-edge"])
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

/// C12-7 (FB-26): wasm-edge hello world must remain in the stdout("literal")
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
        "main =\n  re <= @(__type <= \"Regex\", pattern <= \"a\", flags <= \"\")\n  stdout(\"aba\".replaceAll(re, \"x\"))\n",
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
        "main =\n  tag <= \"Regex\"\n  re <= @(__type <= tag, pattern <= \"a\", flags <= \"\")\n  stdout(\"aba\".replaceAll(re, \"x\"))\n",
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

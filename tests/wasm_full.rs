/// Integration tests for wasm-full backend.
///
/// WF-2a: Validates that wasm-full compiles correctly,
/// does not regress wasm-min/wasm-wasi, and produces correct output.
///
/// wasm-full is a superset of wasm-wasi, which is a superset of wasm-min.
/// It adds extended runtime functions (collections, string/number molds,
/// JSON, Gorillax, bytes, bitwise, etc.) on top of wasm-wasi.
///
/// RC-8b: Parity tests save compiled .wasm files to `target/wasm-test-cache/wasm-full/`
/// so superset tests can reuse them without recompiling.
mod common;

use common::{cache_wasm, cached_wasm, taida_bin, wasmtime_bin};
use std::path::Path;
use std::process::Command;

/// Compile a .td file with wasm-full and return the error message (or None on success).
fn compile_wasm_full(td_path: &Path, wasm_path: &Path) -> Option<String> {
    let output = Command::new(taida_bin())
        .args(["build", "wasm-full"])
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
// WF-2a: Smoke tests
// ---------------------------------------------------------------------------

/// Test: wasm-full compiles the hello example and produces correct output.
/// wasm-full is a superset of wasm-wasi, so wasm_min_hello.td should work.
#[test]
fn wasm_full_hello() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full hello test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_test_hello.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full hello should compile, got: {:?}",
        err
    );

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
    assert_eq!(stdout, "hello");
}

/// Test: wasm-full compiles and runs the env example via wasmtime --env.
/// wasm-full inherits wasm-wasi's env support.
#[test]
fn wasm_full_env() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full env test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_wasi_env.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_test_env.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full env should compile, got: {:?}",
        err
    );

    let run = Command::new(&wasmtime)
        .args([
            "run",
            "--env",
            "TAIDA_TEST_A=hello",
            "--env",
            "TAIDA_TEST_B=world",
            "--",
        ])
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
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines.len() >= 2,
        "wasm-full env output too short: {:?}",
        lines
    );
    assert_eq!(
        lines[0], "hello",
        "EnvVar should resolve TAIDA_TEST_A=hello"
    );
    assert_eq!(
        lines[1], "2",
        "allEnv should see exactly 2 injected env vars"
    );
}

// ---------------------------------------------------------------------------
// WF-2b: String molds parity test
// ---------------------------------------------------------------------------

/// Test: wasm-full compiles and runs compile_str_molds.td with native-parity output.
/// This validates all string mold functions implemented in runtime_full_wasm.c:
/// Upper, Lower, Trim, Replace (first and all), CharAt, Repeat, Reverse, Slice, Pad.
#[test]
fn wasm_full_str_molds_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full str_molds parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compile_str_molds.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_str_molds.wasm");

    // Compile with wasm-full
    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full str_molds should compile, got: {:?}",
        err
    );

    // Run with wasmtime
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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    // Get native output for parity comparison
    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full str_molds output should match native/interpreter output"
    );
}

/// Test: wasm-full compiles and runs compile_num_molds.td with native-parity output.
/// This validates all numeric mold functions implemented in runtime_full_wasm.c:
/// ToFixed, Abs (Int), Floor, Ceil, Round, Truncate, Clamp (Int).
#[test]
fn wasm_full_num_molds_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full num_molds parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compile_num_molds.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_num_molds.wasm");

    // Compile with wasm-full
    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full num_molds should compile, got: {:?}",
        err
    );

    // Run with wasmtime
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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    // Get native output for parity comparison
    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full num_molds output should match native/interpreter output"
    );
}

/// Test: wasm-full compiles and runs compile_list_molds.td with native-parity output.
/// This validates extended list operations in runtime_full_wasm.c:
/// Join, Sum, Concat, Append, Prepend, Sort, Sort(reverse), Unique, Flatten, FindIndex, Count.
#[test]
fn wasm_full_list_molds_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full list_molds parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compile_list_molds.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_list_molds.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full list_molds should compile, got: {:?}",
        err
    );

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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full list_molds output should match native/interpreter output"
    );
}

/// Test: wasm-full compiles and runs compile_list_map.td with native-parity output.
#[test]
fn wasm_full_list_map_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full list_map parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compile_list_map.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_list_map.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full list_map should compile, got: {:?}",
        err
    );

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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full list_map output should match native/interpreter output"
    );
}

/// Test: wasm-full compiles and runs compile_hashmap_set.td with native-parity output.
#[test]
fn wasm_full_hashmap_set_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full hashmap_set parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compile_hashmap_set.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_hashmap_set.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full hashmap_set should compile, got: {:?}",
        err
    );

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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full hashmap_set output should match native/interpreter output"
    );
}

// ---------------------------------------------------------------------------
// WF-2f: Polymorphic dispatch + Lax/Result parity tests
// ---------------------------------------------------------------------------

/// Test: wasm-full compile_methods.td matches interpreter output.
/// Validates: string contains/indexOf/lastIndexOf on static strings (low-address data section),
/// list.get() returning Lax, list.hasValue() on Lax, and all other state-check methods.
#[test]
fn wasm_full_methods_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full methods parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compile_methods.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_methods.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full methods should compile, got: {:?}",
        err
    );

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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full methods output should match interpreter output"
    );
}

/// Test: wasm-full compile_optional_result.td matches interpreter output.
/// Validates: Lax.isEmpty(), Lax.hasValue(), Div[0,0]().isEmpty() = true,
/// Result methods (isSuccess, isError, getOrDefault, map, flatMap).
#[test]
fn wasm_full_optional_result_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full optional_result parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compile_optional_result.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_optional_result.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full optional_result should compile, got: {:?}",
        err
    );

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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full optional_result output should match interpreter output"
    );
}

// ---------------------------------------------------------------------------
// WF-3a: JSON runtime parity tests
// ---------------------------------------------------------------------------

/// Test: wasm-full compiles and runs compile_json.td with native-parity output.
/// Validates JSON[raw, Schema]() schema cast with all rules.
#[test]
fn wasm_full_json_schema_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full json_schema parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compile_json.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_json_schema.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full json_schema should compile, got: {:?}",
        err
    );

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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full json_schema output should match interpreter output"
    );
}

/// Test: wasm-full compiles and runs compile_prelude.td with native-parity output.
/// Validates jsonEncode/jsonPretty serialization.
#[test]
fn wasm_full_json_encode_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full json_encode parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/compile_prelude.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_json_encode.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full json_encode should compile, got: {:?}",
        err
    );

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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full json_encode output should match interpreter output"
    );
}

/// Test: wasm-full compiles and runs 18_std_json.td with native-parity output.
/// Validates full JSON workflow: schema cast + encode + pretty + nested + list.
#[test]
fn wasm_full_json_full_parity() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full json_full parity test");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/18_std_json.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_json_full.wasm");

    let err = compile_wasm_full(&td_path, &wasm_path);
    assert!(
        err.is_none(),
        "wasm-full json_full should compile, got: {:?}",
        err
    );

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

    let wasm_stdout = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    let native_run = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter should run");
    let native_stdout = String::from_utf8_lossy(&native_run.stdout)
        .trim_end()
        .to_string();

    assert_eq!(
        wasm_stdout, native_stdout,
        "wasm-full json_full output should match interpreter output"
    );
}

// ---------------------------------------------------------------------------
// WF-5a: Comprehensive parity test
// ---------------------------------------------------------------------------

/// Run native binary for a .td file, return stdout or None on failure.
fn run_native(td_path: &Path) -> Option<String> {
    let native_path = std::env::temp_dir().join(format!(
        "taida_wf5_native_{}",
        td_path.file_stem()?.to_string_lossy()
    ));
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td_path)
        .arg("-o")
        .arg(&native_path)
        .output()
        .ok()?;
    if !build.status.success() {
        return None;
    }
    let run = Command::new(&native_path).output().ok()?;
    let _ = std::fs::remove_file(&native_path);
    if !run.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&run.stdout).trim_end().to_string())
}

// ---------------------------------------------------------------------------
// WF-5a: Per-fixture parity tests for wasm-full.
//
// C24 Phase 5 (RC-SLOW-2 / C24B-006): Decomposed from the previous monolithic
// `wasm_full_parity_all_examples` into one `#[test]` per fixture, plus a
// lightweight aggregate allowlist guard. See tests/wasm_wasi.rs for the
// design rationale.
// ---------------------------------------------------------------------------

/// Skip: WASI tests needing --env/--dir, edge tests needing host imports,
/// and server examples that block waiting for connections.
const FULL_SKIP_STEMS: &[&str] = &[
    "wasm_wasi_env",
    "wasm_wasi_exists",
    "wasm_wasi_file_io",
    "wasm_wasi_write_failure",
    "wasm_wasi_write_failure_shape",
    "wasm_wasi_stderr",     // stderr goes to separate fd
    "wasm_edge_env",        // edge profile, different env mechanism
    "net_http_hello",       // server blocks on httpServe waiting for connections
    "net_ws_echo",          // D28B-017 WS server example, requires WS client harness
    "net_sse_broadcaster",  // D28B-017 SSE server example, requires SSE client harness
    "net_http_client",      // D28B-017 HTTP client example, requires URL argv fixture
    "terminal_line_editor", // D28B-018 interactive terminal addon example, requires raw-mode harness
    "terminal_spinner",     // D28B-018 interactive terminal addon example, time-bound harness
    "terminal_mouse", // D28B-018 interactive terminal addon example, requires mouse capture harness
];

/// Examples that wasm-full cannot compile (unsupported features).
const FULL_EXPECTED_REJECTED: &[&str] = &[
    "net_http_parse_encode", // net package import cannot resolve in standalone wasm compile
];

/// Examples where the native backend itself fails.
const FULL_EXPECTED_NATIVE_FAIL: &[&str] = &[
    "compile_stream",
    "helper_val",
    "module_math",
    "module_utils",
    "transpile_npm",
    // RC1 Phase 4: addon-backed packages are interpreter-dispatch
    // only in RC1; Cranelift native backend deliberately rejects
    // them with a deterministic compile-time error.
    "addon_echo",
    // RC1.5-4: addon-backed example, native dispatch only
    "addon_terminal",
];

/// Known non-parity examples — pre-existing bugs in native/wasm, not
/// wasm-full regressions.
///
/// Previously contained `11_introspection` with the note "pointer addresses
/// differ between memory layouts" but that fixture parities now (verified
/// 2026-04-23 during C24 Phase 5 decomposition via the per-fixture test
/// `fixture_all_td_11_introspection`). The original loop test had
/// `parity_ok == 70`, which required 11_introspection to be in the
/// parity_ok bucket; so `known_mismatch` was already stale in recent main.
const FULL_EXPECTED_PARITY_DIFF: &[&str] = &[];

fn run_wasm_full_parity_fixture(stem: &str) {
    if FULL_SKIP_STEMS.contains(&stem) {
        return;
    }

    let Some(wasmtime) = wasmtime_bin() else {
        eprintln!("wasmtime not found, skipping wasm-full parity for {}", stem);
        return;
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(format!("{}.td", stem));

    // Native build + run
    let native_out = match run_native(&td_path) {
        Some(s) => s,
        None => {
            if FULL_EXPECTED_NATIVE_FAIL.contains(&stem) {
                return;
            }
            panic!(
                "WF-5 REGRESSION: native backend unexpectedly failed for {}. \
                 If this is now a real native failure, add to FULL_EXPECTED_NATIVE_FAIL.",
                stem
            );
        }
    };

    // wasm-full compile + run, caching the .wasm for superset reuse.
    let wasm_path = std::env::temp_dir().join(format!("taida_wf5_parity_{}.wasm", stem));
    let compile_output = Command::new(taida_bin())
        .args(["build", "wasm-full"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .ok();
    let wasm_output = compile_output.and_then(|co| {
        if !co.status.success() {
            return None;
        }
        cache_wasm("wasm-full", stem, &wasm_path);
        let run = Command::new(&wasmtime)
            .arg("run")
            .arg("--")
            .arg(&wasm_path)
            .output()
            .ok()?;
        let _ = std::fs::remove_file(&wasm_path);
        if !run.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&run.stdout).trim_end().to_string())
    });

    let wasm_out = match wasm_output {
        Some(s) => s,
        None => {
            if FULL_EXPECTED_REJECTED.contains(&stem) {
                return;
            }
            panic!(
                "WF-5 REGRESSION: wasm-full unexpectedly could not compile/run {}. \
                 If this is now a real regression, add to FULL_EXPECTED_REJECTED.",
                stem
            );
        }
    };

    if native_out != wasm_out {
        if FULL_EXPECTED_PARITY_DIFF.contains(&stem) {
            return;
        }
        panic!(
            "WF-5 PARITY FAILED for {}: native='{}' vs wasm-full='{}'",
            stem,
            native_out.chars().take(200).collect::<String>(),
            wasm_out.chars().take(200).collect::<String>(),
        );
    }
}

#[test]
fn wasm_full_parity_allowlist_guard() {
    use common::fixture_lists::ALL_TD_FIXTURES;
    let all = ALL_TD_FIXTURES;

    for stem in FULL_SKIP_STEMS
        .iter()
        .chain(FULL_EXPECTED_REJECTED)
        .chain(FULL_EXPECTED_NATIVE_FAIL)
        .chain(FULL_EXPECTED_PARITY_DIFF)
    {
        assert!(
            all.contains(stem),
            "WF-5: allowlist references unknown fixture `{}`; check spelling or remove from list",
            stem
        );
    }

    let expected_parity_ok = all.len()
        - FULL_SKIP_STEMS.len()
        - FULL_EXPECTED_REJECTED.len()
        - FULL_EXPECTED_NATIVE_FAIL.len()
        - FULL_EXPECTED_PARITY_DIFF.len();

    // WF-5: target parity-OK count. With per-fixture tests enforcing parity
    // individually, this aggregate count reflects the static fixture /
    // allowlist sizes. If a new fixture is added and parities, bump this
    // constant deliberately.
    //
    //   92 fixtures - 14 skip - 1 rejected - 7 native_fail - 0 diff = 70
    assert_eq!(
        expected_parity_ok,
        70,
        "WF-5: parity-OK count drift — got {} = |fixtures {}| - |skip {}| - |rejected {}| - \
         |native_fail {}| - |diff {}|. Expected 70. Update this constant deliberately.",
        expected_parity_ok,
        all.len(),
        FULL_SKIP_STEMS.len(),
        FULL_EXPECTED_REJECTED.len(),
        FULL_EXPECTED_NATIVE_FAIL.len(),
        FULL_EXPECTED_PARITY_DIFF.len(),
    );
}

// Per-fixture tests emitted by build.rs; see tests/wasm_wasi.rs for design.
macro_rules! c24_fixture_runner {
    ($stem:expr) => {
        run_wasm_full_parity_fixture($stem)
    };
}
include!(concat!(env!("OUT_DIR"), "/examples_all_td_tests.rs"));

// ---------------------------------------------------------------------------
// WF-5b: Superset property verification
// ---------------------------------------------------------------------------

/// WF-5b: Every example that wasm-wasi can compile should also compile with wasm-full.
///
/// RC-8b: Uses cached .wasm files from parity tests to avoid recompilation.
/// If a cached .wasm exists for both wasm-wasi and wasm-full, we know compilation
/// succeeded already and can skip the compilation entirely.
///
/// N-2: The cache is a best-effort optimization that does not affect test
/// correctness. Tests never rely on cache ordering or presence -- a cache miss
/// simply triggers recompilation. Test execution order does not matter.
#[test]
fn wasm_full_superset_of_wasm_wasi() {
    let examples_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut td_files: Vec<_> = std::fs::read_dir(&examples_dir)
        .expect("examples/ directory should exist")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "td"))
        .collect();
    td_files.sort();

    let mut wasi_ok_full_fail = Vec::new();
    let mut cache_hits = 0;

    for td_path in &td_files {
        let stem = td_path.file_stem().unwrap().to_string_lossy().to_string();
        // Skip edge-only examples
        if stem.starts_with("wasm_edge_") {
            continue;
        }

        // RC-8b: Check if both profiles have cached .wasm files
        // M-1: Pass td_path so stale caches (source newer than cache) are invalidated.
        let wasi_cached = cached_wasm("wasm-wasi", &stem, td_path).is_some();
        let full_cached = cached_wasm("wasm-full", &stem, td_path).is_some();

        if wasi_cached && full_cached {
            // Both compiled successfully in parity tests -- superset holds
            cache_hits += 1;
            continue;
        }

        // Fall back to compiling if cache misses
        let wasi_ok = if wasi_cached {
            true
        } else {
            let wasi_path = std::env::temp_dir().join(format!("taida_wf5b_wasi_{}.wasm", stem));
            let ok = Command::new(taida_bin())
                .args(["build", "wasm-wasi"])
                .arg(td_path)
                .arg("-o")
                .arg(&wasi_path)
                .output()
                .is_ok_and(|o| o.status.success());
            let _ = std::fs::remove_file(&wasi_path);
            ok
        };

        if !wasi_ok {
            continue; // wasm-wasi can't compile this, skip
        }

        let full_ok = if full_cached {
            true
        } else {
            let full_path = std::env::temp_dir().join(format!("taida_wf5b_full_{}.wasm", stem));
            let ok = Command::new(taida_bin())
                .args(["build", "wasm-full"])
                .arg(td_path)
                .arg("-o")
                .arg(&full_path)
                .output()
                .is_ok_and(|o| o.status.success());
            let _ = std::fs::remove_file(&full_path);
            ok
        };

        if !full_ok {
            wasi_ok_full_fail.push(stem);
        }
    }

    eprintln!(
        "WF-5b Superset: {} cache hits (skipped recompilation)",
        cache_hits
    );

    assert!(
        wasi_ok_full_fail.is_empty(),
        "WF-5b SUPERSET VIOLATION: wasm-wasi compiles but wasm-full rejects: {:?}",
        wasi_ok_full_fail
    );
}

// ---------------------------------------------------------------------------
// WF-5c: Non-regression tests
// ---------------------------------------------------------------------------

/// Test: wasm-min still works after wasm-full additions.
#[test]
fn wasm_full_does_not_break_wasm_min() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_nonreg_min.wasm");

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
        "wasm-min should still compile after wasm-full additions: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
}

/// Test: wasm-wasi still works after wasm-full additions.
#[test]
fn wasm_full_does_not_break_wasm_wasi() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_full_nonreg_wasi.wasm");

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
        "wasm-wasi should still compile after wasm-full additions: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
}

// ── C12B-023: Regex on wasm-full must produce compile error ──────────
//
// PHILOSOPHY I (silent-undefined 禁止): wasm-full still uses the shared
// runtime_core_wasm Regex stubs (it does not link real POSIX regex.h),
// so Regex construction + match/search must be rejected at compile time.

fn assert_full_regex_rejected(stem: &str, source: &str, candidates: &[&str]) {
    let td_path = std::env::temp_dir().join(format!("taida_c12b_023_full_{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("taida_c12b_023_full_{}.wasm", stem));
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("wasm-full")
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
        "C12B-023: wasm-full should reject Regex usage, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("[E1617]"),
        "C12B-023: wasm-full Regex rejection must emit [E1617], got: {}",
        stderr
    );
    assert!(
        candidates.iter().any(|l| stderr.contains(l)),
        "C12B-023: wasm-full [E1617] message should mention one of {:?}, got: {}",
        candidates,
        stderr
    );
}

#[test]
fn test_c12b_023_wasm_full_rejects_regex_ctor() {
    assert_full_regex_rejected(
        "ctor",
        "re <= Regex(\"\\\\d+\", \"\")\nstdout(\"built\")\n",
        &["Regex"],
    );
}

#[test]
fn test_c12b_023_wasm_full_rejects_str_match() {
    assert_full_regex_rejected(
        "match",
        "re <= Regex(\"\\\\d+\", \"\")\ns <= \"abc 123\"\nresult <= s.match(re)\nstdout(result)\n",
        &["Regex", "Str.match"],
    );
}

#[test]
fn test_c12b_023_wasm_full_rejects_str_search() {
    assert_full_regex_rejected(
        "search",
        "re <= Regex(\"\\\\d+\", \"\")\ns <= \"abc 123\"\ni <= s.search(re)\nstdout(i)\n",
        &["Regex", "Str.search"],
    );
}

// ── C12B-023 bypass closure (2026-04-15 external review fix) ─────────
#[test]
fn test_c12b_023_wasm_full_rejects_manual_pack_replaceall() {
    assert_full_regex_rejected(
        "bypass_replaceall",
        "main =\n  re <= @(__type <= \"Regex\", pattern <= \"a\", flags <= \"\")\n  stdout(\"aba\".replaceAll(re, \"x\"))\n",
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_full_rejects_manual_pack_match() {
    assert_full_regex_rejected(
        "bypass_match",
        "re <= @(__type <= \"Regex\", pattern <= \"a\", flags <= \"\")\nstdout(\"abc\".match(re))\n",
        &["reserved for compiler-internal use"],
    );
}

// C12B-023 root fix (2026-04-15 v2): indirect bypass routes.

#[test]
fn test_c12b_023_wasm_full_rejects_variable_bound_tag() {
    assert_full_regex_rejected(
        "bypass_var_tag",
        "main =\n  tag <= \"Regex\"\n  re <= @(__type <= tag, pattern <= \"a\", flags <= \"\")\n  stdout(\"aba\".replaceAll(re, \"x\"))\n",
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_full_rejects_concat_tag() {
    assert_full_regex_rejected(
        "bypass_concat",
        "re <= @(__type <= \"Re\" + \"gex\", pattern <= \"a\", flags <= \"\")\nstdout(\"aba\".replaceAll(re, \"x\"))\n",
        &["reserved for compiler-internal use"],
    );
}

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
        .args(["build", "--target", "wasm-full"])
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
        .args(["build", "--target", "native"])
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

/// WF-5a: For every .td file that compiles with wasm-full,
/// the output must match the native backend output exactly.
#[test]
fn wasm_full_parity_all_examples() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-full parity test");
            return;
        }
    };

    let examples_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut td_files: Vec<_> = std::fs::read_dir(&examples_dir)
        .expect("examples/ directory should exist")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "td"))
        .collect();
    td_files.sort();

    // Skip: WASI tests needing --env/--dir, edge tests needing host imports,
    // and server examples that block waiting for connections.
    let skip_stems: Vec<&str> = vec![
        "wasm_wasi_env",
        "wasm_wasi_exists",
        "wasm_wasi_file_io",
        "wasm_wasi_write_failure",
        "wasm_wasi_write_failure_shape",
        "wasm_wasi_stderr", // stderr goes to separate fd
        "wasm_edge_env",    // edge profile, different env mechanism
        "net_http_hello",   // server blocks on httpServe waiting for connections
    ];

    let mut parity_ok = Vec::new();
    let mut parity_fail = Vec::new();
    let mut compile_rejected = Vec::new();
    let mut native_fail = Vec::new();

    for td_path in &td_files {
        let stem = td_path.file_stem().unwrap().to_string_lossy().to_string();
        if skip_stems.contains(&stem.as_str()) {
            continue;
        }

        // Native build + run
        let native_output = run_native(td_path);
        if native_output.is_none() {
            native_fail.push(stem.clone());
            continue;
        }
        let native_out = native_output.unwrap();

        // wasm-full compile + run
        // RC-8b: Cache the .wasm so superset test can reuse it.
        let wasm_path = std::env::temp_dir().join(format!("taida_wf5_parity_{}.wasm", stem));
        let compile_output = Command::new(taida_bin())
            .args(["build", "--target", "wasm-full"])
            .arg(td_path)
            .arg("-o")
            .arg(&wasm_path)
            .output()
            .ok();
        let wasm_output = compile_output.and_then(|co| {
            if !co.status.success() {
                return None;
            }
            // RC-8b: Cache the .wasm for superset test reuse
            cache_wasm("wasm-full", &stem, &wasm_path);
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
        if wasm_output.is_none() {
            compile_rejected.push(stem.clone());
            continue;
        }
        let wasm_out = wasm_output.unwrap();

        // Known non-parity examples (pre-existing bugs in native/wasm, not wasm-full regressions)
        // 06_lists and 27_prelude_result fixed (Reverse mold + mapError toString)
        let known_mismatch: Vec<&str> = vec![
            "11_introspection", // pointer addresses differ between memory layouts
        ];

        if native_out == wasm_out {
            parity_ok.push(stem.clone());
        } else if known_mismatch.contains(&stem.as_str()) {
            // Expected mismatch, skip
        } else {
            parity_fail.push((stem.clone(), native_out, wasm_out));
        }
    }

    eprintln!(
        "WF-5 Parity: {} OK, {} rejected, {} native-fail",
        parity_ok.len(),
        compile_rejected.len(),
        native_fail.len()
    );

    if !parity_fail.is_empty() {
        let mut msg = format!("WF-5 PARITY FAILED for {} example(s):\n", parity_fail.len());
        for (stem, native, wasm) in &parity_fail {
            msg.push_str(&format!(
                "\n  {}: native='{}' vs wasm-full='{}'\n",
                stem,
                native.chars().take(100).collect::<String>(),
                wasm.chars().take(100).collect::<String>()
            ));
        }
        panic!("{}", msg);
    }

    // --- Strict regression guards (exact counts, not thresholds) ---

    // Expected allowlist: examples that wasm-full cannot compile (unsupported features).
    // Note: stems that fail at the native build/run stage go into native_fail, not here.
    // If this list shrinks, update the count -- that's progress.
    // If it grows, the test fails -- that's a regression.
    // NTH-6: allowlist reduced after NTH-5 poly_add string support enabled
    // 10 examples that previously failed now compile and pass parity.
    let expected_rejected: Vec<&str> = vec![
        // PR-4: 13_async, 14_unmold_backward, compile_async now pass with wasm async support
        // PR-3: 09_modules, compile_module, compile_module_value now pass with module inlining
        "net_http_parse_encode", // net package import cannot resolve in standalone wasm compile
    ];

    // Expected allowlist: examples where native backend itself fails (build or run).
    // These are checked before wasm-full compilation, so they never appear in compile_rejected.
    let expected_native_fail: Vec<&str> = vec![
        "compile_stream",
        "helper_val",
        "module_math",
        "module_utils",
        "transpile_npm",
        // net_http_hello: moved to skip_stems (blocks on httpServe)
        // RC1 Phase 4: addon-backed packages are interpreter-dispatch
        // only in RC1; Cranelift native backend deliberately rejects
        // them with a deterministic compile-time error.
        "addon_echo",
        // RC1.5-4: addon-backed example, native dispatch only
        "addon_terminal",
    ];

    // Detect regressions: any new rejected example not in the allowlist
    let unexpected_rejected: Vec<&String> = compile_rejected
        .iter()
        .filter(|s| !expected_rejected.contains(&s.as_str()))
        .collect();
    assert!(
        unexpected_rejected.is_empty(),
        "WF-5 REGRESSION: unexpected compile_rejected: {:?}",
        unexpected_rejected
    );

    let unexpected_native_fail: Vec<&String> = native_fail
        .iter()
        .filter(|s| !expected_native_fail.contains(&s.as_str()))
        .collect();
    assert!(
        unexpected_native_fail.is_empty(),
        "WF-5 REGRESSION: unexpected native_fail: {:?}",
        unexpected_native_fail
    );

    // Exact parity count -- if this changes, update deliberately.
    // Known non-parity (excluded from this count):
    //   11_introspection: pointer addresses differ between native and wasm memory layouts
    // 06_lists and 27_prelude_result fixed (Reverse mold + mapError toString)
    // PR-4: 13_async, 14_unmold_backward, compile_async now pass (54 -> 57)
    // PR-3: 09_modules, compile_module, compile_module_value now pass (57 -> 60)
    assert_eq!(
        parity_ok.len(),
        60,
        "WF-5: Expected exactly 60 parity-OK examples, got {}. \
         If parity improved, update the expected count. List: {:?}",
        parity_ok.len(),
        parity_ok
    );

    // Guard against allowlist growing (regressions)
    assert!(
        compile_rejected.len() <= expected_rejected.len(),
        "WF-5: compile_rejected count ({}) exceeds expected allowlist ({}). \
         A previously compilable example regressed.",
        compile_rejected.len(),
        expected_rejected.len()
    );

    assert!(
        native_fail.len() <= expected_native_fail.len(),
        "WF-5: native_fail count ({}) exceeds expected allowlist ({}). \
         A previously working native example regressed.",
        native_fail.len(),
        expected_native_fail.len()
    );
}

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
                .args(["build", "--target", "wasm-wasi"])
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
                .args(["build", "--target", "wasm-full"])
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
        .args(["build", "--target", "wasm-min"])
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
        .args(["build", "--target", "wasm-wasi"])
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

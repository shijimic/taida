/// Integration tests for wasm-wasi backend.
///
/// Compiles .td files to .wasm via `taida build --target wasm-wasi`,
/// runs them with wasmtime, and verifies output matches Native/interpreter.
///
/// WW-2: Tests for env, file I/O, and stderr non-regression.
/// WW-3: Validation — parity, superset property, size checks.
///
/// RC-8b: Parity tests save compiled .wasm files to `target/wasm-test-cache/<profile>/`
/// so superset tests can reuse them without recompiling.
mod common;

use common::{run_interpreter, taida_bin, wasmtime_bin};
use std::path::Path;
use std::process::Command;

/// Compile a .td file to wasm-wasi and run with wasmtime.
/// `extra_args` are passed to wasmtime (e.g. --env, --dir).
fn compile_and_run_wasm_wasi(
    td_path: &Path,
    wasmtime: &Path,
    extra_args: &[&str],
) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let wasm_path = std::env::temp_dir().join(format!("taida_wasm_wasi_test_{}.wasm", stem));

    let compile_output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-wasi")
        .arg(td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .ok()?;

    if !compile_output.status.success() {
        let stderr = String::from_utf8_lossy(&compile_output.stderr);
        eprintln!(
            "wasm-wasi compile failed for {}: {}",
            td_path.display(),
            stderr
        );
        return None;
    }

    let mut cmd = Command::new(wasmtime);
    cmd.arg("run");
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.arg("--").arg(&wasm_path);
    let run_output = cmd.output().ok()?;

    let _ = std::fs::remove_file(&wasm_path);

    if !run_output.status.success() {
        let stderr = String::from_utf8_lossy(&run_output.stderr);
        eprintln!(
            "wasmtime execution failed for {}: {}",
            td_path.display(),
            stderr
        );
        return None;
    }

    Some(
        String::from_utf8_lossy(&run_output.stdout)
            .trim_end()
            .to_string(),
    )
}

/// Test: wasm-wasi compiles and runs the stderr fixture.
/// Verifies stdout output matches interpreter (stderr goes to fd=2).
#[test]
fn wasm_wasi_stderr() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-wasi tests");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_wasi_stderr.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm =
        compile_and_run_wasm_wasi(&td_path, &wasmtime, &[]).expect("wasm-wasi should succeed");

    assert_eq!(
        interp, wasm,
        "wasm-wasi stderr output should match interpreter"
    );
}

/// Test: wasm-wasi EnvVar + allEnv.
/// Injects env vars via wasmtime --env and verifies output.
#[test]
fn wasm_wasi_env() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-wasi tests");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_wasi_env.td");

    // Run interpreter with the same env vars
    let interp_output = Command::new(taida_bin())
        .arg(&td_path)
        .env("TAIDA_TEST_A", "hello")
        .env("TAIDA_TEST_B", "world")
        .output()
        .expect("interpreter should run");
    assert!(interp_output.status.success(), "interpreter failed");
    let _interp = String::from_utf8_lossy(&interp_output.stdout)
        .trim_end()
        .to_string();

    // Compile wasm-wasi
    let wasm_path = std::env::temp_dir().join("taida_wasm_wasi_test_env.wasm");
    let compile = Command::new(taida_bin())
        .args(["build", "--target", "wasm-wasi"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("compile should run");
    assert!(
        compile.status.success(),
        "wasm-wasi compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    // Run with wasmtime, injecting only our 2 env vars
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
    let wasm = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    // Line 1: EnvVar unmold value
    let wasm_lines: Vec<&str> = wasm.lines().collect();
    assert!(
        wasm_lines.len() >= 2,
        "wasm output too short: {:?}",
        wasm_lines
    );
    assert_eq!(
        wasm_lines[0], "hello",
        "EnvVar should resolve TAIDA_TEST_A=hello"
    );

    // Line 2: allEnv().size() — wasmtime --env injects exactly 2 vars
    // Interpreter may see more host env vars, so we only check wasm side
    assert_eq!(
        wasm_lines[1], "2",
        "allEnv should see exactly 2 injected env vars"
    );
}

/// Test: wasm-wasi file I/O (Read, writeFile, Exists).
#[test]
fn wasm_wasi_file_io() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-wasi tests");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_wasi_file_io.td");

    // Run interpreter
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    // Clean up temp file from interpreter run
    let _ = std::fs::remove_file("_wasi_test_tmp.txt");

    // Compile wasm-wasi
    let wasm_path = std::env::temp_dir().join("taida_wasm_wasi_test_file_io.wasm");
    let compile = Command::new(taida_bin())
        .args(["build", "--target", "wasm-wasi"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("compile should run");
    assert!(
        compile.status.success(),
        "wasm-wasi compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    // Run with wasmtime, granting access to current directory
    let run = Command::new(&wasmtime)
        .args(["run", "--dir=.", "--"])
        .arg(&wasm_path)
        .output()
        .expect("wasmtime should run");
    let _ = std::fs::remove_file(&wasm_path);
    // Clean up temp file from wasm run
    let _ = std::fs::remove_file("_wasi_test_tmp.txt");

    assert!(
        run.status.success(),
        "wasmtime failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let wasm = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    // Verify writeFile + Read round-trip works
    assert_eq!(
        wasm.trim(),
        "hello from wasi",
        "wasm-wasi file I/O should write and read back content"
    );
    assert_eq!(
        interp, wasm,
        "wasm-wasi file I/O output should match interpreter"
    );
}

/// Test: wasm-wasi Exists[path]() — verifies both existing and non-existing paths.
#[test]
fn wasm_wasi_exists() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-wasi tests");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_wasi_exists.td");

    // Run interpreter
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    // Clean up temp file from interpreter run
    let _ = std::fs::remove_file("_wasi_exists_test.txt");

    // Compile wasm-wasi
    let wasm_path = std::env::temp_dir().join("taida_wasm_wasi_test_exists.wasm");
    let compile = Command::new(taida_bin())
        .args(["build", "--target", "wasm-wasi"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("compile should run");
    assert!(
        compile.status.success(),
        "wasm-wasi compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    // Run with wasmtime, granting access to current directory
    let run = Command::new(&wasmtime)
        .args(["run", "--dir=.", "--"])
        .arg(&wasm_path)
        .output()
        .expect("wasmtime should run");
    let _ = std::fs::remove_file(&wasm_path);
    // Clean up temp file from wasm run
    let _ = std::fs::remove_file("_wasi_exists_test.txt");

    assert!(
        run.status.success(),
        "wasmtime failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let wasm = String::from_utf8_lossy(&run.stdout).trim_end().to_string();

    assert_eq!(
        wasm, "true\nfalse",
        "Exists should return true for existing file and false for non-existing"
    );
    assert_eq!(
        interp, wasm,
        "wasm-wasi Exists output should match interpreter"
    );
}

/// Test: wasm-wasi writeFile failure path — non-existent directory.
#[test]
fn wasm_wasi_write_failure() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-wasi tests");
            return;
        }
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_wasi_write_failure.td");

    // Compile and run wasm-wasi
    let wasm = compile_and_run_wasm_wasi(&td_path, &wasmtime, &["--dir=."])
        .expect("wasm-wasi should succeed");

    assert_eq!(
        wasm, "true",
        "writeFile to non-existent dir should report isError() = true"
    );
}

/// Test: wasm-wasi writeFile failure shape — validates error field names survive Result toString.
/// WFX-S1: ensures error shape (type, message, kind) is not silently broken.
#[test]
fn wasm_wasi_write_failure_shape() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-wasi tests");
            return;
        }
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_wasi_write_failure_shape.td");

    // Compile and run wasm-wasi
    let wasm = compile_and_run_wasm_wasi(&td_path, &wasmtime, &["--dir=."])
        .expect("wasm-wasi should succeed");

    let wasm_lines: Vec<&str> = wasm.lines().collect();
    assert!(
        wasm_lines.len() >= 2,
        "wasm output too short: {:?}",
        wasm_lines
    );

    // Line 1: isError() should be true
    assert_eq!(
        wasm_lines[0], "true",
        "writeFile to non-existent dir should report isError() = true"
    );

    // Line 2: toString() should show "Result(throw <= <error message>)"
    // After Bug E fix, throw display extracts the message field (matching interpreter),
    // not the full BuchiPack structure.
    assert!(
        wasm_lines[1].starts_with("Result(throw <= ") && wasm_lines[1].ends_with(")"),
        "Result toString should show throw with error message, got: {}",
        wasm_lines[1]
    );
    // Verify error message content mentions the writeFile operation
    assert!(
        wasm_lines[1].contains("writeFile"),
        "Error message should mention 'writeFile', got: {}",
        wasm_lines[1]
    );
}

/// Test: wasm-min non-regression — ensure wasm-min still works after wasm-wasi additions.
#[test]
fn wasm_wasi_does_not_break_wasm_min() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");

    // Just verify wasm-min compilation still works
    let wasm_path = std::env::temp_dir().join("taida_wasm_wasi_nonreg_hello.wasm");
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
        "wasm-min should still compile after wasm-wasi additions: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
}

// ---------------------------------------------------------------------------
// WW-3: Validation — parity, superset property, size checks
// ---------------------------------------------------------------------------

/// Run a .td file with the native backend and return its stdout.
fn run_native(td_path: &Path) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let native_path = std::env::temp_dir().join(format!("taida_native_wasi_test_{}", stem));

    let compile_output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(td_path)
        .arg("-o")
        .arg(&native_path)
        .output()
        .ok()?;

    if !compile_output.status.success() {
        return None;
    }

    let output = Command::new(&native_path).output().ok()?;
    let _ = std::fs::remove_file(&native_path);

    if !output.status.success() {
        return None;
    }

    Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
    )
}

/// Compile a .td file to a given target and return the .wasm file size in bytes.
fn compile_wasm_and_get_size(td_path: &Path, target: &str, wasm_path: &Path) -> Option<u64> {
    let output = Command::new(taida_bin())
        .args(["build", "--target", target])
        .arg(td_path)
        .arg("-o")
        .arg(wasm_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let size = std::fs::metadata(wasm_path).map(|m| m.len()).ok()?;
    let _ = std::fs::remove_file(wasm_path);
    Some(size)
}

// RC-8b: Cache functions moved to tests/common/mod.rs (S-2: DRY).
use common::{cache_wasm, cached_wasm};

/// S-1: Result of `run_wasm_cached_or_compile`, carrying both the output and
/// whether the result came from the test cache.
struct CachedRunResult {
    stdout: String,
    cache_hit: bool,
}

/// RC-8b: Run a cached or freshly compiled .wasm with wasmtime, returning stdout
/// and cache-hit information.
///
/// N-2: The cache is a best-effort optimization that does not affect test
/// correctness. Tests never rely on cache ordering or presence -- a cache miss
/// simply triggers recompilation. Test execution order does not matter.
///
/// M-1: Uses `cached_wasm` with source-path comparison to invalidate stale caches.
fn run_wasm_cached_or_compile(
    td_path: &Path,
    profile: &str,
    wasmtime: &Path,
) -> Option<CachedRunResult> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();

    // Try cache first (M-1: td_path passed for stale-cache detection)
    if let Some(cache_path) = cached_wasm(profile, &stem, td_path) {
        let run = Command::new(wasmtime).arg(&cache_path).output().ok()?;
        if run.status.success() {
            return Some(CachedRunResult {
                stdout: String::from_utf8_lossy(&run.stdout).trim_end().to_string(),
                cache_hit: true,
            });
        }
        // Cache hit but wasmtime failed -- fall through to recompile.
    }

    // Cache miss or stale: compile, cache, and run
    let wasm_path = std::env::temp_dir().join(format!("taida_rc8b_{}_{}.wasm", profile, stem));
    let compile_output = Command::new(taida_bin())
        .args(["build", "--target", profile])
        .arg(td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .ok()?;

    if !compile_output.status.success() {
        return None;
    }

    cache_wasm(profile, &stem, &wasm_path);

    let run = Command::new(wasmtime).arg(&wasm_path).output().ok()?;
    let _ = std::fs::remove_file(&wasm_path);

    if !run.status.success() {
        return None;
    }

    Some(CachedRunResult {
        stdout: String::from_utf8_lossy(&run.stdout).trim_end().to_string(),
        cache_hit: false,
    })
}

/// WW-3a: Comprehensive parity test for wasm-wasi.
/// For every .td file in examples/ that successfully compiles with wasm-wasi,
/// the wasm output must match the native backend output exactly.
/// Excludes wasm_wasi_env.td (needs --env args) and file I/O tests (need --dir).
#[test]
fn wasm_wasi_parity_all_examples() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-wasi parity test");
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

    // Skip files that need special wasmtime args (env injection, dir access)
    // Also skip wasm_edge_* examples (different profile, tested in wasm_edge.rs)
    let skip_stems: Vec<&str> = vec![
        "wasm_wasi_env",                 // needs --env
        "wasm_wasi_file_io",             // needs --dir, creates temp files
        "wasm_wasi_exists",              // needs --dir, creates temp files
        "wasm_wasi_write_failure",       // needs --dir
        "wasm_wasi_write_failure_shape", // needs --dir
        "wasm_edge_env",                 // wasm-edge profile, needs taida_host imports
        "net_http_hello",                // server blocks on httpServe waiting for connections
    ];

    let mut parity_ok = Vec::new();
    let mut parity_fail = Vec::new();
    let mut compile_rejected = Vec::new();
    let mut native_fail = Vec::new();

    for td_path in &td_files {
        let stem = td_path.file_stem().unwrap().to_string_lossy().to_string();

        // Skip files that need special runtime args
        if skip_stems.contains(&stem.as_str()) {
            continue;
        }

        // Try native build first
        let native_output = run_native(td_path);
        if native_output.is_none() {
            native_fail.push(stem.clone());
            continue;
        }
        let native_out = native_output.unwrap();

        // Try wasm-wasi compile + run.
        // RC-8b: Save to cache so superset tests can reuse without recompiling.
        let parity_wasm_path = std::env::temp_dir().join(format!("taida_ww3_parity_{}.wasm", stem));
        let compile_output = Command::new(taida_bin())
            .args(["build", "--target", "wasm-wasi"])
            .arg(td_path)
            .arg("-o")
            .arg(&parity_wasm_path)
            .output()
            .ok();
        let wasm_output = compile_output.and_then(|co| {
            if !co.status.success() {
                return None;
            }
            // RC-8b: Cache the .wasm before running wasmtime
            cache_wasm("wasm-wasi", &stem, &parity_wasm_path);
            let run = Command::new(&wasmtime)
                .arg(&parity_wasm_path)
                .output()
                .ok()?;
            let _ = std::fs::remove_file(&parity_wasm_path);
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

        if native_out == wasm_out {
            parity_ok.push(stem.clone());
        } else {
            parity_fail.push((stem.clone(), native_out, wasm_out));
        }
    }

    eprintln!(
        "WW-3 Parity: {} OK, {} rejected, {} native-fail",
        parity_ok.len(),
        compile_rejected.len(),
        native_fail.len()
    );

    // All previously known parity diffs have been fixed (Bug A-G + Reverse mold).
    let expected_parity_diff: Vec<&str> = vec![];

    let unexpected_parity_fail: Vec<_> = parity_fail
        .iter()
        .filter(|(stem, _, _)| !expected_parity_diff.contains(&stem.as_str()))
        .collect();

    if !unexpected_parity_fail.is_empty() {
        let mut msg = format!(
            "WW-3 PARITY FAILED for {} example(s):\n",
            unexpected_parity_fail.len()
        );
        for (stem, native, wasm) in &unexpected_parity_fail {
            msg.push_str(&format!(
                "\n  {}: native='{}' vs wasm-wasi='{}'\n",
                stem,
                native.chars().take(100).collect::<String>(),
                wasm.chars().take(100).collect::<String>()
            ));
        }
        panic!("{}", msg);
    }

    // --- Strict regression guards (exact counts, not thresholds) ---

    // Expected allowlist: examples that wasm-wasi cannot compile (unsupported features).
    // If this list shrinks, update the count — that's progress.
    // If it grows, the test fails — that's a regression.
    // NTH-6: allowlist reduced after NTH-5 poly_add string support enabled
    // 3 examples (07_closures, compile_lambda, wasm_min_pi_approx) now pass parity.
    // WC-3: updated after list molds moved to core. Many list examples now compile.
    // Removed: 10_list_operations, 11_introspection, 16_unmold_both_directions,
    //          compile_hof_molds, compile_list, compile_list_map, compile_list_molds,
    //          compile_rc, compile_str_molds, compile_num_molds, todo_app
    // WC-6: Many examples removed from rejected list — now compile with core
    // extensions. Some produce parity diffs (expected_parity_diff above),
    // others pass parity (28_prelude_collections, 30_class_like_methods,
    // compile_hashmap_set, compile_lax, compile_pack_field_call).
    let expected_rejected: Vec<&str> = vec![
        // PR-4: 13_async, 14_unmold_backward, compile_async now pass with wasm async support
        // PR-3: 09_modules, compile_module, compile_module_value now pass with module inlining
        "net_http_parse_encode", // net package import cannot resolve in standalone wasm compile
    ];

    // Expected allowlist: examples where native backend itself fails.
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

    // Detect regressions: any new rejected/native-fail example not in the allowlist
    let unexpected_rejected: Vec<&String> = compile_rejected
        .iter()
        .filter(|s| !expected_rejected.contains(&s.as_str()))
        .collect();
    assert!(
        unexpected_rejected.is_empty(),
        "WW-3 REGRESSION: unexpected compile_rejected examples: {:?}",
        unexpected_rejected
    );

    let unexpected_native_fail: Vec<&String> = native_fail
        .iter()
        .filter(|s| !expected_native_fail.contains(&s.as_str()))
        .collect();
    assert!(
        unexpected_native_fail.is_empty(),
        "WW-3 REGRESSION: unexpected native_fail examples: {:?}",
        unexpected_native_fail
    );

    // Exact parity count — if this changes, update deliberately.
    // WC-4: JSON in core → 42
    // WC-6: Collection & Pack & Type detection in core → 47
    // Bug A-G + Reverse fix: 47 → 55 (8 previously known diffs now pass)
    // PR-4: async support: 55 → 58 (13_async, 14_unmold_backward, compile_async)
    // PR-3: module inlining: 58 → 61 (09_modules, compile_module, compile_module_value)
    assert_eq!(
        parity_ok.len(),
        61,
        "WW-3: Expected exactly 61 parity-OK examples, got {}. \
         If parity improved, update the expected count. List: {:?}",
        parity_ok.len(),
        parity_ok
    );

    // Guard against allowlist growing (regressions)
    assert!(
        compile_rejected.len() <= expected_rejected.len(),
        "WW-3: compile_rejected count ({}) exceeds expected allowlist ({}). \
         A previously compilable example regressed.",
        compile_rejected.len(),
        expected_rejected.len()
    );

    assert!(
        native_fail.len() <= expected_native_fail.len(),
        "WW-3: native_fail count ({}) exceeds expected allowlist ({}). \
         A previously working native example regressed.",
        native_fail.len(),
        expected_native_fail.len()
    );
}

/// WW-3b: Superset property -- everything wasm-min can compile and run,
/// wasm-wasi must also compile, run, and produce identical output.
///
/// RC-8b: Uses `run_wasm_cached_or_compile` to reuse .wasm artifacts from
/// parity tests (`wasm_min_parity_all_examples` and `wasm_wasi_parity_all_examples`),
/// avoiding double compilation of every example.
///
/// N-2: The cache is a best-effort optimization that does not affect test
/// correctness. Tests never rely on cache ordering or presence -- a cache miss
/// simply triggers recompilation. Test execution order does not matter.
#[test]
fn wasm_wasi_superset_of_wasm_min() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("wasmtime not found, skipping wasm-wasi superset test");
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

    let mut superset_ok = 0;
    let mut superset_fail = Vec::new();
    let mut min_only = Vec::new();
    let mut cache_hits = 0;

    for td_path in &td_files {
        let stem = td_path.file_stem().unwrap().to_string_lossy().to_string();

        // Skip wasm_wasi_* examples (they use OS APIs not in wasm-min)
        if stem.starts_with("wasm_wasi_") {
            continue;
        }

        // RC-8b: Try cached wasm-min output first, fall back to compile
        let min_result = run_wasm_cached_or_compile(td_path, "wasm-min", &wasmtime);
        if min_result.is_none() {
            // wasm-min cannot compile this -- no superset obligation
            continue;
        }
        let min_result = min_result.unwrap();

        // RC-8b: Try cached wasm-wasi output first, fall back to compile
        let wasi_result = run_wasm_cached_or_compile(td_path, "wasm-wasi", &wasmtime);
        if wasi_result.is_none() {
            min_only.push(stem.clone());
            continue;
        }
        let wasi_result = wasi_result.unwrap();

        // S-1: Count cache hits from the authoritative CachedRunResult.
        if min_result.cache_hit {
            cache_hits += 1;
        }
        if wasi_result.cache_hit {
            cache_hits += 1;
        }

        if min_result.stdout == wasi_result.stdout {
            superset_ok += 1;
        } else {
            superset_fail.push((stem.clone(), min_result.stdout, wasi_result.stdout));
        }
    }

    eprintln!(
        "WW-3 Superset: {} OK, {} wasm-min-only (superset violation), {} cache hits",
        superset_ok,
        min_only.len(),
        cache_hits
    );

    // Superset violations: wasm-min succeeded but wasm-wasi failed
    assert!(
        min_only.is_empty(),
        "WW-3 SUPERSET VIOLATION: wasm-min succeeded but wasm-wasi failed for: {:?}",
        min_only
    );

    // Output mismatch: wasm-min and wasm-wasi produce different output
    if !superset_fail.is_empty() {
        let mut msg = format!(
            "WW-3 SUPERSET OUTPUT MISMATCH for {} example(s):\n",
            superset_fail.len()
        );
        for (stem, min_out, wasi_out) in &superset_fail {
            msg.push_str(&format!(
                "\n  {}: wasm-min='{}' vs wasm-wasi='{}'\n",
                stem,
                min_out.chars().take(100).collect::<String>(),
                wasi_out.chars().take(100).collect::<String>()
            ));
        }
        panic!("{}", msg);
    }

    // Exact superset count — if this changes, update deliberately.
    // WC-4: JSON in core → 41
    // WC-6: Collection & Pack & Type detection in core → 54
    // PR-4: async support: 54 → 57 (13_async, 14_unmold_backward, compile_async)
    // PR-3: module inlining: 57 → 60 (09_modules, compile_module, compile_module_value)
    assert_eq!(
        superset_ok, 60,
        "WW-3: Expected exactly 60 superset-verified examples, got {}. \
         If superset coverage improved, update the expected count.",
        superset_ok
    );
}

/// WW-3c: Size check — wasm-wasi binaries should be reasonably bounded.
/// wasm-wasi includes WASI I/O layer, so binaries are larger than wasm-min,
/// but should not be excessively large.
#[test]
fn wasm_wasi_size_check() {
    let hello_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");
    let stderr_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_wasi_stderr.td");

    let hello_min_size = compile_wasm_and_get_size(
        &hello_path,
        "wasm-min",
        &std::env::temp_dir().join("taida_ww3_hello_min.wasm"),
    )
    .expect("wasm-min hello should compile");

    let hello_wasi_size = compile_wasm_and_get_size(
        &hello_path,
        "wasm-wasi",
        &std::env::temp_dir().join("taida_ww3_hello_wasi.wasm"),
    )
    .expect("wasm-wasi hello should compile");

    let stderr_wasi_size = compile_wasm_and_get_size(
        &stderr_path,
        "wasm-wasi",
        &std::env::temp_dir().join("taida_ww3_stderr_wasi.wasm"),
    )
    .expect("wasm-wasi stderr should compile");

    eprintln!(
        "WW-3 Size: hello(wasm-min)={}B, hello(wasm-wasi)={}B, stderr(wasm-wasi)={}B",
        hello_min_size, hello_wasi_size, stderr_wasi_size
    );

    // wasm-wasi hello should be <= 4KB (WASI layer adds overhead but --gc-sections prunes)
    assert!(
        hello_wasi_size <= 4096,
        "WW-3: wasm-wasi hello should be <= 4KB, got {} bytes",
        hello_wasi_size
    );

    // wasm-wasi stderr should be <= 4KB (adds only fd_write to fd=2)
    assert!(
        stderr_wasi_size <= 4096,
        "WW-3: wasm-wasi stderr should be <= 4KB, got {} bytes",
        stderr_wasi_size
    );

    // wasm-wasi should not be more than 10x larger than wasm-min for the same code
    let ratio = hello_wasi_size as f64 / hello_min_size as f64;
    eprintln!("WW-3 Size ratio (wasm-wasi/wasm-min): {:.1}x", ratio);
    assert!(
        ratio <= 10.0,
        "WW-3: wasm-wasi should not be more than 10x larger than wasm-min, got {:.1}x",
        ratio
    );
}

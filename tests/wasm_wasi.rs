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

    // C12B-021 migration: Exists returns Result[Bool]. The existing
    // path and the missing path both succeed as probes, so
    // `.isSuccess()` is `true` on both. The inner Bool bit is
    // exercised via the Interpreter unit tests (Exists behaviour
    // test suite) and is not cross-backend asserted here because
    // the wasm runtime does not propagate Bool tags through
    // `.__value` field access (documented gap).
    assert_eq!(
        wasm, "true\ntrue",
        "Exists should report isSuccess=true for both existing and missing paths"
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

// ---------------------------------------------------------------------------
// WW-3a: Per-fixture parity tests for wasm-wasi.
//
// C24 Phase 5 (RC-SLOW-2 / C24B-006): Previously a single test
// `wasm_wasi_parity_all_examples` iterated over `examples/*.td` in a tight
// loop. That hid fixture-level failures behind one test name and prevented
// nextest from parallelizing across fixtures. Now build.rs enumerates the
// fixtures at compile time and emits one `#[test]` per fixture that forwards
// into `run_wasm_wasi_parity_fixture`. The allowlists and exact-count
// guards are kept as a separate aggregate test (`wasm_wasi_parity_allowlist_guard`).
// ---------------------------------------------------------------------------

/// Stems that need special wasmtime args (env injection, dir access) or are
/// covered by a different test profile.
const WASI_SKIP_STEMS: &[&str] = &[
    "wasm_wasi_env",                 // needs --env
    "wasm_wasi_file_io",             // needs --dir, creates temp files
    "wasm_wasi_exists",              // needs --dir, creates temp files
    "wasm_wasi_write_failure",       // needs --dir
    "wasm_wasi_write_failure_shape", // needs --dir
    "wasm_edge_env",                 // wasm-edge profile, needs taida_host imports
    "net_http_hello",                // server blocks on httpServe waiting for connections
];

/// Examples that wasm-wasi cannot compile (unsupported features).
/// If this list shrinks, update the count in the aggregate guard — that's progress.
const WASI_EXPECTED_REJECTED: &[&str] = &[
    "net_http_parse_encode", // net package import cannot resolve in standalone wasm compile
];

/// Examples where the native backend itself fails.
const WASI_EXPECTED_NATIVE_FAIL: &[&str] = &[
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

/// Known pre-existing parity diffs — expected to fail but not a regression.
/// Kept for symmetry with the other runners; currently empty (Bug A-G
/// + Reverse fix closed all known diffs).
const WASI_EXPECTED_PARITY_DIFF: &[&str] = &[];

/// Run the parity check for a single fixture stem. Used by the per-fixture
/// `#[test]` functions generated in `$OUT_DIR/examples_all_td_tests.rs`.
fn run_wasm_wasi_parity_fixture(stem: &str) {
    // Fixtures needing wasmtime args are covered by other tests.
    if WASI_SKIP_STEMS.contains(&stem) {
        return;
    }

    let Some(wasmtime) = wasmtime_bin() else {
        eprintln!("wasmtime not found, skipping wasm-wasi parity for {}", stem);
        return;
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(format!("{}.td", stem));

    // Try native build first.
    let native_out = match run_native(&td_path) {
        Some(s) => s,
        None => {
            if WASI_EXPECTED_NATIVE_FAIL.contains(&stem) {
                return; // Documented native failure.
            }
            panic!(
                "WW-3 REGRESSION: native backend unexpectedly failed for {}. \
                 If this is now a real native failure, add to WASI_EXPECTED_NATIVE_FAIL.",
                stem
            );
        }
    };

    // Try wasm-wasi compile + run. Cache the .wasm so superset tests can reuse it.
    let parity_wasm_path = std::env::temp_dir().join(format!("taida_ww3_parity_{}.wasm", stem));
    let compile_output = Command::new(taida_bin())
        .args(["build", "--target", "wasm-wasi"])
        .arg(&td_path)
        .arg("-o")
        .arg(&parity_wasm_path)
        .output()
        .ok();
    let wasm_output = compile_output.and_then(|co| {
        if !co.status.success() {
            return None;
        }
        cache_wasm("wasm-wasi", stem, &parity_wasm_path);
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

    let wasm_out = match wasm_output {
        Some(s) => s,
        None => {
            if WASI_EXPECTED_REJECTED.contains(&stem) {
                return; // Documented rejection.
            }
            panic!(
                "WW-3 REGRESSION: wasm-wasi unexpectedly could not compile/run {}. \
                 If this is now a real regression, add to WASI_EXPECTED_REJECTED.",
                stem
            );
        }
    };

    if native_out != wasm_out {
        if WASI_EXPECTED_PARITY_DIFF.contains(&stem) {
            return; // Documented diff.
        }
        panic!(
            "WW-3 PARITY FAILED for {}: native='{}' vs wasm-wasi='{}'",
            stem,
            native_out.chars().take(200).collect::<String>(),
            wasm_out.chars().take(200).collect::<String>(),
        );
    }
}

/// Aggregate regression guard: verifies that the allowlists and expected
/// parity count remain consistent with the fixture set on disk.
///
/// This test does not re-run any fixtures — it only inspects the static
/// fixture list and allowlists. The per-fixture tests generated from
/// `ALL_TD_FIXTURES` (see `$OUT_DIR/examples_all_td_tests.rs`) enforce the
/// actual parity check. Keeping the aggregate count in a separate, cheap
/// test ensures we catch silent drift: if a new fixture is added and
/// happens to parity-match, the count here must be updated deliberately.
#[test]
fn wasm_wasi_parity_allowlist_guard() {
    use common::fixture_lists::ALL_TD_FIXTURES;

    let all = ALL_TD_FIXTURES;

    // Guard: every entry in the allowlists must exist in the fixture set.
    for stem in WASI_SKIP_STEMS
        .iter()
        .chain(WASI_EXPECTED_REJECTED)
        .chain(WASI_EXPECTED_NATIVE_FAIL)
        .chain(WASI_EXPECTED_PARITY_DIFF)
    {
        assert!(
            all.contains(stem),
            "WW-3: allowlist references unknown fixture `{}`; check spelling or remove from list",
            stem
        );
    }

    // Exact expected parity count = all fixtures
    //   - ones we skip (runtime-arg dependent)
    //   - ones wasm-wasi rejects (documented)
    //   - ones native rejects (documented)
    //   - ones with known diff (documented)
    let expected_parity_ok = all.len()
        - WASI_SKIP_STEMS.len()
        - WASI_EXPECTED_REJECTED.len()
        - WASI_EXPECTED_NATIVE_FAIL.len()
        - WASI_EXPECTED_PARITY_DIFF.len();

    // Historical target count (WC-4 through C13-1): 71
    // If the fixture set changes and parity improves, update here deliberately.
    assert_eq!(
        expected_parity_ok,
        71,
        "WW-3: parity-OK count drift — got {} = |fixtures {}| - |skip {}| - |rejected {}| - \
         |native_fail {}| - |diff {}|. Expected 71. Update this constant deliberately.",
        expected_parity_ok,
        all.len(),
        WASI_SKIP_STEMS.len(),
        WASI_EXPECTED_REJECTED.len(),
        WASI_EXPECTED_NATIVE_FAIL.len(),
        WASI_EXPECTED_PARITY_DIFF.len(),
    );
}

// Per-fixture tests: build.rs emits one `#[test] fn fixture_all_td_<stem>() { ... }`
// per fixture stem in `ALL_TD_FIXTURES`. The macro forwards to our runner.
macro_rules! c24_fixture_runner {
    ($stem:expr) => {
        run_wasm_wasi_parity_fixture($stem)
    };
}
include!(concat!(env!("OUT_DIR"), "/examples_all_td_tests.rs"));

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
    // B11-2f: stdout convert_to_string: 60 → 62 (compile_b11_features, compile_hof_molds)
    // B11-11c: compile_b11_2f_stdout regression fixture added (62 → 63)
    // C12-1e: compile_c12_1_tag_table regression fixture added (63 → 64)
    // C12-3d: compile_c12_3_mutual_tail added (64 → 65)
    // C12-5: compile_c12_5_side_effect_returns added (65 → 66)
    // C12-4c: compile_c12_4_arm_pure_expr added (66 → 67)
    // C12-11: compile_c12_11_tag_prop added (67 → 68)
    // C12B-034: compile_c12b_034_wasm_nonbool_param added (68 → 69)
    // C13-1: compile_c13_1_tail_bind added (69 → 70)
    assert_eq!(
        superset_ok, 70,
        "WW-3: Expected exactly 70 superset-verified examples, got {}. \
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

// ── C12B-023: Regex on wasm-wasi must produce compile error ──────────
//
// PHILOSOPHY I (silent-undefined 禁止): even wasm-wasi shares the
// runtime_core_wasm Regex stubs; construction + match/search must be
// rejected at compile time with `[E1617]`.

fn assert_wasi_regex_rejected(stem: &str, source: &str, candidates: &[&str]) {
    let td_path = std::env::temp_dir().join(format!("taida_c12b_023_wasi_{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("taida_c12b_023_wasi_{}.wasm", stem));
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-wasi")
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
        "C12B-023: wasm-wasi should reject Regex usage, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("[E1617]"),
        "C12B-023: wasm-wasi Regex rejection must emit [E1617], got: {}",
        stderr
    );
    assert!(
        candidates.iter().any(|l| stderr.contains(l)),
        "C12B-023: wasm-wasi [E1617] message should mention one of {:?}, got: {}",
        candidates,
        stderr
    );
}

#[test]
fn test_c12b_023_wasm_wasi_rejects_regex_ctor() {
    assert_wasi_regex_rejected(
        "ctor",
        "re <= Regex(\"\\\\d+\", \"\")\nstdout(\"built\")\n",
        &["Regex"],
    );
}

#[test]
fn test_c12b_023_wasm_wasi_rejects_str_match() {
    assert_wasi_regex_rejected(
        "match",
        "re <= Regex(\"\\\\d+\", \"\")\ns <= \"abc 123\"\nresult <= s.match(re)\nstdout(result)\n",
        &["Regex", "Str.match"],
    );
}

#[test]
fn test_c12b_023_wasm_wasi_rejects_str_search() {
    assert_wasi_regex_rejected(
        "search",
        "re <= Regex(\"\\\\d+\", \"\")\ns <= \"abc 123\"\ni <= s.search(re)\nstdout(i)\n",
        &["Regex", "Str.search"],
    );
}

// ── C12B-023 bypass closure (2026-04-15 external review fix) ─────────
//
// Reviewer reproduction code + adjacent `_poly` entrypoints; pin that
// wasm-wasi rejects the manual-pack path at type-check time too.

#[test]
fn test_c12b_023_wasm_wasi_rejects_manual_pack_replaceall() {
    assert_wasi_regex_rejected(
        "bypass_replaceall",
        "main =\n  re <= @(__type <= \"Regex\", pattern <= \"a\", flags <= \"\")\n  stdout(\"aba\".replaceAll(re, \"x\"))\n",
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_wasi_rejects_manual_pack_match() {
    assert_wasi_regex_rejected(
        "bypass_match",
        "re <= @(__type <= \"Regex\", pattern <= \"a\", flags <= \"\")\nstdout(\"abc\".match(re))\n",
        &["reserved for compiler-internal use"],
    );
}

// C12B-023 root fix (2026-04-15 v2): indirect bypass routes.

#[test]
fn test_c12b_023_wasm_wasi_rejects_variable_bound_tag() {
    assert_wasi_regex_rejected(
        "bypass_var_tag",
        "main =\n  tag <= \"Regex\"\n  re <= @(__type <= tag, pattern <= \"a\", flags <= \"\")\n  stdout(\"aba\".replaceAll(re, \"x\"))\n",
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_wasi_rejects_concat_tag() {
    assert_wasi_regex_rejected(
        "bypass_concat",
        "re <= @(__type <= \"Re\" + \"gex\", pattern <= \"a\", flags <= \"\")\nstdout(\"aba\".replaceAll(re, \"x\"))\n",
        &["reserved for compiler-internal use"],
    );
}

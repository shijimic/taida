/// Integration tests for wasm-min backend.
///
/// Compiles .td files to .wasm via `taida build --target wasm-min`,
/// runs them with wasmtime, and verifies output matches the interpreter.
///
/// WC-7d: Size gate CI tests — hard gates on .wasm file sizes.
/// Prelude-complete baselines (WC-1~WC-6): hello = 321 bytes, pi_approx = 6,736 bytes.
/// Gate: hello <= 512 bytes, pi <= 8,192 bytes.
///
/// RC-8b: Parity tests save compiled .wasm files to `target/wasm-test-cache/wasm-min/`
/// so superset tests in wasm_wasi.rs can reuse them without recompiling.
mod common;

use common::{cache_wasm, run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

/// AT-7: Require wasmtime or skip with clear visibility.
///
/// In CI (when `CI` env var is set), panics if wasmtime is not found,
/// ensuring wasm tests are never silently skipped in CI.
/// Locally, returns None so tests are skipped with an eprintln message.
fn require_wasmtime() -> Option<PathBuf> {
    match wasmtime_bin() {
        Some(p) => Some(p),
        None => {
            if std::env::var("CI").is_ok() {
                panic!(
                    "AT-7: wasmtime not found in CI environment. \
                     Install wasmtime in ci.yml or wasm-min runtime tests are never verified."
                );
            }
            eprintln!("SKIP: wasmtime not found, skipping wasm-min runtime test");
            None
        }
    }
}

/// Compile a .td file to wasm-min and run with wasmtime.
/// RC-8b: Also caches the compiled .wasm for superset test reuse.
fn compile_and_run_wasm(td_path: &Path, wasmtime: &Path) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let wasm_path = std::env::temp_dir().join(format!("taida_wasm_test_{}.wasm", stem));

    // Compile
    let compile_output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-min")
        .arg(td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .ok()?;

    if !compile_output.status.success() {
        let stderr = String::from_utf8_lossy(&compile_output.stderr);
        eprintln!(
            "wasm-min compile failed for {}: {}",
            td_path.display(),
            stderr
        );
        return None;
    }

    // RC-8b: Cache the compiled .wasm for superset tests
    cache_wasm("wasm-min", &stem, &wasm_path);

    // Run with wasmtime
    let run_output = Command::new(wasmtime).arg(&wasm_path).output().ok()?;

    // Clean up
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

/// Write source to a temp file, compile to wasm-min and run, return stdout.
fn compile_and_run_wasm_src(source: &str, wasmtime: &Path, label: &str) -> Option<String> {
    let td_path = std::env::temp_dir().join(format!("taida_wasm_src_{}.td", label));
    let wasm_path = std::env::temp_dir().join(format!("taida_wasm_src_{}.wasm", label));
    std::fs::write(&td_path, source).ok()?;

    let compile_output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-min")
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .ok()?;

    let _ = std::fs::remove_file(&td_path);

    if !compile_output.status.success() {
        let stderr = String::from_utf8_lossy(&compile_output.stderr);
        eprintln!("wasm-min compile failed for {}: {}", label, stderr);
        return None;
    }

    let run_output = Command::new(wasmtime).arg(&wasm_path).output().ok()?;
    let _ = std::fs::remove_file(&wasm_path);

    if !run_output.status.success() {
        let stderr = String::from_utf8_lossy(&run_output.stderr);
        eprintln!("wasmtime execution failed for {}: {}", label, stderr);
        return None;
    }

    Some(
        String::from_utf8_lossy(&run_output.stdout)
            .trim_end()
            .to_string(),
    )
}

/// Run source with interpreter, return stdout.
fn run_interpreter_src(source: &str, label: &str) -> Option<String> {
    let td_path = std::env::temp_dir().join(format!("taida_interp_src_{}.td", label));
    std::fs::write(&td_path, source).ok()?;
    let result = run_interpreter(&td_path);
    let _ = std::fs::remove_file(&td_path);
    result
}

#[test]
fn wasm_min_hello() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(interp, wasm, "wasm-min output should match interpreter");
}

#[test]
fn wasm_min_pi_approx() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_pi_approx.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(interp, wasm, "wasm-min output should match interpreter");
}

// ---------------------------------------------------------------------------
// WC-7d: Size Gate CI — hard gates on .wasm file sizes
//
// Baselines (prelude-complete core, WC-1 through WC-6):
//   hello = 321 bytes, pi_approx = 6,736 bytes
//
// wasm-ld --gc-sections prunes unused functions, so hello (which uses only
// stdout + int-to-string) stays tiny even though core now contains all
// prelude functions. pi_approx pulls in more of core (float formatting,
// string concat, etc.) but is still well under 10KB.
//
// Gate values include ~60% headroom (hello) / ~22% headroom (pi) above
// current baselines to allow minor growth without breaking CI.
// ---------------------------------------------------------------------------

/// Helper: compile a .td to .wasm and return the file size in bytes.
fn compile_wasm_and_get_size(td_path: &Path, wasm_path: &Path) -> u64 {
    let output = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min"])
        .arg(td_path)
        .arg("-o")
        .arg(wasm_path)
        .output()
        .expect("failed to run taida");
    assert!(
        output.status.success(),
        "wasm-min compile failed for {}: {}",
        td_path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let size = std::fs::metadata(wasm_path).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(wasm_path);
    size
}

#[test]
fn wasm_min_size_gate() {
    let hello_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");
    let pi_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_pi_approx.td");

    let hello_size = compile_wasm_and_get_size(
        &hello_path,
        &std::env::temp_dir().join("taida_wasm_size_hello.wasm"),
    );
    let pi_size = compile_wasm_and_get_size(
        &pi_path,
        &std::env::temp_dir().join("taida_wasm_size_pi.wasm"),
    );

    // WC-7d baselines (prelude-complete core): hello = 321 bytes, pi = 6,736 bytes
    eprintln!(
        "wasm-min hello size: {} bytes (WC-7d baseline: 321)",
        hello_size
    );
    eprintln!(
        "wasm-min pi size: {} bytes (WC-7d baseline: 6,736)",
        pi_size
    );

    // Hard gate: hello must be <= 512 bytes (~60% headroom above 321 baseline)
    assert!(
        hello_size > 0 && hello_size <= 512,
        "HARD GATE FAIL: hello.wasm should be <= 512 bytes (WC-7d gate), got {} bytes",
        hello_size
    );

    // Hard gate: pi_approx must be <= 8,192 bytes (~22% headroom above 6,736 baseline)
    assert!(
        pi_size > 0 && pi_size <= 8192,
        "HARD GATE FAIL: pi.wasm should be <= 8,192 bytes (WC-7d gate), got {} bytes. \
         Prelude-complete core baseline is 6,736 bytes.",
        pi_size
    );

    // Report exact values for tracking
    eprintln!(
        "Size gate passed: hello={} (gate: 512), pi={} (gate: 8,192)",
        hello_size, pi_size
    );
}

/// WC-7d: Size gate with exact baseline comparison — reports the ratio.
#[test]
fn wasm_min_size_gate_wado_comparison() {
    let hello_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_hello.td");
    let pi_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm_min_pi_approx.td");

    let hello_size = compile_wasm_and_get_size(
        &hello_path,
        &std::env::temp_dir().join("taida_wasm_wado_hello.wasm"),
    );
    let pi_size = compile_wasm_and_get_size(
        &pi_path,
        &std::env::temp_dir().join("taida_wasm_wado_pi.wasm"),
    );

    // Report Wado comparison ratios
    let hello_ratio = 1572.0 / hello_size as f64;
    let pi_ratio = 9269.0 / pi_size as f64;
    eprintln!(
        "Wado comparison: hello = {} bytes (Wado: 1,572 = {:.1}x larger), \
         pi = {} bytes (Wado: 9,269 = {:.1}x larger)",
        hello_size, hello_ratio, pi_size, pi_ratio
    );

    // Sanity: both should be non-zero and compile correctly
    assert!(hello_size > 0, "hello.wasm should not be empty");
    assert!(pi_size > 0, "pi.wasm should not be empty");
}

// W-4: BuchiPack is now supported in wasm-min.
// wasm_min_reject_buchipack removed -- see wasm_min_pack_basic instead.

/// W-4: Basic BuchiPack support — create, field access, stdout.
#[test]
fn wasm_min_pack_basic() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/pack_basic.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "pack_basic: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-4: Nested BuchiPack — inner pack accessed through outer pack fields.
#[test]
fn wasm_min_pack_nested() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/pack_nested.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "pack_nested: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-4: BuchiPack compile acceptance test (was wasm_min_reject_buchipack before W-4).
#[test]
fn wasm_min_pack_accepted() {
    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/unsupported_pack.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_pack_accepted.wasm");

    let output = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida");

    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        output.status.success(),
        "W-4: BuchiPack should now be accepted by wasm-min, but compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// W-5: wasm_min_reject_closure removed -- Closures are now supported.
// See wasm_min_closure_basic, wasm_min_closure_accepted instead.

/// W-5: Closures should now be accepted by wasm-min.
#[test]
fn wasm_min_closure_accepted() {
    let td_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm_min/unsupported_closure.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_closure_accepted.wasm");

    let output = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida");

    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        output.status.success(),
        "W-5: Closures should now be accepted by wasm-min, but compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// W-3: wasm_min_reject_float removed -- Float is now supported.
// See wasm_min_float_accepted test instead.

// ---------------------------------------------------------------------------
// W-2: Regression tests (from review Low findings)
// ---------------------------------------------------------------------------

/// W-2: Multiple global variables should each get their own C static variable.
/// Verifies that globals do not collide (F-4 regression).
#[test]
fn wasm_min_multiple_globals() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/multiple_globals.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "multiple globals: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-2: --release gate should work with wasm-min (positive case: no TODO/Stub).
#[test]
fn wasm_min_release_gate_positive() {
    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/release_gate.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_release_pos.wasm");

    // Compiling with --release should succeed (no TODO/Stub in source)
    let output = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min", "--release"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida");

    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        output.status.success(),
        "wasm-min --release should succeed for clean code, but failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// W-3: Float support tests
// ---------------------------------------------------------------------------

/// W-3: Float literals and debug(Float) should work.
#[test]
fn wasm_min_float_basic() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/float_basic.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "float_basic: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-3: Float arithmetic (add, sub, mul) should work.
#[test]
fn wasm_min_float_arith() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/float_arith.td");
    // AT-8: Compare against interpreter (reference implementation), not native.
    // Float formatting may differ between interpreter and compiled backends;
    // if this test fails, it indicates a formatting parity issue to be tracked.
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "float_arith: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-3: String concatenation should work (requires bump allocator).
#[test]
fn wasm_min_str_ops() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/str_ops.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "str_ops: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-3: Float is no longer rejected (was rejected in wasm-min v1).
#[test]
fn wasm_min_float_accepted() {
    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/unsupported_float.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_float_accepted.wasm");

    let output = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida");

    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        output.status.success(),
        "W-3: Float should now be accepted by wasm-min, but compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// W-2: --release gate should reject TODO molds in wasm-min (negative case).
#[test]
fn wasm_min_release_gate_negative() {
    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/release_gate_todo.td");
    let wasm_path = std::env::temp_dir().join("taida_wasm_release_neg.wasm");

    // Compiling with --release should fail (TODO mold present)
    let output = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min", "--release"])
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida");

    let _ = std::fs::remove_file(&wasm_path);

    assert!(
        !output.status.success(),
        "wasm-min --release should fail when TODO mold is present, but it succeeded"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Release gate failed"),
        "Error should contain 'Release gate failed', got: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// W-3f: Blocker regression tests
// ---------------------------------------------------------------------------

/// W-3f F-1: debug(Float) should handle small non-zero values (scientific notation).
/// AT-8: Compare against interpreter (reference implementation).
#[test]
fn wasm_min_float_small_values() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/float_small.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "float_small: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-3f F-2: String.length() should work via taida_polymorphic_length.
#[test]
fn wasm_min_str_length() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/str_length.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "str_length: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-3f F-2: Int.toString() should work via taida_polymorphic_to_string.
#[test]
fn wasm_min_int_to_string() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/int_to_string.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "int_to_string: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-3f F-2: Int["str"]() ]=> should work via taida_int_mold_str + taida_generic_unmold.
#[test]
fn wasm_min_int_mold_str() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/int_mold_str.td");
    // AT-8: Compare against interpreter (reference implementation), not native.
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "int_mold_str: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

// ---------------------------------------------------------------------------
// W-4: Collection type tests
// ---------------------------------------------------------------------------

/// W-4: Basic list support — create, push, length.
#[test]
fn wasm_min_list_basic() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/list_basic.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "list_basic: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

// ---------------------------------------------------------------------------
// W-4f: HashMap/Set integration tests (F-2)
// ---------------------------------------------------------------------------

/// W-4f F-2: HashMap basic operations — new, set, has, size, keys, values, remove, merge, entries.
#[test]
fn wasm_min_hashmap_basic() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/hashmap_basic.td");
    // AT-8: Compare against interpreter (reference implementation), not native.
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "hashmap_basic: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-4f F-2: Set basic operations — setOf, size, add, has, remove, union, intersect, diff, toList.
#[test]
fn wasm_min_set_basic() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/set_basic.td");
    // AT-8: Compare against interpreter (reference implementation), not native.
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "set_basic: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

// ---------------------------------------------------------------------------
// W-5: Control flow and function tests
// ---------------------------------------------------------------------------

/// W-5: Basic closure — function returning a function with captured variable.
#[test]
fn wasm_min_closure_basic() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/closure_basic.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "closure_basic: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-5: Error ceiling — error handling with |== operator.
#[test]
fn wasm_min_error_ceiling() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/error_ceiling.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "error_ceiling: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-5: Lax[T] — Div/Mod molds return Lax, ]=> unmolds.
#[test]
fn wasm_min_lax_basic() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/lax_basic.td");
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "lax_basic: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

// ---------------------------------------------------------------------------
// W-5f: Lax/Result toString and Result basic tests
// ---------------------------------------------------------------------------

/// W-5f F-2: Lax.toString() — "Lax(42)", "Lax(default: 0)", "Lax(3)".
#[test]
fn wasm_min_lax_tostring() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/lax_tostring.td");
    // AT-8: Compare against interpreter (reference implementation), not native.
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "lax_tostring: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-5f F-1/F-3: Result basic — create, unmold, toString.
#[test]
fn wasm_min_result_basic() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/result_basic.td");
    // AT-8: Compare against interpreter (reference implementation), not native.
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "result_basic: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-5g F-1: Result with predicate — predicate-fail should be error, predicate-pass should succeed.
#[test]
fn wasm_min_result_predicate() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/result_predicate.td");
    // AT-8: Compare against interpreter (reference implementation), not native.
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "result_predicate: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

/// W-5g F-2: Str->Float/Bool mold failure should return empty Lax (not success Lax(0)).
#[test]
fn wasm_min_mold_fail() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let td_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm_min/mold_fail.td");
    // AT-8: Compare against interpreter (reference implementation), not native.
    let interp = run_interpreter(&td_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm(&td_path, &wasmtime).expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "mold_fail: wasm-min output should match interpreter (expected '{}', got '{}')",
        interp, wasm
    );
}

// ---------------------------------------------------------------------------
// W-6: Parity test — all compilable examples must match native output
// ---------------------------------------------------------------------------

/// W-6: Comprehensive parity test.
/// AT-8: Compare against interpreter (reference implementation), not native.
/// For every .td file in examples/ that successfully compiles with wasm-min,
/// the wasm output must match the interpreter output exactly.
#[test]
fn wasm_min_parity_all_examples() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let examples_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut td_files: Vec<_> = std::fs::read_dir(&examples_dir)
        .expect("examples/ directory should exist")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "td"))
        .collect();
    td_files.sort();

    let mut parity_ok = Vec::new();
    let mut parity_fail = Vec::new();
    let mut compile_rejected = Vec::new();
    let mut interp_fail = Vec::new();

    for td_path in &td_files {
        let stem = td_path.file_stem().unwrap().to_string_lossy().to_string();

        // Try interpreter first (reference implementation)
        let interp_output = run_interpreter(td_path);
        if interp_output.is_none() {
            interp_fail.push(stem.clone());
            continue;
        }
        let interp_out = interp_output.unwrap();

        // Try wasm-min compile + run
        let wasm_output = compile_and_run_wasm(td_path, &wasmtime);
        if wasm_output.is_none() {
            compile_rejected.push(stem.clone());
            continue;
        }
        let wasm_out = wasm_output.unwrap();

        if interp_out == wasm_out {
            parity_ok.push(stem.clone());
        } else {
            parity_fail.push((stem.clone(), interp_out, wasm_out));
        }
    }

    eprintln!(
        "W-6 Parity: {} OK, {} rejected, {} interp-fail",
        parity_ok.len(),
        compile_rejected.len(),
        interp_fail.len()
    );

    // WC-3/WC-6: Known parity diffs -- examples that compile but have known
    // behavioral differences. These are excluded from parity failure.
    // All 9 previously known diffs have been fixed:
    //   - Bug A: taida_int_mold_str now returns Lax (not raw int)
    //   - Bug B: taida_list_get now returns Lax (not raw value)
    //   - Bug C: taida_hashmap_get_lax now returns Lax (not raw value)
    //   - Bug D: taida_polymorphic_is_empty now handles Lax
    //   - Bug E: Error toString via _wasm_throw_to_display_string + result_map_error
    //   - Bug F: RelaxedGorillax type detection (removed WASM_MIN_HEAP_ADDR check)
    //   - Bug G: Sort[](by <= fn) now lowered to taida_list_sort_by
    //   - Reverse: Removed from string-returning molds in lower.rs
    let expected_parity_diff: Vec<&str> = vec![];

    // Filter out expected diffs
    let unexpected_parity_fail: Vec<_> = parity_fail
        .iter()
        .filter(|(stem, _, _)| !expected_parity_diff.contains(&stem.as_str()))
        .collect();

    if !unexpected_parity_fail.is_empty() {
        let mut msg = format!(
            "W-6 PARITY FAILED for {} example(s):\n",
            unexpected_parity_fail.len()
        );
        for (stem, interp, wasm) in &unexpected_parity_fail {
            msg.push_str(&format!(
                "\n  {}: interp='{}' vs wasm='{}'\n",
                stem,
                interp.chars().take(100).collect::<String>(),
                wasm.chars().take(100).collect::<String>()
            ));
        }
        panic!("{}", msg);
    }

    if !parity_fail.is_empty() {
        eprintln!(
            "W-6 Parity: {} expected-diff examples skipped: {:?}",
            parity_fail.len(),
            parity_fail
                .iter()
                .map(|(s, _, _)| s.as_str())
                .collect::<Vec<_>>()
        );
    }

    // WC-7a: Exact parity count — update deliberately when parity improves.
    // 61 = 58 (prev) + 3 (PR-3: module inlining: 09_modules, compile_module, compile_module_value)
    // 63 = 61 + 2 (B11-2f: stdout convert_to_string path — compile_b11_features, compile_hof_molds)
    // 64 = 63 + 1 (B11-11c: compile_b11_2f_stdout regression fixture)
    // 65 = 64 + 1 (C12-1e: compile_c12_1_tag_table regression fixture)
    // 66 = 65 + 1 (C12-3d: compile_c12_3_mutual_tail — tail-only mutual recursion)
    // 67 = 66 + 1 (C12-5: compile_c12_5_side_effect_returns — stdout Int return)
    // 68 = 67 + 1 (C12-4c: compile_c12_4_arm_pure_expr — `| |>` pure-expr boundary)
    // 69 = 68 + 1 (C12-11: compile_c12_11_tag_prop — param_tag_vars Bool prop)
    // 70 = 69 + 1 (C12B-034: compile_c12b_034_wasm_nonbool_param — memory-safe non-Bool through param)
    // 71 = 70 + 1 (C13-1: compile_c13_1_tail_bind — tail-binding semantics)
    assert_eq!(
        parity_ok.len(),
        71,
        "WC-7: Expected exactly 71 parity-OK examples, got {}. \
         If parity improved, update the expected count. List: {:?}",
        parity_ok.len(),
        parity_ok
    );
}

// ── RC-1D-ii: WASM-specific module inline quality tests ─────────────────
//
// These tests verify that the WASM module inliner correctly handles
// multi-module scenarios that previously had bugs (symbol collision, etc).

/// Compile a multi-module .td project to wasm-min and run with wasmtime.
/// Uses a unique output path to avoid parallel test collisions on .o files.
fn compile_and_run_wasm_project(td_path: &Path, wasmtime: &Path, label: &str) -> Option<String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let wasm_path = std::env::temp_dir().join(format!("taida_wasm_proj_{}_{}.wasm", label, nanos));

    let compile_output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-min")
        .arg(td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .ok()?;

    if !compile_output.status.success() {
        let stderr = String::from_utf8_lossy(&compile_output.stderr);
        eprintln!(
            "wasm-min compile failed for {} ({}): {}",
            td_path.display(),
            label,
            stderr
        );
        return None;
    }

    let run_output = Command::new(wasmtime).arg(&wasm_path).output().ok()?;
    let _ = std::fs::remove_file(&wasm_path);
    // Also clean up intermediate .o files
    let tmp_base = wasm_path.with_extension("_wasm_tmp");
    let _ = std::fs::remove_file(tmp_base.with_extension("gen.o"));
    let _ = std::fs::remove_file(tmp_base.with_extension("gen.c"));
    let _ = std::fs::remove_file(tmp_base.with_extension("rt.o"));
    let _ = std::fs::remove_file(tmp_base.with_extension("rt.c"));
    let _ = std::fs::remove_dir_all(tmp_base.with_extension("include"));

    if !run_output.status.success() {
        let stderr = String::from_utf8_lossy(&run_output.stderr);
        eprintln!(
            "wasmtime execution failed for {} ({}): {}",
            td_path.display(),
            label,
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

/// RC-1o/WASM: Symbol collision test — two modules with identically-named
/// internal functions (_helper) must not collide in the WASM flat namespace.
/// This was a real bug fixed in RC-1o: non-exported functions were not
/// namespaced with module_key, causing mod_b._helper to overwrite mod_a._helper.
#[test]
fn wasm_min_rc1o_symbol_collision() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("taida_wasm_rc1o_{}_{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir).expect("create temp dir");

    std::fs::write(
        dir.join("mod_a.td"),
        r#"_helper x = x * 2 => :Int
compute x = _helper(x) + 1 => :Int
<<< @(compute)
"#,
    )
    .expect("write mod_a");

    std::fs::write(
        dir.join("mod_b.td"),
        r#"_helper x = x * 3 => :Int
compute x = _helper(x) + 2 => :Int
<<< @(compute)
"#,
    )
    .expect("write mod_b");

    std::fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a.td => @(compute => computeA)
>>> ./mod_b.td => @(compute => computeB)
stdout(computeA(5).toString())
stdout(computeB(5).toString())
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm_project(&main_path, &wasmtime, "rc1o_collision")
        .expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "RC-1o/WASM: symbol collision — interp='{}', wasm='{}'",
        interp, wasm
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// RC-1m/WASM: Deep nested dependency (4 levels) with string operations.
/// Verifies WASM module inliner handles 4+ levels of transitive dependencies.
#[test]
fn wasm_min_rc1m_deep_nested_four_levels() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("taida_wasm_rc1m_{}_{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir).expect("create temp dir");

    std::fs::write(
        dir.join("mod_d.td"),
        r#"_internal_d x = "D(" + x + ")" => :Str
wrap_d x = _internal_d(x) => :Str
<<< @(wrap_d)
"#,
    )
    .expect("write mod_d");

    std::fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d.td => @(wrap_d)
_internal_c x = "C(" + wrap_d(x) + ")" => :Str
wrap_c x = _internal_c(x) => :Str
<<< @(wrap_c)
"#,
    )
    .expect("write mod_c");

    std::fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_c.td => @(wrap_c)
_internal_b x = "B(" + wrap_c(x) + ")" => :Str
wrap_b x = _internal_b(x) => :Str
<<< @(wrap_b)
"#,
    )
    .expect("write mod_b");

    std::fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b.td => @(wrap_b)
wrap_a x = "A(" + wrap_b(x) + ")" => :Str
<<< @(wrap_a)
"#,
    )
    .expect("write mod_a");

    std::fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a.td => @(wrap_a)
stdout(wrap_a("hello"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm_project(&main_path, &wasmtime, "rc1m_deep")
        .expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "RC-1m/WASM: deep nested — interp='{}', wasm='{}'",
        interp, wasm
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// RC-1n/WASM: Diamond dependency with private helpers.
/// B and C both import D. Private helpers must not collide.
#[test]
fn wasm_min_rc1n_diamond_dependency() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("taida_wasm_rc1n_{}_{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir).expect("create temp dir");

    std::fs::write(
        dir.join("mod_d.td"),
        r#"_secret x = "[" + x + "]" => :Str
shared x = "D:" + _secret(x) => :Str
<<< @(shared)
"#,
    )
    .expect("write mod_d");

    std::fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_d.td => @(shared)
_b_helper x = shared(x) => :Str
fromB x = "B:" + _b_helper(x) => :Str
<<< @(fromB)
"#,
    )
    .expect("write mod_b");

    std::fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d.td => @(shared)
_c_helper x = shared(x) => :Str
fromC x = "C:" + _c_helper(x) => :Str
<<< @(fromC)
"#,
    )
    .expect("write mod_c");

    std::fs::write(
        dir.join("main.td"),
        r#">>> ./mod_b.td => @(fromB)
>>> ./mod_c.td => @(fromC)
stdout(fromB("x"))
stdout(fromC("y"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm_project(&main_path, &wasmtime, "rc1n_diamond")
        .expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "RC-1n/WASM: diamond — interp='{}', wasm='{}'",
        interp, wasm
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// RCB-43/WASM: Diamond dependency with different symbols from shared module.
/// B imports funcX from D, C imports funcY from D (different symbol sets).
/// This exercises the diff-symbol path in inline_wasm_module_imports, where
/// the second encounter of D must use the cached IR instead of re-parsing.
#[test]
fn wasm_min_rcb43_diamond_different_symbols() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };

    let dir = unique_temp_dir("taida_wasm_rcb43");

    // D exports two distinct functions: funcX and funcY
    std::fs::write(
        dir.join("mod_d.td"),
        r#"funcX x = "X:" + x => :Str
funcY y = "Y:" + y => :Str
<<< @(funcX, funcY)
"#,
    )
    .expect("write mod_d");

    // B imports only funcX from D
    std::fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_d.td => @(funcX)
fromB x = "B(" + funcX(x) + ")" => :Str
<<< @(fromB)
"#,
    )
    .expect("write mod_b");

    // C imports only funcY from D (different symbol from B)
    std::fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d.td => @(funcY)
fromC y = "C(" + funcY(y) + ")" => :Str
<<< @(fromC)
"#,
    )
    .expect("write mod_c");

    std::fs::write(
        dir.join("main.td"),
        r#">>> ./mod_b.td => @(fromB)
>>> ./mod_c.td => @(fromC)
stdout(fromB("hello"))
stdout(fromC("world"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("interpreter should succeed");
    let wasm = compile_and_run_wasm_project(&main_path, &wasmtime, "rcb43_diamond_diff")
        .expect("wasm-min should succeed");

    assert_eq!(
        interp, wasm,
        "RCB-43/WASM: diamond with different symbols — interp='{}', wasm='{}'",
        interp, wasm
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── RC-6: Type Inheritance Soundness (WASM) ─────────────────────────

#[test]
fn rc6_wasm_error_inheritance() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };
    let source = r#"
Error => AppError = @(code: Int)
err <= AppError(type <= "AppError", message <= "test", code <= 42)
Str[err.code]() ]=> code_str
stdout(err.__type + " " + code_str)
"#;
    let interp =
        run_interpreter_src(source, "rc6_wasm_err").expect("RC-6: interpreter should succeed");
    let wasm = compile_and_run_wasm_src(source, &wasmtime, "rc6_wasm_err")
        .expect("RC-6: wasm-min should succeed");
    assert_eq!(interp, wasm, "RC-6/WASM: error inheritance mismatch");
}

#[test]
fn rc6_wasm_error_multilevel() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };
    let source = r#"
Error => AppError = @(app_code: Int)
AppError => ValidationError = @(field_name: Str)
ve <= ValidationError(type <= "VE", message <= "bad", app_code <= 400, field_name <= "email")
Str[ve.app_code]() ]=> ac
stdout(ve.__type + " " + ac + " " + ve.field_name)
"#;
    let interp = run_interpreter_src(source, "rc6_wasm_multi_err")
        .expect("RC-6: interpreter should succeed");
    let wasm = compile_and_run_wasm_src(source, &wasmtime, "rc6_wasm_multi_err")
        .expect("RC-6: wasm-min should succeed");
    assert_eq!(interp, wasm, "RC-6/WASM: multilevel error mismatch");
}

#[test]
fn rc6_wasm_throw_catch() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };
    let source = r#"
Error => L1Error = @(l1: Str)
L1Error => L2Error = @(l2: Str)
catch_fn x =
  |== error: Error =
    "caught(" + error.type + "): " + error.l1 + "+" + error.l2
  => :Str
  L2Error(type <= "L2Error", message <= "deep", l1 <= "one", l2 <= "two").throw()
  "unreachable"
=> :Str
stdout(catch_fn(1))
"#;
    let interp =
        run_interpreter_src(source, "rc6_wasm_throw").expect("RC-6: interpreter should succeed");
    let wasm = compile_and_run_wasm_src(source, &wasmtime, "rc6_wasm_throw")
        .expect("RC-6: wasm-min should succeed");
    assert_eq!(interp, wasm, "RC-6/WASM: throw/catch mismatch");
}

#[test]
fn rc6_wasm_custom_inheritance() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };
    let source = r#"
Vehicle = @(name: Str, speed: Int)
Vehicle => Car = @(doors: Int)
car <= Car(name <= "Sedan", speed <= 120, doors <= 4)
Str[car.speed]() ]=> sp
Str[car.doors]() ]=> dr
stdout(car.__type + " " + car.name + " " + sp + " " + dr)
"#;
    let interp =
        run_interpreter_src(source, "rc6_wasm_custom").expect("RC-6: interpreter should succeed");
    let wasm = compile_and_run_wasm_src(source, &wasmtime, "rc6_wasm_custom")
        .expect("RC-6: wasm-min should succeed");
    assert_eq!(interp, wasm, "RC-6/WASM: custom inheritance mismatch");
}

#[test]
fn rc6_wasm_custom_multilevel() {
    let Some(wasmtime) = require_wasmtime() else {
        return;
    };
    let source = r#"
Shape = @(color: Str)
Shape => Polygon = @(sides: Int)
Polygon => Rectangle = @(width: Int, height: Int)
rect <= Rectangle(color <= "blue", sides <= 4, width <= 10, height <= 5)
Str[rect.sides]() ]=> s
Str[rect.width]() ]=> w
Str[rect.height]() ]=> h
stdout(rect.__type + " " + rect.color + " " + s + " " + w + " " + h)
"#;
    let interp = run_interpreter_src(source, "rc6_wasm_multilevel")
        .expect("RC-6: interpreter should succeed");
    let wasm = compile_and_run_wasm_src(source, &wasmtime, "rc6_wasm_multilevel")
        .expect("RC-6: wasm-min should succeed");
    assert_eq!(interp, wasm, "RC-6/WASM: custom multilevel mismatch");
}

// ── NB-30: Net HTTP API compile error integration tests ──────────────

/// NB-30: httpServe in wasm-min must produce compile error (not silent success).
#[test]
fn test_nb30_wasm_min_net_http_serve_compile_error() {
    let source = r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

httpServe(8080, handler, 1, 1000) ]=> serverResult
stdout(serverResult.ok)
"#;
    let td_path = std::env::temp_dir().join("taida_nb30_serve.td");
    let wasm_path = std::env::temp_dir().join("taida_nb30_serve.wasm");
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-min")
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
        "NB-30: wasm-min should reject httpServe, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("httpServe") || stderr.contains("net"),
        "NB-30: compile error should mention httpServe or net.\nstderr: {}",
        stderr
    );
}

/// NB-30: httpParseRequestHead in wasm-min must produce compile error.
#[test]
fn test_nb30_wasm_min_net_http_parse_compile_error() {
    let source = r#">>> taida-lang/net => @(httpParseRequestHead)
bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
stdout(result.__value.consumed)
"#;
    let td_path = std::env::temp_dir().join("taida_nb30_parse.td");
    let wasm_path = std::env::temp_dir().join("taida_nb30_parse.wasm");
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-min")
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
        "NB-30: wasm-min should reject httpParseRequestHead, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("httpParseRequestHead") || stderr.contains("net"),
        "NB-30: compile error should mention httpParseRequestHead or net.\nstderr: {}",
        stderr
    );
}

/// NB-30: httpEncodeResponse in wasm-min must produce compile error.
#[test]
fn test_nb30_wasm_min_net_http_encode_compile_error() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)
result <= httpEncodeResponse(@(status <= 200, headers <= @[], body <= "Hello"))
stdout(result.__value.kind)
"#;
    let td_path = std::env::temp_dir().join("taida_nb30_encode.td");
    let wasm_path = std::env::temp_dir().join("taida_nb30_encode.wasm");
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-min")
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
        "NB-30: wasm-min should reject httpEncodeResponse, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("httpEncodeResponse") || stderr.contains("net"),
        "NB-30: compile error should mention httpEncodeResponse or net.\nstderr: {}",
        stderr
    );
}

// ── C12B-023: Regex on wasm profiles must produce compile error ──────
//
// PHILOSOPHY I (silent-undefined 禁止): wasm profiles only ship stubs for
// Regex-related runtime helpers. `Regex(...)` construction / `str.match`
// / `str.search` / `str.replace(Regex(...), ...)` all need to be rejected
// at compile time with `[E1617]`.

/// Helper: attempt to build a .td source for a given wasm profile and
/// return the combined stderr + status. Cleans up both .td and .wasm.
fn try_build_wasm(source: &str, stem: &str, target: &str) -> (bool, String) {
    let td_path = std::env::temp_dir().join(format!("taida_c12b_023_{}.td", stem));
    let wasm_path = std::env::temp_dir().join(format!("taida_c12b_023_{}.wasm", stem));
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg(target)
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida build");

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);

    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Assert that the given source is rejected at compile time with `[E1617]`
/// on the given wasm profile. `expected_api_candidates` lists acceptable
/// labels that the diagnostic may mention — in practice `Regex(...)`
/// construction always fires first (since `str.match(re)` / `str.search(re)`
/// require constructing `re` in the first place), so the Regex-ctor message
/// is the normal outcome even for match/search scenarios.
fn assert_regex_rejected(stem: &str, target: &str, source: &str, expected_api_candidates: &[&str]) {
    let (ok, stderr) = try_build_wasm(source, stem, target);
    assert!(
        !ok,
        "C12B-023: {} should reject Regex usage, but compile succeeded.\nstderr: {}",
        target, stderr
    );
    assert!(
        stderr.contains("[E1617]"),
        "C12B-023: {} Regex rejection must emit [E1617], got: {}",
        target,
        stderr
    );
    assert!(
        expected_api_candidates
            .iter()
            .any(|label| stderr.contains(label)),
        "C12B-023: {} [E1617] message should mention one of {:?}, got: {}",
        target,
        expected_api_candidates,
        stderr
    );
}

const C12B_023_SRC_REGEX_CTOR: &str = r#"re <= Regex("\\d+", "")
stdout("built")
"#;

const C12B_023_SRC_MATCH: &str = r#"re <= Regex("\\d+", "")
s <= "abc 123"
result <= s.match(re)
stdout(result)
"#;

const C12B_023_SRC_SEARCH: &str = r#"re <= Regex("\\d+", "")
s <= "abc 123"
i <= s.search(re)
stdout(i)
"#;

const C12B_023_SRC_REPLACE_ALL: &str = r#"re <= Regex("\\d+", "")
s <= "a1 b2 c3"
out <= s.replaceAll(re, "X")
stdout(out)
"#;

#[test]
fn test_c12b_023_wasm_min_rejects_regex_ctor() {
    assert_regex_rejected("min_ctor", "wasm-min", C12B_023_SRC_REGEX_CTOR, &["Regex"]);
}

#[test]
fn test_c12b_023_wasm_min_rejects_str_match() {
    // `s.match(re)` requires constructing `re = Regex(...)` first, so the
    // diagnostic may cite either `Regex` or `Str.match`. Either is correct
    // — both come from the same `[E1617]` path.
    assert_regex_rejected(
        "min_match",
        "wasm-min",
        C12B_023_SRC_MATCH,
        &["Regex", "Str.match"],
    );
}

#[test]
fn test_c12b_023_wasm_min_rejects_str_search() {
    assert_regex_rejected(
        "min_search",
        "wasm-min",
        C12B_023_SRC_SEARCH,
        &["Regex", "Str.search"],
    );
}

#[test]
fn test_c12b_023_wasm_min_rejects_str_replace_all_with_regex() {
    // replaceAll with a Regex argument constructs the Regex first; the
    // Regex(...) ctor itself is the hook that fires [E1617].
    assert_regex_rejected(
        "min_replaceall",
        "wasm-min",
        C12B_023_SRC_REPLACE_ALL,
        &["Regex"],
    );
}

// ── C12B-023 bypass closure (2026-04-15 external review fix) ─────────
//
// External reviewer found that hand-constructing
// `@(__type <= "Regex", pattern <= "a", flags <= "")` and feeding the
// pack to `_poly` dispatchers (replaceAll / replace / split) bypassed
// `validate_regex_api_for_wasm` (since `taida_regex_new` was never
// emitted) and produced a wasm binary with silent UB at runtime. The
// fix rejects manual `__type <= "Regex"` pack literals at type-check
// time with `[E1617]`, closing the vector on *all* backends. Below we
// pin the reviewer's exact repro + the adjacent `_poly` entrypoints.

const C12B_023_BYPASS_REPLACE_ALL: &str = r#"main =
  re <= @(__type <= "Regex", pattern <= "a", flags <= "")
  stdout("aba".replaceAll(re, "x"))
"#;

const C12B_023_BYPASS_REPLACE_FIRST: &str = r#"re <= @(__type <= "Regex", pattern <= "a", flags <= "")
stdout("aba".replace(re, "x"))
"#;

const C12B_023_BYPASS_SPLIT: &str = r#"re <= @(__type <= "Regex", pattern <= " ", flags <= "")
stdout("a b c".split(re))
"#;

const C12B_023_BYPASS_MATCH: &str = r#"re <= @(__type <= "Regex", pattern <= "a", flags <= "")
stdout("abc".match(re))
"#;

#[test]
fn test_c12b_023_wasm_min_rejects_manual_pack_replaceall() {
    assert_regex_rejected(
        "min_bypass_replaceall",
        "wasm-min",
        C12B_023_BYPASS_REPLACE_ALL,
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_min_rejects_manual_pack_replace() {
    assert_regex_rejected(
        "min_bypass_replace",
        "wasm-min",
        C12B_023_BYPASS_REPLACE_FIRST,
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_min_rejects_manual_pack_split() {
    assert_regex_rejected(
        "min_bypass_split",
        "wasm-min",
        C12B_023_BYPASS_SPLIT,
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_min_rejects_manual_pack_match() {
    assert_regex_rejected(
        "min_bypass_match",
        "wasm-min",
        C12B_023_BYPASS_MATCH,
        &["reserved for compiler-internal use"],
    );
}

// C12B-023 root fix (2026-04-15 v2): indirect bypass routes must be
// rejected by the field-name-based checker. Pin per-profile to ensure
// every wasm target stops these at type-check.

#[test]
fn test_c12b_023_wasm_min_rejects_variable_bound_tag() {
    let src = r#"main =
  tag <= "Regex"
  re <= @(__type <= tag, pattern <= "a", flags <= "")
  stdout("aba".replaceAll(re, "x"))
"#;
    assert_regex_rejected(
        "min_bypass_var_tag",
        "wasm-min",
        src,
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_min_rejects_funcarg_bound_tag() {
    let src = r#"inner t = @(__type <= t, pattern <= "a", flags <= "")
main =
  re <= inner("Regex")
  stdout("aba".replaceAll(re, "x"))
"#;
    assert_regex_rejected(
        "min_bypass_funcarg",
        "wasm-min",
        src,
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_min_rejects_concat_tag() {
    let src = r#"re <= @(__type <= "Re" + "gex", pattern <= "a", flags <= "")
stdout("aba".replaceAll(re, "x"))
"#;
    assert_regex_rejected(
        "min_bypass_concat",
        "wasm-min",
        src,
        &["reserved for compiler-internal use"],
    );
}

// C12B-023 bypass closure (3rd layer, 2026-04-15): definition-site
// rejection. A user-authored `TypeDef` / `MoldDef` / `InheritanceDef`
// whose field name starts with `__` must be rejected before any
// instance is built. Without this, `Fake = @(__type <= "Regex", ...)`
// then `Fake(...)` materialises a pack with `__type == "Regex"` that
// slips past the 2nd-layer expression-site check (the TypeInst itself
// assigns only user-named fields like `payload <= "x"`, not `__type`).

#[test]
fn test_c12b_023_wasm_min_rejects_typedef_forged_regex_pack() {
    let src = r#"Fake = @(__type <= "Regex", pattern <= "a", flags <= "", payload: Str)
main =
  re <= Fake(payload <= "x")
  stdout("aba".replaceAll(re, "x"))
"#;
    assert_regex_rejected(
        "min_bypass_typedef_forged",
        "wasm-min",
        src,
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_min_rejects_typedef_forged_body_stream() {
    let src = r#"FakeReq = @(__body_stream <= "__v4_body_stream", __body_token <= 99999, x: Int)
main =
  req <= FakeReq(x <= 1)
  stdout("ok")
"#;
    assert_regex_rejected(
        "min_bypass_typedef_body_stream",
        "wasm-min",
        src,
        &["reserved for compiler-internal use"],
    );
}

#[test]
fn test_c12b_023_wasm_min_rejects_inheritance_forged_regex_pack() {
    let src = r#"Error => CustomError = @(__type <= "Regex", info: Str)
main =
  stdout("ok")
"#;
    assert_regex_rejected(
        "min_bypass_inheritance_forged",
        "wasm-min",
        src,
        &["reserved for compiler-internal use"],
    );
}

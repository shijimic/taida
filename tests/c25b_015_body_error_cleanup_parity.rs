//! C25B-015: 3-backend body-error cleanup parity audit.
//!
//! C20B-015 / C20B-017 hardened the *interpreter*'s `call_function` /
//! `eval_user_method` / `|==` handler-body cleanup paths for body-level
//! errors. The same symmetry is assumed to hold for the JS backend
//! (`src/js/codegen.rs` → `try/finally`) and the native backend
//! (`src/codegen/native_runtime/core.c` → `setjmp/longjmp`), but
//! before this module nothing actually *pinned* the parity.
//!
//! Each test runs an example under `examples/quality/c25b_015_body_error_cleanup/`
//! on all three backends and asserts identical stdout. If any backend
//! diverges, the failing test names which backend leaked.
//!
//! Fixtures cover three scenarios:
//! 1. Nested call unwind: throw bubbles through three function frames
//!    before an outer `|==` catches it; the caller scope survives and
//!    the `tail` binding keeps working.
//! 2. Closure-body error: a lambda captures an outer variable, its
//!    body throws, the outer `|==` catches it. The lambda frame must
//!    be released on every backend (JS closure try/finally, native
//!    closure refcount release on longjmp, interpreter closure pop).
//! 3. Handler-body error: the body of an inner `|==` handler itself
//!    throws a secondary error; the outer handler catches it. This is
//!    the exact scope-leak scenario C20B-017 (ROOT-20) fixed inside
//!    the interpreter — cross-backend parity was not regression-guarded
//!    until now.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn taida_bin() -> PathBuf {
    manifest_dir().join("target/release/taida")
}

fn fixture_dir() -> PathBuf {
    manifest_dir().join("examples/quality/c25b_015_body_error_cleanup")
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ensure_taida_bin_built() -> bool {
    taida_bin().exists()
}

fn unique_temp(prefix: &str, ext: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}_{}_{}.{}",
        prefix,
        std::process::id(),
        nanos,
        ext
    ))
}

fn read_expected(name: &str) -> String {
    fs::read_to_string(fixture_dir().join(format!("{}.expected", name)))
        .unwrap_or_else(|e| panic!("read {}.expected: {}", name, e))
}

fn outputs_equal(a: &str, b: &str) -> bool {
    // Match trailing-newline tolerant comparison (matches the C20B-015
    // parity helper).
    a.trim_end_matches('\n') == b.trim_end_matches('\n')
}

fn run_interpreter(fixture: &Path) -> String {
    let out = Command::new(taida_bin())
        .arg(fixture)
        .output()
        .expect("spawn interpreter");
    assert!(
        out.status.success(),
        "interpreter non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn run_js(fixture: &Path, tag: &str) -> Option<String> {
    if !node_available() {
        eprintln!("SKIP[{tag}]: node not available");
        return None;
    }
    let outdir = unique_temp(&format!("c25b015_{}_js", tag), "dir");
    fs::create_dir_all(&outdir).expect("mkdir outdir");
    let mjs = outdir.join(format!("{}.mjs", tag));
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg("-o")
        .arg(&mjs)
        .arg(fixture)
        .output()
        .expect("spawn js build");
    assert!(
        build.status.success(),
        "js build failed[{}]: {}",
        tag,
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node").arg(&mjs).output().expect("spawn node");
    let _ = fs::remove_dir_all(&outdir);
    assert!(
        run.status.success(),
        "node exit failed[{}]: {}",
        tag,
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8_lossy(&run.stdout).to_string())
}

fn run_native(fixture: &Path, tag: &str) -> Option<String> {
    if !cc_available() {
        eprintln!("SKIP[{tag}]: cc not available");
        return None;
    }
    let outdir = unique_temp(&format!("c25b015_{}_native", tag), "dir");
    fs::create_dir_all(&outdir).expect("mkdir outdir");
    let bin = outdir.join(format!("{}_bin", tag));
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg("-o")
        .arg(&bin)
        .arg(fixture)
        .output()
        .expect("spawn native build");
    if !build.status.success() {
        // Capture build stderr to the assertion message for post-mortem.
        let _ = fs::remove_dir_all(&outdir);
        panic!(
            "native build failed[{}]: status={:?} stderr={}",
            tag,
            build.status.code(),
            String::from_utf8_lossy(&build.stderr)
        );
    }
    let run = Command::new(&bin).output().expect("spawn native binary");
    let _ = fs::remove_dir_all(&outdir);
    assert!(
        run.status.success(),
        "native binary exit failed[{}]: status={:?} stderr={}",
        tag,
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8_lossy(&run.stdout).to_string())
}

fn assert_three_backend_parity(fixture_name: &str) {
    if !ensure_taida_bin_built() {
        eprintln!(
            "SKIP[{}]: taida release binary not built. Run `cargo build --release --bin taida` first.",
            fixture_name
        );
        return;
    }
    let fixture = fixture_dir().join(format!("{}.td", fixture_name));
    assert!(fixture.exists(), "fixture not found: {}", fixture.display());
    let expected = read_expected(fixture_name);

    let interp = run_interpreter(&fixture);
    assert!(
        outputs_equal(&interp, &expected),
        "C25B-015[{}] interpreter output mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        fixture_name,
        expected,
        interp
    );

    if let Some(js) = run_js(&fixture, fixture_name) {
        assert!(
            outputs_equal(&js, &interp),
            "C25B-015[{}] JS output diverges from interpreter.\n--- interp ---\n{}\n--- js ---\n{}\n",
            fixture_name,
            interp,
            js
        );
    }

    if let Some(native) = run_native(&fixture, fixture_name) {
        assert!(
            outputs_equal(&native, &interp),
            "C25B-015[{}] native output diverges from interpreter.\n--- interp ---\n{}\n--- native ---\n{}\n",
            fixture_name,
            interp,
            native
        );
    }
}

// ── Scenario 1: nested call unwind through three frames ──

#[test]
fn c25b_015_nested_call_unwind_three_backend_parity() {
    assert_three_backend_parity("nested_call_unwind");
}

// ── Scenario 2: closure-body error with captured outer variable ──

#[test]
fn c25b_015_closure_body_error_three_backend_parity() {
    assert_three_backend_parity("closure_body_error");
}

// ── Scenario 3: handler-body error (C20B-017 cross-backend parity) ──

#[test]
fn c25b_015_handler_body_error_three_backend_parity() {
    assert_three_backend_parity("handler_body_error");
}

// ── Interpreter-only reuse smoke: confirm the trailing bindings at
//    top-level don't silently see leaked closure / handler bindings
//    from inside the function under audit. This is a narrower assert
//    than scope-depth inspection (which the C20B-015 / C20B-017 suites
//    already cover for the interpreter), but it works across all
//    three backends because top-level name collisions would either
//    shadow (wrong value) or crash (undefined) on any sane backend.

#[test]
fn c25b_015_trial_does_not_leak_private_tag_to_top_level_interpreter() {
    if !ensure_taida_bin_built() {
        eprintln!(
            "SKIP: taida release binary not built. Run `cargo build --release --bin taida` first."
        );
        return;
    }
    let tmp = unique_temp("c25b015_leak_check", "dir");
    fs::create_dir_all(&tmp).expect("mkdir");
    let src = "\
Error => MyError = @()

throwOops msg =
  MyError(type <= \"MyError\", message <= msg).throw()
  \"\"
=> :Str

trial input =
  |== error: Error =
    tag <= \"inner-tag\"
    \"caught:\" + error.message
  => :Str
  tag <= \"inner-tag\"
  inner x = throwOops(\"oops\") + \" \" + tag + \" \" + x => :Str
  inner(input)
=> :Str

ignored <= trial(\"input\")
stdout(ignored + \" \")
// At top level, `tag` must NOT be defined. Referencing it should
// produce a runtime error, NOT silently print the leaked handler /
// closure value.
stdout(tag)
";
    let src_path = tmp.join("leak_check.td");
    fs::write(&src_path, src).expect("write src");
    let out = Command::new(taida_bin())
        .arg(&src_path)
        .output()
        .expect("spawn interp");
    let _ = fs::remove_dir_all(&tmp);
    assert!(
        !out.status.success(),
        "expected top-level reference to leaked inner `tag` to fail, but program succeeded. \
         stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains("tag") && (stderr.contains("Undefined") || stderr.contains("not found")),
        "expected 'Undefined variable: tag' style error, got stderr={}",
        stderr
    );
}

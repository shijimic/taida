//! D28B-023: 関数型制約付き型変数 (`F <= :T => :T`) を関数として呼び出す
//! higher-order generic を type checker + 3-backend (interpreter / JS / native)
//! で pin する。
//!
//! Pre-D28B-023:
//!
//! ```taida
//! applyFn[T, F <= :T => :T] x: T fn: F = fn(x) => :T
//! double x: Int = x * 2 => :Int
//! n <= applyFn(3, double)
//! ```
//!
//! は `[E1510] Cannot call 'fn' of type F as a function` で reject されていた。
//! parser (`parse_func_type_params`) は `F <= :T => :T` の関数型制約を受理して
//! いたが、type checker が call site で F を関数型として resolve する logic を
//! 持っていなかった。
//!
//! Post-D28B-023:
//!
//! - 制約付き型変数の constraint が `Type::Function(...)` のとき、call は
//!   その関数型に従って dispatch される。引数 arity / 型もそこで検査される。
//! - 制約のない型変数 (`[T] fn: T`) は引き続き `[E1510]` で reject、ただし
//!   higher-order generic を志向していた書き手のために hint を拡張。
//! - 3-backend (interpreter / JS / native) で同一 stdout を観測する parity を
//!   pin (Float の repr divergence を避けるため Int 値で確認)。
//!
//! Red test ゼロ容認 — 3-backend いずれかで divergence が出たら D28 regression。

mod common;

use common::{node_available, taida_bin};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const FIXTURE_INT: &str = r#"applyFn[T, F <= :T => :T] x: T fn: F = fn(x) => :T
double x: Int = x * 2 => :Int
inc x: Int = x + 1 => :Int
n <= applyFn(3, double)
m <= applyFn(10, inc)
debug(n)
debug(m)
"#;

const EXPECTED_INT: &str = "6\n11\n";

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

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_temp_td(prefix: &str, src: &str) -> PathBuf {
    let path = unique_temp(prefix, "td");
    fs::write(&path, src).expect("failed to write fixture");
    path
}

// ── Checker: higher-order generic should now type-check cleanly ──

#[test]
fn d28b_023_checker_accepts_function_constrained_type_param_call() {
    let src_path = write_temp_td("d28b023_check_ok", FIXTURE_INT);
    let out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src_path)
        .output()
        .expect("failed to spawn taida way check");
    let _ = fs::remove_file(&src_path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        out.status.success(),
        "D28B-023: `taida way check` should accept `F <= :T => :T` higher-order generic.\n\
         stdout=\n{}\nstderr=\n{}",
        stdout,
        stderr
    );
    assert!(
        !combined.contains("E1510"),
        "D28B-023: checker output unexpectedly contains E1510:\n{}",
        combined
    );
}

// ── Negative checker: unconstrained `[T] fn: T` still rejects with extended hint ──

#[test]
fn d28b_023_checker_still_rejects_unconstrained_call_with_hint() {
    let bad = "badCall[T] x: T fn: T = fn(x) => :T\n";
    let src_path = write_temp_td("d28b023_check_neg", bad);
    let out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src_path)
        .output()
        .expect("failed to spawn taida way check");
    let _ = fs::remove_file(&src_path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        !out.status.success(),
        "D28B-023: unconstrained `[T] fn: T` must still fail check.\ncombined={}",
        combined
    );
    assert!(
        combined.contains("[E1510]"),
        "D28B-023: expected E1510 to remain for unconstrained type-var call.\ncombined={}",
        combined
    );
    assert!(
        combined.contains("higher-order generic"),
        "D28B-023: expected extended hint mentioning higher-order generic.\ncombined={}",
        combined
    );
}

// ── Interpreter (reference) ──

#[test]
fn d28b_023_interpreter_runs_higher_order_generic() {
    let src_path = write_temp_td("d28b023_interp", FIXTURE_INT);
    let out = Command::new(taida_bin())
        .arg(&src_path)
        .output()
        .expect("failed to spawn interpreter");
    let _ = fs::remove_file(&src_path);
    assert!(
        out.status.success(),
        "interpreter non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(
        stdout, EXPECTED_INT,
        "D28B-023 interpreter output mismatch."
    );
}

// ── JS backend ──

#[test]
fn d28b_023_js_matches_interpreter() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let src_path = write_temp_td("d28b023_js_src", FIXTURE_INT);
    let mjs = unique_temp("d28b023_js", "mjs");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(&src_path)
        .arg("-o")
        .arg(&mjs)
        .output()
        .expect("failed to spawn js build");
    let _ = fs::remove_file(&src_path);
    assert!(
        build.status.success(),
        "js build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node")
        .arg(&mjs)
        .output()
        .expect("failed to spawn node");
    let _ = fs::remove_file(&mjs);
    assert!(
        run.status.success(),
        "node exit failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    assert_eq!(stdout, EXPECTED_INT, "D28B-023 JS output mismatch.");
}

// ── Native backend ──

#[test]
fn d28b_023_native_matches_interpreter() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let src_path = write_temp_td("d28b023_native_src", FIXTURE_INT);
    let bin = unique_temp("d28b023_native", "bin");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&src_path)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn native build");
    let _ = fs::remove_file(&src_path);
    assert!(
        build.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .output()
        .expect("failed to spawn native binary");
    let _ = fs::remove_file(&bin);
    assert!(
        run.status.success(),
        "native binary exit failed: status={:?}, stderr={}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    assert_eq!(stdout, EXPECTED_INT, "D28B-023 Native output mismatch.");
}

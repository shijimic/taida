//! D28B-024: subtype 制約付き型変数 (`T <= :Num` / `:Int` / `:Float`) の下で
//! 算術演算 (`+` / `-` / `*`) と順序 / 等価比較 (`<` / `>` / `>=` / `==` / `!=`)
//! を generic に解決することを type checker + 3-backend (interpreter / JS /
//! native) で pin する。
//!
//! Pre-D28B-024:
//!
//! ```taida
//! add[T <= :Num] x: T y: T = x + y => :T
//! ```
//!
//! は `Cannot apply Add to T and T` で reject されていた。subtype 制約 (E1509)
//! のチェックは機能していたが、`+` / `-` / `*` の operator dispatch が
//! 型変数を numeric として認識する logic を持っていなかった。
//!
//! Post-D28B-024:
//!
//! - `T <= :Num` (もしくは `:Int` / `:Float`) の下で `+` / `-` / `*` が numeric
//!   operation として resolve される。同じ T 同士なら結果型は T (`=> :T` の
//!   戻り値注釈と整合)、異なる numeric 同士なら Float / Int / Num に widening。
//! - 順序比較 (`<` / `>` / `>=`) と等価比較 (`==` / `!=`) も同じ generic 解決を行う。
//! - 制約のない型変数 (`[T]`) は引き続き reject (regression なし)。
//!
//! Note: Taida の `<=` は単一方向 bind 演算子であり比較演算子ではない
//! (PHILOSOPHY 演算子 10 種参照)。順序比較は `<` / `>` / `>=` のみ。
//!
//! Red test ゼロ容認 — 3-backend いずれかで divergence が出たら D28 regression。

mod common;

use common::{node_available, taida_bin};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

// Int で確認するパターン (3-backend で repr divergence なし)。
// Float / Mixed numeric は別途 D28B-009 (Float edge cases) で wD が pin する。
const FIXTURE_NUM_INT: &str = r#"add[T <= :Num] x: T y: T = x + y => :T
sub[T <= :Num] x: T y: T = x - y => :T
mul[T <= :Num] x: T y: T = x * y => :T
isLt[T <= :Num] x: T y: T = x < y => :Bool
isGt[T <= :Num] x: T y: T = x > y => :Bool
isGtEq[T <= :Num] x: T y: T = x >= y => :Bool
isEq[T <= :Num] x: T y: T = x == y => :Bool
isNotEq[T <= :Num] x: T y: T = x != y => :Bool
debug(add(1, 2))
debug(sub(5, 2))
debug(mul(3, 4))
debug(isLt(1, 2))
debug(isGt(5, 1))
debug(isGtEq(7, 7))
debug(isEq(7, 7))
debug(isNotEq(7, 8))
"#;

const EXPECTED_NUM_INT: &str = "3\n3\n12\ntrue\ntrue\ntrue\ntrue\ntrue\n";

// `T <= :Int` 制約版 (Num より strict)。
const FIXTURE_INT_BOUND: &str = r#"addInt[T <= :Int] x: T y: T = x + y => :T
debug(addInt(10, 20))
debug(addInt(0, -5))
"#;

const EXPECTED_INT_BOUND: &str = "30\n-5\n";

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

// ── Checker positive: `T <= :Num` arithmetic + comparisons accepted ──

#[test]
fn d28b_024_checker_accepts_num_bounded_arithmetic() {
    let src_path = write_temp_td("d28b024_check_num", FIXTURE_NUM_INT);
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
        "D28B-024: `taida way check` should accept `T <= :Num` generic arithmetic.\n\
         stdout=\n{}\nstderr=\n{}",
        stdout,
        stderr
    );
    assert!(
        !combined.contains("Cannot apply"),
        "D28B-024: checker still emitted `Cannot apply ...`:\n{}",
        combined
    );
}

#[test]
fn d28b_024_checker_accepts_int_bounded_arithmetic() {
    let src_path = write_temp_td("d28b024_check_int", FIXTURE_INT_BOUND);
    let out = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&src_path)
        .output()
        .expect("failed to spawn taida way check");
    let _ = fs::remove_file(&src_path);
    assert!(
        out.status.success(),
        "D28B-024: `taida way check` should accept `T <= :Int` generic arithmetic.\n\
         stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── Checker negative: unconstrained `[T]` arithmetic still rejected ──

#[test]
fn d28b_024_checker_rejects_unconstrained_arithmetic() {
    let bad = "addBad[T] x: T y: T = x + y => :T\n";
    let src_path = write_temp_td("d28b024_check_neg", bad);
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
        "D28B-024: unconstrained `[T]` arithmetic must still fail check.\ncombined={}",
        combined
    );
    assert!(
        combined.contains("Cannot apply Add"),
        "D28B-024: expected `Cannot apply Add` for unconstrained type-var.\ncombined={}",
        combined
    );
}

// ── Checker negative: constraint violation at call site (E1509) preserved ──

#[test]
fn d28b_024_checker_preserves_e1509_on_constraint_violation() {
    let bad = "add[T <= :Num] x: T y: T = x + y => :T\ndebug(add(\"hi\", \"there\"))\n";
    let src_path = write_temp_td("d28b024_check_e1509", bad);
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
        "D28B-024: passing Str to `T <= :Num` must still fail.\ncombined={}",
        combined
    );
    assert!(
        combined.contains("[E1509]"),
        "D28B-024: expected E1509 constraint violation.\ncombined={}",
        combined
    );
}

// ── Interpreter (reference) ──

#[test]
fn d28b_024_interpreter_runs_num_bounded() {
    let src_path = write_temp_td("d28b024_interp_num", FIXTURE_NUM_INT);
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
        stdout, EXPECTED_NUM_INT,
        "D28B-024 interpreter output mismatch."
    );
}

#[test]
fn d28b_024_interpreter_runs_int_bounded() {
    let src_path = write_temp_td("d28b024_interp_int", FIXTURE_INT_BOUND);
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
        stdout, EXPECTED_INT_BOUND,
        "D28B-024 interpreter (Int bound) output mismatch."
    );
}

// ── JS backend ──

#[test]
fn d28b_024_js_matches_interpreter_num_bounded() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let src_path = write_temp_td("d28b024_js_num_src", FIXTURE_NUM_INT);
    let mjs = unique_temp("d28b024_js_num", "mjs");
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
    assert_eq!(stdout, EXPECTED_NUM_INT, "D28B-024 JS output mismatch.");
}

#[test]
fn d28b_024_js_matches_interpreter_int_bounded() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let src_path = write_temp_td("d28b024_js_int_src", FIXTURE_INT_BOUND);
    let mjs = unique_temp("d28b024_js_int", "mjs");
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
    assert_eq!(
        stdout, EXPECTED_INT_BOUND,
        "D28B-024 JS (Int bound) output mismatch."
    );
}

// ── Native backend ──

#[test]
fn d28b_024_native_matches_interpreter_num_bounded() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let src_path = write_temp_td("d28b024_native_num_src", FIXTURE_NUM_INT);
    let bin = unique_temp("d28b024_native_num", "bin");
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
    assert_eq!(stdout, EXPECTED_NUM_INT, "D28B-024 Native output mismatch.");
}

#[test]
fn d28b_024_native_matches_interpreter_int_bounded() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let src_path = write_temp_td("d28b024_native_int_src", FIXTURE_INT_BOUND);
    let bin = unique_temp("d28b024_native_int", "bin");
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
    assert_eq!(
        stdout, EXPECTED_INT_BOUND,
        "D28B-024 Native (Int bound) output mismatch."
    );
}

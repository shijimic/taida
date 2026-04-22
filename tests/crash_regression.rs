/// Crash-regression corpus tests.
///
/// Each `.td` case in `tests/crash_regression/` reproduces a previously observed
/// crash/parity issue. The interpreter output is treated as the reference.
mod common;

use common::{normalize, taida_bin};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn crash_regression_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("crash_regression")
}

fn unique_temp_path(prefix: &str, stem: &str, ext: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}_{}_{}_{}.{}",
        prefix,
        stem,
        std::process::id(),
        nanos,
        ext
    ))
}

// normalize() is provided by common::normalize (RCB-26).

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

fn run_interpreter(td_path: &Path) -> Result<String, String> {
    let output = Command::new(taida_bin())
        .arg(td_path)
        .output()
        .map_err(|e| format!("failed to execute interpreter: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "interpreter failed (status: {:?})\nstderr:\n{}\nstdout:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        ));
    }

    Ok(normalize(&String::from_utf8_lossy(&output.stdout)))
}

fn run_js(td_path: &Path) -> Result<String, String> {
    let stem = td_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("invalid file stem: {}", td_path.display()))?;
    let js_path = unique_temp_path("taida_crash_js", stem, "mjs");

    let transpile_output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .map_err(|e| format!("failed to execute build: {e}"))?;

    if !transpile_output.status.success() {
        let _ = fs::remove_file(&js_path);
        return Err(format!(
            "transpile failed (status: {:?})\nstderr:\n{}\nstdout:\n{}",
            transpile_output.status.code(),
            String::from_utf8_lossy(&transpile_output.stderr),
            String::from_utf8_lossy(&transpile_output.stdout),
        ));
    }

    let run_output = Command::new("node")
        .arg(&js_path)
        .output()
        .map_err(|e| format!("failed to execute node: {e}"))?;
    let _ = fs::remove_file(&js_path);

    if !run_output.status.success() {
        return Err(format!(
            "node execution failed (status: {:?})\nstderr:\n{}\nstdout:\n{}",
            run_output.status.code(),
            String::from_utf8_lossy(&run_output.stderr),
            String::from_utf8_lossy(&run_output.stdout),
        ));
    }

    Ok(normalize(&String::from_utf8_lossy(&run_output.stdout)))
}

fn run_native(td_path: &Path) -> Result<String, String> {
    let stem = td_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("invalid file stem: {}", td_path.display()))?;
    let binary_path = unique_temp_path("taida_crash_native", stem, "bin");

    let compile_output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(td_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .map_err(|e| format!("failed to execute build: {e}"))?;

    if !compile_output.status.success() {
        let _ = fs::remove_file(&binary_path);
        return Err(format!(
            "native compile failed (status: {:?})\nstderr:\n{}\nstdout:\n{}",
            compile_output.status.code(),
            String::from_utf8_lossy(&compile_output.stderr),
            String::from_utf8_lossy(&compile_output.stdout),
        ));
    }

    let run_output = Command::new(&binary_path)
        .output()
        .map_err(|e| format!("failed to execute compiled binary: {e}"))?;
    let _ = fs::remove_file(&binary_path);

    if !run_output.status.success() {
        return Err(format!(
            "native binary failed (status: {:?})\nstderr:\n{}\nstdout:\n{}",
            run_output.status.code(),
            String::from_utf8_lossy(&run_output.stderr),
            String::from_utf8_lossy(&run_output.stdout),
        ));
    }

    Ok(normalize(&String::from_utf8_lossy(&run_output.stdout)))
}

// C24 Phase 5 (RC-SLOW-2 / C24B-006): per-fixture decomposition of
// `test_crash_regression_corpus_three_way`. The original was a single
// 10-17s test iterating `tests/crash_regression/*.td` serially; now each
// fixture becomes its own `#[test]`.

fn run_crash_regression_fixture(stem: &str) {
    if !cc_available() {
        eprintln!(
            "SKIP: cc not available, skipping crash-regression for {}",
            stem
        );
        return;
    }

    let has_node = node_available();
    let td_path = crash_regression_dir().join(format!("{}.td", stem));

    let interp =
        run_interpreter(&td_path).unwrap_or_else(|e| panic!("{}: interpreter failed\n{}", stem, e));
    let native = run_native(&td_path).unwrap_or_else(|e| panic!("{}: native failed\n{}", stem, e));

    if native != interp {
        panic!(
            "{}: interpreter/native mismatch\n  interp ({} lines):\n{}\n  native ({} lines):\n{}",
            stem,
            interp.lines().count(),
            interp
                .lines()
                .map(|l| format!("    {}", l))
                .collect::<Vec<_>>()
                .join("\n"),
            native.lines().count(),
            native
                .lines()
                .map(|l| format!("    {}", l))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    if has_node {
        let js = run_js(&td_path).unwrap_or_else(|e| panic!("{}: js failed\n{}", stem, e));
        if js != interp {
            panic!(
                "{}: interpreter/js mismatch\n  interp ({} lines):\n{}\n  js ({} lines):\n{}",
                stem,
                interp.lines().count(),
                interp
                    .lines()
                    .map(|l| format!("    {}", l))
                    .collect::<Vec<_>>()
                    .join("\n"),
                js.lines().count(),
                js.lines()
                    .map(|l| format!("    {}", l))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    }
}

mod crash_regression_fixture_list {
    include!(concat!(env!("OUT_DIR"), "/crash_regression_fixtures.rs"));
}

#[test]
fn test_crash_regression_corpus_nonempty() {
    assert!(
        !crash_regression_fixture_list::CRASH_REGRESSION_FIXTURES.is_empty(),
        "No crash-regression corpus files found"
    );
}

macro_rules! c24_fixture_runner {
    ($stem:expr) => {
        run_crash_regression_fixture($stem)
    };
}
include!(concat!(env!("OUT_DIR"), "/crash_regression_tests.rs"));

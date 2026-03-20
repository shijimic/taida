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

#[test]
fn test_crash_regression_corpus_three_way() {
    if !cc_available() {
        eprintln!("SKIP: cc not available, skipping crash-regression native checks");
        return;
    }
    let has_node = node_available();

    let dir = crash_regression_dir();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("tests/crash_regression directory should exist")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".td"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    assert!(
        !entries.is_empty(),
        "No crash-regression corpus files found in {}",
        dir.display()
    );

    let mut passed = 0usize;
    let mut failures = Vec::new();

    for entry in &entries {
        let td_path = entry.path();
        let name = td_path
            .file_stem()
            .expect("file stem")
            .to_string_lossy()
            .to_string();

        let interp = match run_interpreter(&td_path) {
            Ok(out) => out,
            Err(err) => {
                failures.push(format!("{}: interpreter failed\n{}", name, err));
                continue;
            }
        };

        let native = match run_native(&td_path) {
            Ok(out) => out,
            Err(err) => {
                failures.push(format!("{}: native failed\n{}", name, err));
                continue;
            }
        };

        if native != interp {
            // AT-9: Show full output diff, not just first 4 lines
            failures.push(format!(
                "{}: interpreter/native mismatch\n  interp ({} lines):\n{}\n  native ({} lines):\n{}",
                name,
                interp.lines().count(),
                interp.lines().map(|l| format!("    {}", l)).collect::<Vec<_>>().join("\n"),
                native.lines().count(),
                native.lines().map(|l| format!("    {}", l)).collect::<Vec<_>>().join("\n"),
            ));
            continue;
        }

        if has_node {
            let js = match run_js(&td_path) {
                Ok(out) => out,
                Err(err) => {
                    failures.push(format!("{}: js failed\n{}", name, err));
                    continue;
                }
            };

            if js != interp {
                // AT-9: Show full output diff, not just first 4 lines
                failures.push(format!(
                    "{}: interpreter/js mismatch\n  interp ({} lines):\n{}\n  js ({} lines):\n{}",
                    name,
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
                ));
                continue;
            }
        }

        passed += 1;
    }

    eprintln!(
        "Crash regression corpus: {}/{} passed (js_checked: {})",
        passed,
        entries.len(),
        has_node
    );

    if !failures.is_empty() {
        panic!(
            "{} crash-regression case(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n\n"),
        );
    }
}

//! C18-1: Enum cross-module parity test.
//!
//! Runs `examples/quality/enum_cross_module.td` through all three backends
//! (Interpreter, JS, Native) and asserts byte-identical stdout against the
//! canonical `examples/quality/enum_cross_module.expected`.
//!
//! Red test ゼロ容認 — any backend divergence is a C18-1 regression. The
//! unit tests in `src/types/checker_tests.rs` already pin the checker-level
//! E1608 / E1618 contract; this harness pins the runtime parity.

mod common;

use common::taida_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn td_path() -> PathBuf {
    manifest_dir().join("examples/quality/enum_cross_module.td")
}

fn expected_path() -> PathBuf {
    manifest_dir().join("examples/quality/enum_cross_module.expected")
}

fn read_expected() -> String {
    fs::read_to_string(expected_path())
        .expect("examples/quality/enum_cross_module.expected must exist")
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

fn outputs_equal(a: &str, b: &str) -> bool {
    a.trim_end_matches('\n') == b.trim_end_matches('\n')
}

#[test]
fn c18_1_enum_cross_module_interpreter_matches_expected() {
    let out = Command::new(taida_bin())
        .arg(td_path())
        .output()
        .expect("failed to invoke interpreter");
    assert!(
        out.status.success(),
        "interpreter exited with non-zero status: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C18-1 interpreter output mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
}

#[test]
fn c18_1_enum_cross_module_js_matches_interpreter() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let js_out_path = unique_temp("c18_enum_cross_module", "mjs");
    let build_out = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(td_path())
        .arg("-o")
        .arg(&js_out_path)
        .output()
        .expect("failed to invoke js build");
    assert!(
        build_out.status.success(),
        "js build failed: {}",
        String::from_utf8_lossy(&build_out.stderr)
    );
    let node_out = Command::new("node")
        .arg(&js_out_path)
        .output()
        .expect("failed to invoke node");
    // Clean up the main .mjs plus the submodule .mjs emitted alongside it
    // (the builder places the dependency next to the entry output).
    let _ = fs::remove_file(&js_out_path);
    let dir = js_out_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if let Some(ext) = p.extension()
            && ext == "mjs"
            && let Some(stem) = p.file_stem()
            && stem.to_string_lossy().contains("c18_enum_cross_module")
        {
            let _ = fs::remove_file(&p);
        }
    }
    // Also remove the co-emitted colors.mjs if present
    let sub_dir = dir.join("enum_cross_module_pkg");
    let _ = fs::remove_dir_all(&sub_dir);

    assert!(
        node_out.status.success(),
        "node exit failed: {}",
        String::from_utf8_lossy(&node_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&node_out.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C18-1 JS output mismatch (interpreter is reference).\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
}

#[test]
fn c18_1_enum_cross_module_native_matches_interpreter() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let bin_path = unique_temp("c18_enum_cross_module", "bin");
    let build_out = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(td_path())
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("failed to invoke native build");
    assert!(
        build_out.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&build_out.stderr)
    );
    let run_out = Command::new(&bin_path)
        .output()
        .expect("failed to execute native binary");
    let _ = fs::remove_file(&bin_path);
    assert!(
        run_out.status.success(),
        "native binary exit failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&run_out.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C18-1 native output mismatch (interpreter is reference).\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
}

//! Backend-parity helpers shared by the `f65_*` regression tests: run one
//! program on the interpreter and on compiled backends, and require the
//! same stdout from each.
#![allow(dead_code)]

use crate::common::{taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::{Command, Output};

/// The compiled backends a portable program is expected to agree on.
pub const ALL_COMPILED: &[&str] = &["native", "wasm-min", "wasm-wasi"];

pub fn stdout_of(output: &Output, what: &str) -> String {
    assert!(
        output.status.success(),
        "{what}: exit {:?}\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string()
}

/// Build `td` for `profile` and run it in `dir`. `None` when the profile
/// needs wasmtime and it is not installed.
pub fn run_compiled(profile: &str, td: &Path, dir: &Path) -> Option<String> {
    let wasmtime = if profile == "native" {
        None
    } else {
        Some(wasmtime_bin()?)
    };
    let bin = dir.join(format!(
        "{}.{profile}",
        td.file_stem().unwrap().to_string_lossy()
    ));
    let build = Command::new(taida_bin())
        .args(["build", profile])
        .arg(td)
        .arg("-o")
        .arg(&bin)
        .current_dir(dir)
        .output()
        .unwrap();
    stdout_of(&build, &format!("{profile} build"));
    let run = match wasmtime {
        None => Command::new(&bin).current_dir(dir).output(),
        Some(wasmtime) => Command::new(wasmtime)
            .args(["run", "--dir=.", "--"])
            .arg(&bin)
            .current_dir(dir)
            .output(),
    }
    .unwrap();
    Some(stdout_of(&run, profile))
}

pub fn run_interpreter(td: &Path, dir: &Path) -> String {
    let output = Command::new(taida_bin())
        .arg(td)
        .current_dir(dir)
        .output()
        .unwrap();
    stdout_of(&output, "interpreter")
}

/// Write `files` (name, source) into `dir`, run `main.td` on the
/// interpreter and on each compiled `profile`, and require every backend
/// to print `expected`.
pub fn parity_files_in(dir: &Path, files: &[(&str, &str)], expected: &str, profiles: &[&str]) {
    for (name, source) in files {
        std::fs::write(dir.join(name), source).unwrap();
    }
    let td = dir.join("main.td");
    assert_eq!(run_interpreter(&td, dir), expected, "interpreter");
    for profile in profiles {
        if let Some(out) = run_compiled(profile, &td, dir) {
            assert_eq!(out, expected, "{profile}");
        }
    }
}

pub fn parity_in(dir: &Path, source: &str, expected: &str, profiles: &[&str]) {
    parity_files_in(dir, &[("main.td", source)], expected, profiles);
}

/// [`parity_in`] in a fresh scratch directory.
pub fn parity(tag: &str, source: &str, expected: &str, profiles: &[&str]) {
    let dir = unique_temp_dir(tag);
    parity_in(&dir, source, expected, profiles);
    std::fs::remove_dir_all(dir).unwrap();
}

/// [`parity_files_in`] in a fresh scratch directory.
pub fn parity_files(tag: &str, files: &[(&str, &str)], expected: &str, profiles: &[&str]) {
    let dir = unique_temp_dir(tag);
    parity_files_in(&dir, files, expected, profiles);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Run `taida way check` on `files` (written into a fresh scratch directory;
/// `main.td` is checked) and return whether it passed and its output.
pub fn way_check_files(tag: &str, files: &[(&str, &str)]) -> (bool, String) {
    let dir = unique_temp_dir(tag);
    for (name, source) in files {
        std::fs::write(dir.join(name), source).unwrap();
    }
    let out = Command::new(taida_bin())
        .args(["way", "check"])
        .arg(dir.join("main.td"))
        .output()
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// Require `taida way check` to reject `source` with diagnostic `code`.
pub fn assert_rejected(tag: &str, source: &str, code: &str) {
    let (ok, out) = way_check_files(tag, &[("main.td", source)]);
    assert!(!ok && out.contains(code), "expected {code}, got:\n{out}");
}

//! C18B-004 regression: package-backed Enum import must work across
//! checker / interpreter / JS / Native.
//!
//! Pre-fix the JS codegen and Native lowering both short-circuited on
//! package paths (`>>> acme/lib => @(Color)`) with an "Unknown enum
//! variant" error at build time even though the checker / interpreter
//! resolved the same import fine. This test builds a scratch project
//! in the temp directory with a `.taida/deps/acme/lib/main.td` that
//! exports an Enum and a factory function, then asserts all three
//! backends agree on stdout.
//!
//! The test is skipped when `node` / `cc` are not on the PATH so it
//! matches the other C18 parity suites' behaviour.

mod common;

use common::taida_bin;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn unique_temp(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

/// Build `<project>/.taida/deps/acme/lib/main.td` that defines a
/// `Color` enum and a `pickColor` factory, plus `<project>/main.td`
/// that consumes them via a package import.
///
/// The consumer exercises the three headline Enum-facing paths that
/// used to blow up at build time pre-C18B-004:
///   1. `Color:Red()` literal — tests that JS / Native resolve the
///      Enum at build time.
///   2. Enum equality on a function-returned Enum — tests function
///      boundary (#6 in the Hachikuma audit).
///   3. Enum ordering comparison — tests C18-4 with the imported enum.
///
/// The `jsonEncode(@(state <= Color:Blue()))` direction is intentionally
/// NOT covered here because cross-module `__taida_enumDefs` propagation
/// is a separate pre-existing gap (present for both relative-path and
/// package-path imports) that predates C18B-004 and is out of scope
/// for the C18B-004 fix — see `.dev/C18_BLOCKERS.md` for triage.
fn set_up_project() -> PathBuf {
    let project = unique_temp("c18b_004_enum_pkg_import");
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&project).expect("mkdir project");

    // .taida sentinel so `find_project_root` picks up this directory,
    // plus an empty `packages.tdm` so the JS build's
    // `find_packages_tdm_from` resolver also agrees the project root
    // is `<tmp>/` (JS codegen reads `pkg_root` directly rather than
    // walking `.taida/`).
    fs::create_dir_all(project.join(".taida")).expect("mkdir .taida");
    fs::write(
        project.join("packages.tdm"),
        "[package]\nname = \"c18b_004_scratch\"\n",
    )
    .expect("write packages.tdm");

    let deps_root = project.join(".taida").join("deps").join("acme").join("lib");
    fs::create_dir_all(&deps_root).expect("mkdir .taida/deps/acme/lib");

    // Enum exporter — package entry point.
    let lib_td = r#"Enum => Color = :Red :Green :Blue

pickColor n =
  | n == 0 |> Color:Red()
  | n == 1 |> Color:Green()
  | _ |> Color:Blue()
=> :Color

<<< @(Color, pickColor)
"#;
    fs::write(deps_root.join("main.td"), lib_td).expect("write deps main.td");

    // Consumer main — C18B-004 scope is the build-time resolution,
    // not the cross-module jsonEncode wire bridge.
    let main_td = r#">>> acme/lib => @(Color, pickColor)

red <= Color:Red()
stdout(red.toString())

picked <= pickColor(1)
match <= picked == Color:Green()
stdout(match.toString())

ord <= Color:Green() >= Color:Red()
stdout(ord.toString())

blue <= pickColor(99)
isBlue <= blue == Color:Blue()
stdout(isBlue.toString())
"#;
    fs::write(project.join("main.td"), main_td).expect("write main.td");

    project
}

const EXPECTED: &str = "0\ntrue\ntrue\ntrue\n";

fn run_interp(main_td: &Path) -> (String, bool, String) {
    let out = Command::new(taida_bin())
        .arg(main_td)
        .output()
        .expect("failed to invoke interpreter");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (stdout, out.status.success(), stderr)
}

#[test]
fn c18b_004_interpreter_package_enum_import() {
    let project = set_up_project();
    let main = project.join("main.td");
    let (stdout, ok, stderr) = run_interp(&main);
    let _ = fs::remove_dir_all(&project);
    assert!(ok, "interpreter failed: stderr={}", stderr);
    assert_eq!(
        stdout, EXPECTED,
        "interpreter stdout mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        EXPECTED, stdout
    );
}

#[test]
fn c18b_004_js_package_enum_import() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let project = set_up_project();
    let main = project.join("main.td");
    let out_path = project.join("main.mjs");

    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(&main)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("failed to invoke js build");
    assert!(
        build.status.success(),
        "js build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new("node")
        .arg(&out_path)
        .output()
        .expect("failed to invoke node");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let ok = run.status.success();
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let _ = fs::remove_dir_all(&project);
    assert!(ok, "node exit failed: {}", stderr);
    assert_eq!(
        stdout, EXPECTED,
        "JS stdout mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        EXPECTED, stdout
    );
}

#[test]
fn c18b_004_native_package_enum_import() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let project = set_up_project();
    let main = project.join("main.td");
    let out_path = project.join("main.bin");

    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&main)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("failed to invoke native build");
    assert!(
        build.status.success(),
        "native build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&out_path)
        .output()
        .expect("failed to execute native binary");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let ok = run.status.success();
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let _ = fs::remove_dir_all(&project);
    assert!(ok, "native binary exit failed: {}", stderr);
    assert_eq!(
        stdout, EXPECTED,
        "native stdout mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        EXPECTED, stdout
    );
}

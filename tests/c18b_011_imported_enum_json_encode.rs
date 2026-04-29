//! C18B-011 regression: consumer-side `jsonEncode(@(state <= ImportedEnum:X()))`
//! must emit the variant-name Str on all 3 backends for both relative-path
//! and package imports.
//!
//! Pre-fix the JS backend absorbed the imported enum's variant list into
//! codegen's `self.enum_defs` (so the ordinal lowered correctly), but the
//! generated `.mjs` did not re-register the enum into the consumer
//! module's `__taida_enumDefs` runtime registry. As a result,
//! `__taida_enumVal('Color', 2).toJSON()` fell back to the raw ordinal
//! (`2`) instead of the variant name (`"Blue"`), so JS emitted
//! `{"state":2}` while Interpreter / Native correctly emitted
//! `{"state":"Blue"}`.
//!
//! The fix emits a `__taida_registerEnumDef('Color', [...])` line next
//! to each `import { Color } from '...'` statement in `gen_import`, so
//! the consumer's per-module registry matches the exporter's.
//!
//! This test pins the symmetry for both import shapes:
//!   * relative import (`>>> ./colors.td => @(Color)`)
//!   * package import (`>>> acme/lib => @(Color)`)
//!
//! Tests skip when `node` / `cc` are not on PATH, matching the rest of
//! the C18 parity suite.

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

// =========================================================================
// Case 1: relative-path import
// =========================================================================

fn set_up_relative_project() -> PathBuf {
    let project = unique_temp("c18b_011_rel");
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&project).expect("mkdir project");

    // Minimal `.taida` sentinel so project-root resolution succeeds for
    // the JS backend. `packages.tdm` is also required for the build's
    // `find_packages_tdm_from` logic to agree on the root.
    fs::create_dir_all(project.join(".taida")).expect("mkdir .taida");
    fs::write(
        project.join("packages.tdm"),
        "[package]\nname = \"c18b_011_rel\"\n",
    )
    .expect("write packages.tdm");

    let colors_td = "Enum => Color = :Red :Green :Blue\n<<< @(Color)\n";
    fs::write(project.join("colors.td"), colors_td).expect("write colors.td");

    let main_td = r#">>> ./colors.td => @(Color)
stdout(jsonEncode(@(state <= Color:Blue())))
stdout(jsonEncode(@(state <= Color:Red())))
"#;
    fs::write(project.join("main.td"), main_td).expect("write main.td");

    project
}

const EXPECTED_REL: &str = "{\"state\":\"Blue\"}\n{\"state\":\"Red\"}\n";

#[test]
fn c18b_011_interpreter_relative_imported_enum_jsonencode() {
    let project = set_up_relative_project();
    let main = project.join("main.td");

    let out = Command::new(taida_bin())
        .arg(&main)
        .output()
        .expect("invoke interpreter");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let ok = out.status.success();
    let _ = fs::remove_dir_all(&project);

    assert!(ok, "interpreter failed: stderr={}", stderr);
    assert_eq!(
        stdout, EXPECTED_REL,
        "interpreter stdout mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        EXPECTED_REL, stdout
    );
}

#[test]
fn c18b_011_js_relative_imported_enum_jsonencode() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let project = set_up_relative_project();
    let main = project.join("main.td");
    let out_path = project.join("main.mjs");

    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(&main)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("invoke js build");
    assert!(
        build.status.success(),
        "js build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new("node")
        .arg(&out_path)
        .output()
        .expect("invoke node");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let ok = run.status.success();
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let _ = fs::remove_dir_all(&project);

    assert!(ok, "node exit failed: {}", stderr);
    assert_eq!(
        stdout, EXPECTED_REL,
        "JS stdout mismatch (C18B-011 regression).\n--- expected ---\n{}\n--- got ---\n{}\n",
        EXPECTED_REL, stdout
    );
}

#[test]
fn c18b_011_native_relative_imported_enum_jsonencode() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let project = set_up_relative_project();
    let main = project.join("main.td");
    let out_path = project.join("main.bin");

    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&main)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("invoke native build");
    assert!(
        build.status.success(),
        "native build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&out_path)
        .output()
        .expect("execute native binary");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let ok = run.status.success();
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let _ = fs::remove_dir_all(&project);

    assert!(ok, "native binary failed: {}", stderr);
    assert_eq!(
        stdout, EXPECTED_REL,
        "native stdout mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        EXPECTED_REL, stdout
    );
}

// =========================================================================
// Case 2: package import (`.taida/deps/acme/lib/`)
// =========================================================================

fn set_up_package_project() -> PathBuf {
    let project = unique_temp("c18b_011_pkg");
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&project).expect("mkdir project");

    fs::create_dir_all(project.join(".taida")).expect("mkdir .taida");
    fs::write(
        project.join("packages.tdm"),
        "[package]\nname = \"c18b_011_pkg\"\n",
    )
    .expect("write packages.tdm");

    let deps_root = project.join(".taida").join("deps").join("acme").join("lib");
    fs::create_dir_all(&deps_root).expect("mkdir .taida/deps/acme/lib");

    let lib_td = r#"Enum => Color = :Red :Green :Blue

pickColor n =
  | n == 0 |> Color:Red()
  | n == 1 |> Color:Green()
  | _ |> Color:Blue()
=> :Color

<<< @(Color, pickColor)
"#;
    fs::write(deps_root.join("main.td"), lib_td).expect("write deps main.td");
    fs::write(
        deps_root.join("packages.tdm"),
        "[package]\nname = \"acme/lib\"\n",
    )
    .expect("write deps packages.tdm");

    // Consumer exercises consumer-side literal + imported factory.
    // Both directions must emit the variant name Str, not the ordinal.
    let main_td = r#">>> acme/lib => @(Color, pickColor)

stdout(jsonEncode(@(state <= Color:Blue())))
stdout(jsonEncode(@(state <= pickColor(1))))
"#;
    fs::write(project.join("main.td"), main_td).expect("write main.td");

    project
}

const EXPECTED_PKG: &str = "{\"state\":\"Blue\"}\n{\"state\":\"Green\"}\n";

fn run_interp(main: &Path) -> (String, bool, String) {
    let out = Command::new(taida_bin())
        .arg(main)
        .output()
        .expect("invoke interpreter");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn c18b_011_interpreter_package_imported_enum_jsonencode() {
    let project = set_up_package_project();
    let main = project.join("main.td");
    let (stdout, ok, stderr) = run_interp(&main);
    let _ = fs::remove_dir_all(&project);
    assert!(ok, "interpreter failed: stderr={}", stderr);
    assert_eq!(
        stdout, EXPECTED_PKG,
        "interpreter stdout mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        EXPECTED_PKG, stdout
    );
}

#[test]
fn c18b_011_js_package_imported_enum_jsonencode() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let project = set_up_package_project();
    let main = project.join("main.td");
    let out_path = project.join("main.mjs");

    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg(&main)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("invoke js build");
    assert!(
        build.status.success(),
        "js build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new("node")
        .arg(&out_path)
        .output()
        .expect("invoke node");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let ok = run.status.success();
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let _ = fs::remove_dir_all(&project);
    assert!(ok, "node exit failed: {}", stderr);
    assert_eq!(
        stdout, EXPECTED_PKG,
        "JS stdout mismatch (C18B-011 package regression).\n--- expected ---\n{}\n--- got ---\n{}\n",
        EXPECTED_PKG, stdout
    );
}

#[test]
fn c18b_011_native_package_imported_enum_jsonencode() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let project = set_up_package_project();
    let main = project.join("main.td");
    let out_path = project.join("main.bin");

    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&main)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("invoke native build");
    assert!(
        build.status.success(),
        "native build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&out_path).output().expect("execute native");
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let ok = run.status.success();
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    let _ = fs::remove_dir_all(&project);
    assert!(ok, "native binary failed: {}", stderr);
    assert_eq!(
        stdout, EXPECTED_PKG,
        "native stdout mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        EXPECTED_PKG, stdout
    );
}

//! E32B-008: `taida build` descriptor-mode CLI ambiguity diagnostics.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::process::Command;

fn stderr_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn e32b_008_existing_js_single_target_build_still_parses() {
    let dir = unique_temp_dir("e32b_008_js_single_target");
    let src = dir.join("main.td");
    let out = dir.join("main.mjs");
    write_file(&src, "stdout(\"e32b-008 js\")\n");

    let output = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("run taida build js");

    assert!(
        output.status.success(),
        "existing `taida build js <PATH>` must keep working; stderr={}",
        stderr_text(&output)
    );
    assert!(out.exists(), "JS output should be written");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e32b_008_existing_target_path_grammar_still_recognizes_native_and_wasm() {
    let dir = unique_temp_dir("e32b_008_target_path_grammar");
    for target in ["native", "wasm-min", "wasm-wasi", "wasm-edge", "wasm-full"] {
        let missing = dir.join(format!("missing-{}.td", target));
        let output = Command::new(taida_bin())
            .arg("build")
            .arg(target)
            .arg(&missing)
            .output()
            .unwrap_or_else(|_| panic!("run taida build {target} <missing-path>"));

        assert_eq!(
            output.status.code(),
            Some(1),
            "`taida build {target} <PATH>` should reach target-specific input validation"
        );
        let stderr = stderr_text(&output);
        assert!(
            stderr.contains("Build input not found")
                && stderr.contains(&missing.display().to_string()),
            "target/path grammar should keep `{target}` as the build target and next positional as PATH; got: {}",
            stderr
        );
        assert!(
            !stderr.contains("[E1900]") && !stderr.contains("Unknown option for build"),
            "existing single-target grammar must not be rejected as descriptor ambiguity; got: {}",
            stderr
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e32b_008_positional_target_plus_descriptor_selector_rejects_e1900() {
    let dir = unique_temp_dir("e32b_008_e1900");
    let src = dir.join("main.td");
    write_file(&src, "stdout(\"plain\")\n");

    let output = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(&src)
        .args(["--unit", "server-x"])
        .output()
        .expect("run taida build js <PATH> --unit");

    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("[E1900]")
            && stderr.contains("positional build target")
            && stderr.contains("Descriptor build mode"),
        "expected E1900 target/descriptor ambiguity, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e32b_008_descriptor_selectors_are_mutually_exclusive_e1901() {
    let dir = unique_temp_dir("e32b_008_e1901");
    let src = dir.join("main.td");
    write_file(&src, "stdout(\"plain\")\n");

    let cases: &[(&[&str], &str)] = &[
        (&["--unit", "server-x", "--plan", "web"], "unit+plan"),
        (&["--unit", "server-x", "--all-units"], "unit+all"),
        (&["--plan", "web", "--all-units"], "plan+all"),
    ];

    for (flags, label) in cases {
        let output = Command::new(taida_bin())
            .arg("build")
            .arg(&src)
            .args(*flags)
            .output()
            .unwrap_or_else(|_| panic!("run taida build descriptor selector case {label}"));

        assert_eq!(output.status.code(), Some(2), "case {label}");
        let stderr = stderr_text(&output);
        assert!(
            stderr.contains("[E1901]") && stderr.contains("mutually exclusive"),
            "expected E1901 for {label}, got: {}",
            stderr
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

// Project-root marker tightening fires `[E1902]` before the descriptor
// export check is reached when the temp directory has no parent project
// root. The descriptor-export probe needs to be reframed against a fixture
// rooted under a real `packages.tdm` / `taida.toml` marker before the pin
// can be reactivated.
#[test]
#[ignore = "Project root marker check pre-empts the descriptor export probe in /tmp; needs a rooted fixture"]
fn e32b_008_descriptor_mode_without_exported_descriptors_rejects_e1902() {
    let dir = unique_temp_dir("e32b_008_e1902");
    let src = dir.join("main.td");
    write_file(&src, "stdout(\"plain\")\n");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg(&src)
        .args(["--unit", "server-x"])
        .output()
        .expect("run taida build <PATH> --unit with no descriptors");

    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("[E1902]") && stderr.contains("exports no BuildUnit or BuildPlan"),
        "expected E1902 missing descriptor export, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e32b_008_unknown_unit_reports_e1903_with_candidates() {
    let dir = unique_temp_dir("e32b_008_e1903");
    let src = dir.join("main.td");
    write_file(
        &src,
        r#"
server <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= main
)
<<< server
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg(&src)
        .args(["--unit", "typo"])
        .output()
        .expect("run taida build <PATH> --unit typo");

    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("[E1903]")
            && stderr.contains("No exported BuildUnit named 'typo'")
            && stderr.contains("server-x"),
        "expected E1903 unit candidates, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e32b_008_unknown_plan_reports_e1904_with_candidates() {
    let dir = unique_temp_dir("e32b_008_e1904");
    let src = dir.join("main.td");
    write_file(
        &src,
        r#"
plan <= BuildPlan(
  name <= "web-release",
  units <= @[]
)
<<< plan
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg(&src)
        .args(["--plan", "typo"])
        .output()
        .expect("run taida build <PATH> --plan typo");

    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("[E1904]")
            && stderr.contains("No exported BuildPlan named 'typo'")
            && stderr.contains("web-release"),
        "expected E1904 plan candidates, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

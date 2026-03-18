//! CLI `taida build` and `taida transpile` tests.
//!
//! Covers: release gate, diag-format jsonl, package import resolution,
//! directory entry, stale output cleanup, checker integration.
//!
//! RCB-29: Split from `todo_cli.rs` (1764 lines) into responsibility-based test files.

mod common;

use common::{node_available, taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::process::Command;

// ── Release gate ──

#[test]
fn test_build_native_release_blocks_todo_and_stub() {
    let dir = unique_temp_dir("taida_release_build_native");
    let src = r#"
t <= TODO[Stub["ship later"]](id <= "REL-1", task <= "replace this")
t ]=> v
stdout(typeof(v))
"#;
    let input = dir.join("main.td");
    let bin = dir.join("app_bin");
    write_file(&input, src);

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg("--release")
        .arg(&input)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to run taida build --target native --release");

    assert!(
        !output.status.success(),
        "build --target native --release should fail when TODO/Stub exists"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Release gate failed"),
        "expected release gate message, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_js_release_blocks_todo_and_stub() {
    let dir = unique_temp_dir("taida_release_build");
    let src_dir = dir.join("src");
    let out_dir = dir.join("dist");
    fs::create_dir_all(&src_dir).expect("failed to create src dir");
    write_file(
        &src_dir.join("main.td"),
        r#"
x <= TODO[Int](id <= "REL-2", task <= "remove before release")
stdout(x.toString())
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg("--release")
        .arg("--outdir")
        .arg(&out_dir)
        .arg(&src_dir)
        .output()
        .expect("failed to run taida build --target js --release");

    assert!(
        !output.status.success(),
        "build --target js --release should fail when TODO/Stub exists"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Release gate failed"),
        "expected release gate message, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_native_release_blocks_todo_in_imported_module() {
    let dir = unique_temp_dir("taida_release_build_native_import");
    let main_td = dir.join("main.td");
    let dep_td = dir.join("dep.td");
    let bin = dir.join("app_bin");

    write_file(
        &main_td,
        r#"
>>> ./dep => @(v)
v ]=> out
stdout(out.toString())
"#,
    );
    write_file(
        &dep_td,
        r#"
v <= TODO[Int](id <= "REL-DEP", task <= "must be removed")
<<< @(v)
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg("--release")
        .arg(&main_td)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to run taida build --target native --release");

    assert!(
        !output.status.success(),
        "build --target native --release should fail when imported module has TODO/Stub"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Release gate failed"),
        "expected release gate message, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── Directory entry ──

#[test]
fn test_build_native_directory_default_entry() {
    let dir = unique_temp_dir("taida_build_native_dir_default");
    let project = dir.join("proj");
    let bin = dir.join("app_bin");
    fs::create_dir_all(&project).expect("failed to create project dir");
    write_file(
        &project.join("main.td"),
        r#"
stdout("hello native dir")
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(&project)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to run taida build --target native <DIR>");

    assert!(
        output.status.success(),
        "build should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bin.exists(), "expected native output binary to exist");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_native_directory_entry_override() {
    let dir = unique_temp_dir("taida_build_native_dir_entry");
    let project = dir.join("proj");
    let bin = dir.join("app_bin");
    fs::create_dir_all(&project).expect("failed to create project dir");
    write_file(
        &project.join("main.td"),
        r#"
stdout("default entry")
"#,
    );
    write_file(
        &project.join("custom_entry.td"),
        r#"
stdout("custom entry")
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(&project)
        .arg("--entry")
        .arg("custom_entry")
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to run taida build --target native <DIR> --entry");

    assert!(
        output.status.success(),
        "build should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bin.exists(), "expected native output binary to exist");

    let _ = fs::remove_dir_all(&dir);
}

// ── Transpile ──

#[test]
fn test_transpile_alias_produces_js_output() {
    let dir = unique_temp_dir("taida_transpile_e2e");
    let src = dir.join("main.td");
    let js_out = dir.join("main.mjs");
    write_file(&src, "stdout(\"transpile works\")\n");

    let output = Command::new(taida_bin())
        .arg("transpile")
        .arg(&src)
        .arg("-o")
        .arg(&js_out)
        .output()
        .expect("failed to run taida transpile");

    assert!(
        output.status.success(),
        "transpile should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(js_out.exists(), "JS output should exist");

    let _ = fs::remove_dir_all(&dir);
}

// ── Diag format ──

#[test]
fn test_build_js_diag_format_jsonl_outputs_parse_error_record() {
    let dir = unique_temp_dir("taida_build_jsonl_diag");
    let src = dir.join("broken.td");
    write_file(&src, "x <= ");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg("--diag-format")
        .arg("jsonl")
        .arg(&src)
        .output()
        .expect("failed to run taida build --diag-format jsonl");

    assert!(
        !output.status.success(),
        "build should fail for parse error in jsonl diag mode"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout
        .lines()
        .next()
        .expect("jsonl diagnostics should emit at least one line");
    let first: serde_json::Value =
        serde_json::from_str(first_line).expect("first diagnostic line should be valid json");
    assert_eq!(first["schema"], "taida.diagnostic.v1");
    assert_eq!(first["stream"], "compile");
    assert_eq!(first["kind"], "error");
    assert_eq!(first["stage"], "parse");
    assert_eq!(first["severity"], "ERROR");
    assert!(first.get("code").is_some());
    assert!(first.get("message").is_some());
    assert!(first.get("location").is_some());
    assert!(first.get("suggestion").is_some());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_diag_format_jsonl_emits_checker_error() {
    let dir = unique_temp_dir("taida_checker_jsonl");
    let src = dir.join("main.td");
    write_file(&src, "x <= 1\nx <= 2\n");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg("--diag-format")
        .arg("jsonl")
        .arg(&src)
        .output()
        .expect("build --diag-format jsonl");

    assert!(
        !output.status.success(),
        "build should fail with checker error in jsonl mode"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout
        .lines()
        .next()
        .expect("jsonl should emit at least one line");
    let diag: serde_json::Value =
        serde_json::from_str(first_line).expect("first jsonl line should be valid json");
    assert_eq!(diag["schema"], "taida.diagnostic.v1");
    assert_eq!(diag["stream"], "compile");
    assert_eq!(diag["kind"], "error");
    assert_eq!(diag["stage"], "type");
    assert_eq!(diag["code"], "E1501");

    let _ = fs::remove_dir_all(&dir);
}

// ── Checker stops backend ──

// C-8c: build stops on checker failure
#[test]
fn test_build_stops_on_checker_error() {
    let dir = unique_temp_dir("taida_checker_stops_backend");
    let src = dir.join("main.td");
    let bin = dir.join("out_bin");
    let js_out = dir.join("out.mjs");
    write_file(&src, "x <= 1\nx <= 2\n");

    // build --target js
    let build_js = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&src)
        .arg("-o")
        .arg(&js_out)
        .output()
        .expect("build --target js");

    // build --target native
    let build_native = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build --target native");

    // Both should fail with the same checker error
    for (name, out) in &[
        ("build --target js", &build_js),
        ("build --target native", &build_native),
    ] {
        assert!(
            !out.status.success(),
            "{} should fail on checker error",
            name
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[E1501]"),
            "{} should show E1501 error, got: {}",
            name,
            stderr
        );
    }

    // JS output file should NOT be created (backend didn't run)
    assert!(
        !js_out.exists(),
        "JS output should not exist when checker fails"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── Package import resolution ──

#[test]
fn test_build_js_fails_on_unresolved_package_import() {
    let dir = unique_temp_dir("taida_missing_pkg_import");
    let src = dir.join("main.td");
    let build_js_out = dir.join("build_out.mjs");

    write_file(&src, ">>> alice/missing => @(run)\nstdout(\"ok\")\n");
    write_file(&dir.join("packages.tdm"), ">>> alice/missing@a.1\n");

    let build_js = Command::new(taida_bin())
        .current_dir(&dir)
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&src)
        .arg("-o")
        .arg(&build_js_out)
        .output()
        .expect("build --target js");

    assert!(
        !build_js.status.success(),
        "build --target js should fail on unresolved package import"
    );
    let stderr = String::from_utf8_lossy(&build_js.stderr);
    assert!(
        stderr.contains("Could not resolve package import 'alice/missing'"),
        "build --target js should surface the unresolved package import, got: {}",
        stderr
    );

    assert!(
        !build_js_out.exists(),
        "build output should not exist when package import resolution fails"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_js_resolves_package_import_from_source_root_with_custom_output() {
    if !node_available() {
        return;
    }

    let dir = unique_temp_dir("taida_pkg_import_success");
    let project = dir.join("project");
    let caller = dir.join("caller");
    let dist = dir.join("dist");
    let dep_dir = project
        .join(".taida")
        .join("deps")
        .join("alice")
        .join("pkg");
    fs::create_dir_all(&caller).expect("create caller dir");
    fs::create_dir_all(&dist).expect("create dist dir");
    fs::create_dir_all(&dep_dir).expect("create dep dir");

    write_file(&project.join("packages.tdm"), ">>> alice/pkg@a.1\n");
    write_file(
        &project.join("main.td"),
        ">>> alice/pkg => @(greet)\nstdout(greet())\n",
    );
    write_file(
        &dep_dir.join("main.td"),
        "greet =\n  \"hello from pkg\"\n=> :Str\n\n<<< @(greet)\n",
    );

    let js_out = dist.join("app.mjs");
    let build_out = Command::new(taida_bin())
        .current_dir(&caller)
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(&js_out)
        .output()
        .expect("build --target js with custom output");

    assert!(
        build_out.status.success(),
        "build should succeed: {}",
        String::from_utf8_lossy(&build_out.stderr)
    );
    assert!(js_out.exists(), "expected JS output to exist");
    assert!(
        dep_dir.join("main.mjs").exists(),
        "dependency should be transpiled in-place"
    );

    let run_out = Command::new("node")
        .arg(&js_out)
        .output()
        .expect("node run");
    assert!(
        run_out.status.success(),
        "generated JS should run: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run_out.stdout).trim(),
        "hello from pkg"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── Stale output cleanup ──

#[test]
fn test_build_js_failure_does_not_leave_stale_local_module_outputs() {
    let dir = unique_temp_dir("taida_pkg_import_no_stale");
    let project = dir.join("project");
    let dist = dir.join("dist");
    fs::create_dir_all(&project).expect("create project dir");
    fs::create_dir_all(&dist).expect("create dist dir");

    write_file(&project.join("packages.tdm"), ">>> alice/missing@a.1\n");
    write_file(
        &project.join("main.td"),
        ">>> ./ok => @(value)\n>>> ./helper => @(run)\nstdout(value)\n",
    );
    write_file(&project.join("ok.td"), "value <= \"ok\"\n<<< @(value)\n");
    write_file(
        &project.join("helper.td"),
        ">>> alice/missing => @(missing)\nhelperValue =\n  \"bad\"\n=> :Str\n\n<<< @(helperValue)\n",
    );

    let build_out = Command::new(taida_bin())
        .current_dir(&project)
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(dist.join("app.mjs"))
        .output()
        .expect("build --target js with unresolved package import");

    assert!(
        !build_out.status.success(),
        "build should fail on unresolved package import"
    );
    let stderr = String::from_utf8_lossy(&build_out.stderr);
    assert!(
        stderr.contains("Could not resolve package import 'alice/missing'"),
        "expected unresolved package import error, got: {}",
        stderr
    );
    assert!(
        !dist.join("app.mjs").exists(),
        "main output should not exist after failed build"
    );
    assert!(
        !dist.join("ok.mjs").exists(),
        "successfully staged earlier local module output should not leak after failed build"
    );
    assert!(
        !dist.join("helper.mjs").exists(),
        "local module output should not exist after failed build"
    );

    let emitted_mjs = fs::read_dir(&dist)
        .expect("read dist dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("mjs"))
        .count();
    assert_eq!(emitted_mjs, 0, "no final .mjs outputs should remain");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_js_failure_does_not_leave_stale_dependency_outputs() {
    let dir = unique_temp_dir("taida_pkg_import_no_stale_deps");
    let project = dir.join("project");
    let deps = project.join(".taida").join("deps").join("alice");
    fs::create_dir_all(&deps).expect("create deps root");

    write_file(
        &project.join("packages.tdm"),
        ">>> alice/good@a.1\n>>> alice/pkg@a.1\n>>> alice/missing@a.1\n",
    );
    write_file(
        &project.join("main.td"),
        ">>> alice/pkg => @(greet)\nstdout(greet())\n",
    );

    let good_dir = deps.join("good");
    let pkg_dir = deps.join("pkg");
    fs::create_dir_all(&good_dir).expect("create good dep dir");
    fs::create_dir_all(&pkg_dir).expect("create pkg dep dir");

    write_file(
        &good_dir.join("main.td"),
        "greet =\n  \"hello from good\"\n=> :Str\n\n<<< @(greet)\n",
    );
    write_file(
        &pkg_dir.join("main.td"),
        ">>> alice/good => @(greet)\n>>> alice/missing => @(missing)\n\nwelcome =\n  greet()\n=> :Str\n\n<<< @(welcome)\n",
    );

    let build_out = Command::new(taida_bin())
        .current_dir(&project)
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("dist").join("app.mjs"))
        .output()
        .expect("build --target js with bad dep graph");

    assert!(
        !build_out.status.success(),
        "build should fail when a dependency import cannot be resolved"
    );
    let stderr = String::from_utf8_lossy(&build_out.stderr);
    assert!(
        stderr.contains("Could not resolve package import 'alice/missing'"),
        "expected unresolved dependency import error, got: {}",
        stderr
    );

    assert!(
        !good_dir.join("main.mjs").exists(),
        "successfully transpiled dependency output should not leak after failed build"
    );
    assert!(
        !pkg_dir.join("main.mjs").exists(),
        "failing dependency output should not exist after failed build"
    );
    assert!(
        !project.join("dist").join("app.mjs").exists(),
        "main output should not exist after failed build"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── C-6a: same-scope duplicate check vs build consistency ──

#[test]
fn test_same_scope_duplicate_check_vs_build_consistency() {
    let dir = unique_temp_dir("taida_c6a_consistency");
    let src = dir.join("main.td");
    write_file(&src, "x <= 1\nx <= 2\nstdout(x.toString())\n");

    // taida check
    let check_out = Command::new(taida_bin())
        .arg("check")
        .arg(&src)
        .output()
        .expect("check");

    // taida build --target js
    let build_out = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&src)
        .output()
        .expect("build");

    // Both should fail
    assert!(!check_out.status.success(), "check should fail for E1501");
    assert!(!build_out.status.success(), "build should fail for E1501");

    // Both should report E1501
    let check_stderr = String::from_utf8_lossy(&check_out.stderr);
    let build_stderr = String::from_utf8_lossy(&build_out.stderr);
    assert!(
        check_stderr.contains("[E1501]")
            || String::from_utf8_lossy(&check_out.stdout).contains("E1501"),
        "check should report E1501, got stderr: {}, stdout: {}",
        check_stderr,
        String::from_utf8_lossy(&check_out.stdout)
    );
    assert!(
        build_stderr.contains("[E1501]"),
        "build should report E1501, got: {}",
        build_stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

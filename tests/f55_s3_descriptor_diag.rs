//! Build-descriptor runtime-use diagnostic ([E1532]).
//!
//! Build-driver descriptors (`BuildUnit` / `BuildPlan` / `AssetBundle` /
//! `RouteAsset` / `BuildHook`) are consumed by `taida build --unit` /
//! `--plan` / `--all-units`, which parse and match the descriptor module's
//! AST directly without invoking the type checker. Treating a descriptor as
//! an ordinary runtime value (passing it to a builtin / user function, a
//! conversion / mold argument, an operator operand, etc.) is rejected by the
//! checker with `[E1532]`.
//!
//! The diagnostic is checker-common, so these tests drive the interpreter
//! entry point (`taida <FILE>`) — the same flow as the other type-error
//! regression suites — and additionally pin that the descriptor *build* path
//! (`taida build --unit`) keeps working because it bypasses the checker.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::path::Path;
use std::process::Command;

fn stderr_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Run `taida <FILE>` (interpreter) on a fixture written into a fresh temp dir.
fn run_interp(label: &str, source: &str) -> std::process::Output {
    let dir = unique_temp_dir(label);
    let src = dir.join("main.td");
    write_file(&src, source);
    let output = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run taida interpreter");
    let _ = fs::remove_dir_all(&dir);
    output
}

fn assert_e1532(output: &std::process::Output, label: &str) {
    assert!(
        !output.status.success(),
        "{label}: descriptor runtime use must be rejected\nstdout={}\nstderr={}",
        stdout_text(output),
        stderr_text(output)
    );
    let stderr = stderr_text(output);
    assert!(
        stderr.contains("[E1532]"),
        "{label}: expected [E1532], got: {stderr}"
    );
}

fn assert_no_e1532(output: &std::process::Output, label: &str) {
    let stderr = stderr_text(output);
    assert!(
        !stderr.contains("[E1532]"),
        "{label}: descriptor in a valid position must not trigger [E1532], got: {stderr}"
    );
}

// ── invalid (reject) ────────────────────────────────────────────────

/// Invalid #1: descriptor passed to a builtin (`stdout`) argument.
#[test]
fn e1532_rejects_descriptor_as_builtin_argument() {
    let output = run_interp(
        "f55_s3_builtin_arg",
        r#"serverMain <= "x"
webUnit <= BuildUnit(name <= "u", target <= "native", entry <= serverMain)
stdout(webUnit)
"#,
    );
    assert_e1532(&output, "builtin arg");
}

/// Invalid #2: descriptor passed to a user-defined function argument.
#[test]
fn e1532_rejects_descriptor_as_user_function_argument() {
    let output = run_interp(
        "f55_s3_user_fn_arg",
        r#"serverMain <= "x"
useIt u: @(name: Str) = u.name => :Str
webUnit <= BuildUnit(name <= "u", target <= "native", entry <= serverMain)
stdout(useIt(webUnit))
"#,
    );
    assert_e1532(&output, "user function arg");
}

/// Invalid #3: descriptor passed as a conversion / user-mold argument
/// (`Str[...]()`).
#[test]
fn e1532_rejects_descriptor_as_mold_argument() {
    let output = run_interp(
        "f55_s3_mold_arg",
        r#"serverMain <= "x"
webUnit <= BuildUnit(name <= "u", target <= "native", entry <= serverMain)
s <= Str[webUnit]()
stdout(s)
"#,
    );
    assert_e1532(&output, "mold arg");
}

/// Invalid #4: descriptor used as an operator operand via field access.
/// (`webUnit.name == "u"` constructs the descriptor inline in operand position.)
#[test]
fn e1532_rejects_descriptor_in_operator_operand() {
    let output = run_interp(
        "f55_s3_operand",
        r#"serverMain <= "x"
match <= BuildUnit(name <= "u", target <= "native", entry <= serverMain).name == "u"
stdout(match.toString())
"#,
    );
    assert_e1532(&output, "operator operand field access");
}

// ── valid (pass) ────────────────────────────────────────────────────

/// Valid #1: a top-level binding RHS descriptor exported with `<<<`.
#[test]
fn e1532_allows_top_level_export_of_descriptor() {
    let output = run_interp(
        "f55_s3_export",
        r#"serverMain <= "x"
webUnit <= BuildUnit(name <= "u", target <= "native", entry <= serverMain)
<<< webUnit
"#,
    );
    assert_no_e1532(&output, "top-level export");
    assert!(
        output.status.success(),
        "top-level export of a descriptor should type-check\nstderr={}",
        stderr_text(&output)
    );
}

/// Valid #2: a descriptor nested in another descriptor's field
/// (`RouteAsset` inside `BuildUnit.assets`, and a `BuildUnit` reference inside
/// `BuildPlan.units`).
#[test]
fn e1532_allows_descriptor_nested_in_descriptor_field() {
    let output = run_interp(
        "f55_s3_nested",
        r#"serverMain <= "x"
frontendA <= BuildUnit(name <= "fa", target <= "wasm-min", entry <= serverMain)
serverX <= BuildUnit(
  name <= "sx",
  target <= "native",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/app.wasm", unit <= frontendA)]
)
plan <= BuildPlan(name <= "p", units <= @[serverX])
<<< plan
"#,
    );
    assert_no_e1532(&output, "nested descriptor field");
    assert!(
        output.status.success(),
        "nested descriptors should type-check\nstderr={}",
        stderr_text(&output)
    );
}

/// Valid #3 (regression): the descriptor build path (`taida build --unit`)
/// bypasses the checker, so an importless descriptor module still builds even
/// though it never registers `[E1532]`. Uses the JS target so the assertion
/// turns on the descriptor-driver / checker-bypass behaviour rather than the
/// native C toolchain.
#[test]
fn e1532_does_not_break_descriptor_build_unit_path() {
    let dir = unique_temp_dir("f55_s3_build_unit");
    write_file(&dir.join("packages.tdm"), "");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#">>> ./server.td => @(serverMain)
serverX <= BuildUnit(name <= "server-x", target <= "js", entry <= serverMain)
<<< serverX
"#,
    );

    let output = build_descriptor(&dir, &["main.td", "--unit", "server-x"]);
    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "descriptor `--unit` build must keep working (checker is bypassed)\nstdout={}\nstderr={}",
        stdout_text(&output),
        stderr_text(&output)
    );
    assert!(
        !stderr_text(&output).contains("[E1532]"),
        "descriptor build path must not emit [E1532], got: {}",
        stderr_text(&output)
    );
}

fn build_descriptor(project: &Path, args: &[&str]) -> std::process::Output {
    Command::new(taida_bin())
        .current_dir(project)
        .arg("build")
        .args(args)
        .output()
        .expect("taida build descriptor")
}

// ── scope shadowing (review follow-up) ──────────────────────────────

/// Valid #4: a function parameter with the same name as a top-level
/// descriptor binding shadows it — the parameter is an ordinary runtime
/// value and must not be flagged.
#[test]
fn e1532_allows_function_param_shadowing_descriptor_name() {
    let output = run_interp(
        "f55_s3_param_shadow",
        r#"serverMain <= "x"
webUnit <= BuildUnit(name <= "u", target <= "native", entry <= serverMain)
useName webUnit: Str = webUnit => :Str
stdout(useName("hello"))
<<< webUnit
"#,
    );
    assert_no_e1532(&output, "function param shadow");
    assert!(
        output.status.success(),
        "a Str parameter named like a descriptor binding must type-check and run\nstdout={}\nstderr={}",
        stdout_text(&output),
        stderr_text(&output)
    );
    assert!(
        stdout_text(&output).contains("hello"),
        "the shadowing parameter must carry the runtime argument, got: {}",
        stdout_text(&output)
    );
}

/// Valid #5: a lambda parameter with a descriptor binding's name shadows it
/// inside the lambda body.
#[test]
fn e1532_allows_lambda_param_shadowing_descriptor_name() {
    let output = run_interp(
        "f55_s3_lambda_shadow",
        r#"serverMain <= "x"
webUnit <= BuildUnit(name <= "u", target <= "native", entry <= serverMain)
shout <= _ webUnit: Str = webUnit + "!"
stdout(shout("hey"))
<<< webUnit
"#,
    );
    assert_no_e1532(&output, "lambda param shadow");
    assert!(
        output.status.success(),
        "a lambda parameter named like a descriptor binding must type-check and run\nstdout={}\nstderr={}",
        stdout_text(&output),
        stderr_text(&output)
    );
    assert!(
        stdout_text(&output).contains("hey!"),
        "the shadowing lambda parameter must carry the runtime argument, got: {}",
        stdout_text(&output)
    );
}

/// Invalid #5: the scope-shadow allowance must not leak — a function body
/// that reaches the *top-level* descriptor binding (no shadow in scope) is
/// still a runtime use.
#[test]
fn e1532_still_rejects_descriptor_use_inside_function_without_shadow() {
    let output = run_interp(
        "f55_s3_fn_body_no_shadow",
        r#"serverMain <= "x"
webUnit <= BuildUnit(name <= "u", target <= "native", entry <= serverMain)
leak x: Str = Str[webUnit]() => :Str
stdout(leak("hi"))
"#,
    );
    assert_e1532(&output, "function body without shadow");
}

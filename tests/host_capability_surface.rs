mod common;

use std::process::Command;

use common::{taida_bin, unique_temp_dir};
use taida::interpreter::{AsyncStatus, HostCallMockStep, Interpreter, Value};
use taida::types::{CompileTarget, TypeChecker};

fn run_way_check(source: &str, label: &str) -> (bool, String) {
    let dir = unique_temp_dir(label);
    let td_path = dir.join("main.td");
    std::fs::write(&td_path, source).expect("write host capability fixture");

    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&td_path)
        .output()
        .expect("spawn taida way check");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
    (output.status.success(), combined)
}

fn run_taida_source(source: &str, label: &str) -> (bool, String) {
    let dir = unique_temp_dir(label);
    let td_path = dir.join("main.td");
    std::fs::write(&td_path, source).expect("write host capability runtime fixture");

    let output = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("spawn taida runtime");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
    (output.status.success(), combined)
}

fn typecheck_with_manifest(source: &str, manifest: &[(&str, &str)]) -> Vec<String> {
    let (program, parse_errors) = taida::parser::parse(source);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    let mut checker = TypeChecker::new();
    checker.set_compile_target(CompileTarget::WasmEdge);
    checker.set_host_capability_manifest(manifest.iter().copied());
    checker.check_program(&program);
    checker
        .errors
        .iter()
        .map(|err| err.message.clone())
        .collect()
}

fn eval_with_host_fixture(source: &str, capability: &str, steps: Vec<HostCallMockStep>) -> Value {
    let (program, parse_errors) = taida::parser::parse(source);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    let mut interpreter = Interpreter::new();
    interpreter.set_host_capability_mock_steps(capability, steps);
    interpreter
        .eval_program(&program)
        .expect("host capability fixture should evaluate")
}

#[test]
fn host_call_cage_surface_type_checks_with_wired_steps() {
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, HostCall, HostStep, HostCapability)

D1Database <= "cloudflare/d1"

callHost req: WebRequest =
  db <= HostCapability["DB", D1Database]()
  Cage[db, HostCall[@[HostStep["prepare", @["select * from items where id = ?"]](), HostStep["bind", @[req.rawQuery]](), HostStep["fetch", @[req]]()], WebResponse]()]()
=> :Async[WebResponse]
"#;

    let (ok, output) = run_way_check(source, "host_capability_surface_ok");
    assert!(
        ok,
        "expected HostCall Cage surface to type-check:\n{}",
        output
    );
    assert!(
        output.contains("errors=0") && !output.contains("[ERROR]"),
        "expected clean HostCall check:\n{}",
        output
    );
}

#[test]
fn host_step_args_must_be_wired() {
    let source = r#">>> taida-lang/abi => @(HostStep)

bad <= HostStep["bad", @[_ x: Int = x]]()
"#;

    let (ok, output) = run_way_check(source, "host_step_unwired_args");
    assert!(!ok, "function arg should not satisfy Wired:\n{}", output);
    assert!(
        output.contains("[E3601]") && output.contains("HostStep args"),
        "diagnostic should identify HostStep args as the Wired boundary:\n{}",
        output
    );
}

#[test]
fn host_step_args_must_be_a_list() {
    let source = r#">>> taida-lang/abi => @(HostStep)

bad <= HostStep["get", "key"]()
"#;

    let (ok, output) = run_way_check(source, "host_step_args_not_list");
    assert!(!ok, "scalar HostStep args should be rejected:\n{}", output);
    assert!(
        output.contains("[E3601]") && output.contains("HostStep args must be"),
        "diagnostic should identify HostStep args list shape:\n{}",
        output
    );
}

#[test]
fn host_step_args_reject_empty_pack_payload() {
    let source = r#">>> taida-lang/abi => @(HostStep)

bad <= HostStep["bad", @[@()]]()
"#;

    let (ok, output) = run_way_check(source, "host_step_empty_pack_args");
    assert!(
        !ok,
        "empty pack payload should not satisfy Wired:\n{}",
        output
    );
    assert!(
        output.contains("[E3601]") && output.contains("HostStep args"),
        "diagnostic should reject @() through HostStep args:\n{}",
        output
    );
}

#[test]
fn host_step_method_must_be_compile_time_string() {
    let source = r#">>> taida-lang/abi => @(HostStep)

bad <= HostStep[toString("get"), @[]]()
"#;

    let (ok, output) = run_way_check(source, "host_step_dynamic_method");
    assert!(
        !ok,
        "dynamic HostStep method should be rejected:\n{}",
        output
    );
    assert!(
        output.contains("[E3603]") && output.contains("HostStep method"),
        "diagnostic should identify HostStep method identity:\n{}",
        output
    );
}

#[test]
fn host_step_list_rejects_non_step_elements() {
    let source = r#">>> taida-lang/abi => @(HostStep)

bad <= @[HostStep["ok", @[]](), "not-a-step"]
"#;

    let (ok, output) = run_way_check(source, "host_step_mixed_list");
    assert!(!ok, "mixed HostStep list should be rejected:\n{}", output);
    assert!(
        output.contains("[E3602]"),
        "diagnostic should identify HostStep list shape:\n{}",
        output
    );
}

#[test]
fn host_call_output_must_be_wired() {
    let source = r#">>> taida-lang/abi => @(HostCall, HostStep, HostCapability)

Kind <= "mock/kind"

bad =
  cap <= HostCapability["CAP", Kind]()
  Cage[cap, HostCall[@[HostStep["get", @[]]()], Async[Str]()]()]()
=> :Async[Str]
"#;

    let (ok, output) = run_way_check(source, "host_call_unwired_out");
    assert!(
        !ok,
        "Async output target should not satisfy Wired:\n{}",
        output
    );
    assert!(
        output.contains("[E3601]") && output.contains("HostCall output"),
        "diagnostic should identify HostCall output:\n{}",
        output
    );
}

#[test]
fn host_call_unit_output_is_value_absence_escape() {
    let source = r#">>> taida-lang/abi => @(HostCall, HostStep, HostCapability)

Kind <= "mock/kind"

bad =
  cap <= HostCapability["CAP", Kind]()
  Cage[cap, HostCall[@[HostStep["get", @[]]()], Unit]()]()
=> :Async[Str]
"#;

    let (ok, output) = run_way_check(source, "host_call_unit_out");
    assert!(!ok, "Unit HostCall output should be rejected:\n{}", output);
    assert!(
        output.contains("[E1520]"),
        "diagnostic should identify Unit output as value absence:\n{}",
        output
    );
}

#[test]
fn host_cage_subject_must_be_host_capability() {
    let source = r#">>> taida-lang/abi => @(HostCall, HostStep)

bad =
  Cage["not-a-capability", HostCall[@[HostStep["get", @[]]()], Str]()]()
=> :Async[Str]
"#;

    let (ok, output) = run_way_check(source, "host_cage_bad_subject");
    assert!(
        !ok,
        "Host Cage subject should require HostCapability:\n{}",
        output
    );
    assert!(
        output.contains("[E1517]") && output.contains("HostCapability"),
        "diagnostic should identify HostCapability subject requirement:\n{}",
        output
    );
}

#[test]
fn host_descriptor_import_alias_is_rejected() {
    let source = r#">>> taida-lang/abi => @(HostCapability => HC)

cap <= HC["CAP", "mock/kind"]()
"#;

    let (ok, output) = run_way_check(source, "host_descriptor_alias");
    assert!(
        !ok,
        "host descriptor aliases should be rejected:\n{}",
        output
    );
    assert!(
        output.contains("[E1502]") && output.contains("cannot be imported with an alias"),
        "diagnostic should identify descriptor alias restriction:\n{}",
        output
    );
}

#[test]
fn host_capability_import_executes_as_pack_sentinel() {
    let source = r#">>> taida-lang/abi => @(HostCapability)

cap <= HostCapability["CAP", "mock/kind"]()
cap.name
"#;

    let (ok, output) = run_taida_source(source, "host_capability_runtime_pack");
    assert!(ok, "HostCapability import should run:\n{}", output);
    assert!(
        output.contains("CAP"),
        "HostCapability runtime pack should expose name field:\n{}",
        output
    );
}

#[test]
fn host_call_fixture_returns_async_output() {
    let source = r#">>> taida-lang/abi => @(HostCall, HostStep, HostCapability)

Kind <= "mock/kind"
cap <= HostCapability["CAP", Kind]()
Cage[cap, HostCall[@[HostStep["get", @["k"]](), HostStep["decode", @[]]()], Str]()]() >=> out
out
"#;

    let result = eval_with_host_fixture(
        source,
        "CAP",
        vec![
            HostCallMockStep {
                method: "get".to_string(),
                args: vec![Value::str("k".to_string())],
                result: Value::str("raw".to_string()),
            },
            HostCallMockStep {
                method: "decode".to_string(),
                args: Vec::new(),
                result: Value::str("value".to_string()),
            },
        ],
    );

    assert_eq!(result, Value::str("value".to_string()));
}

#[test]
fn host_call_fixture_missing_mock_returns_rejected_async() {
    let source = r#">>> taida-lang/abi => @(HostCall, HostStep, HostCapability)

Kind <= "mock/kind"
cap <= HostCapability["MISSING", Kind]()
Cage[cap, HostCall[@[HostStep["get", @[]]()], Str]()]()
"#;

    let (program, parse_errors) = taida::parser::parse(source);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    let mut interpreter = Interpreter::new();
    let result = interpreter
        .eval_program(&program)
        .expect("missing fixture should be a rejected Async value");

    let Value::Async(async_value) = result else {
        panic!("HostCall should return Async, got {:?}", result);
    };
    assert_eq!(async_value.status, AsyncStatus::Rejected);
    let Value::Error(error) = *async_value.error else {
        panic!(
            "rejected HostCall should carry Error, got {:?}",
            async_value.error
        );
    };
    assert_eq!(error.error_type, "HostCapabilityError");
    assert!(
        error
            .message
            .contains("No interpreter host fixture registered"),
        "unexpected error message: {}",
        error.message
    );
}

#[test]
fn host_capability_manifest_accepts_declared_pair() {
    let source = r#">>> taida-lang/abi => @(HostCapability)

D1Database <= "cloudflare/d1"

db <= HostCapability["DB", D1Database]()
"#;

    let errors = typecheck_with_manifest(source, &[("DB", "cloudflare/d1")]);
    assert!(
        errors.is_empty(),
        "declared HostCapability pair should type-check: {:?}",
        errors
    );
}

#[test]
fn host_capability_manifest_rejects_undeclared_pair() {
    let source = r#">>> taida-lang/abi => @(HostCapability)

Kind <= "mock/kind"

cap <= HostCapability["MISSING", Kind]()
"#;

    let errors = typecheck_with_manifest(source, &[("CAP", "mock/kind")]);
    assert!(
        errors.iter().any(|msg| msg.contains("[E3603]"))
            && errors.iter().any(|msg| msg.contains("MISSING")),
        "undeclared HostCapability pair should produce E3603: {:?}",
        errors
    );
}

#[test]
fn host_capability_manifest_requires_compile_time_pair() {
    let source = r#">>> taida-lang/abi => @(HostCapability)

cap <= HostCapability[toString("CAP"), "mock/kind"]()
"#;

    let errors = typecheck_with_manifest(source, &[("CAP", "mock/kind")]);
    assert!(
        errors.iter().any(|msg| msg.contains("[E3603]"))
            && errors
                .iter()
                .any(|msg| msg.contains("compile-time Str values")),
        "dynamic HostCapability pair should produce E3603: {:?}",
        errors
    );
}

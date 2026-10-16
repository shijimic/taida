// F62B-021: generic Out passthrough — host-call helpers as generic
// functions.
//
//   Stage 1: explicit-type-argument calls. `genfn[T1, T2](args)` binds the
//   declared type parameters directly, so return-position-only type
//   parameters work and inference is bypassed.
//
//   Stage 2: a generic body may pass a type parameter into a host-call Out
//   slot (`Uncage[b, m, T]()` / `HostCall[steps, T]()`). The Out schema is
//   compile-time per call site: codegen gives such functions hidden
//   `#schema_{T}` string parameters and explicit call sites append
//   the resolved schema descriptors (dictionary passing). Inference-form
//   calls to such functions are rejected at check time.
//
// This is the abstraction the D1 access layer needed: a `queryAll`-style
// helper instead of a fully spelled-out HostCall per query.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::process::{Command, Output};
use taida::interpreter::{HostCallMockStep, Interpreter, Value};

fn eval_with_fixture(source: &str, capability: &str, steps: Vec<HostCallMockStep>) -> Value {
    let (program, parse_errors) = taida::parser::parse(source);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);
    let mut interpreter = Interpreter::new();
    interpreter.set_host_capability_mock_steps(capability, steps);
    interpreter
        .eval_program(&program)
        .expect("host capability fixture should evaluate")
}

fn run_interp(label: &str, source: &str) -> Output {
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

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// ── stage 1: explicit type arguments ─────────────────────────────

/// `ident[Int](5)` binds T explicitly; types erase at runtime.
#[test]
fn explicit_type_arguments_call_generic_functions() {
    let output = run_interp(
        "f62b021_explicit",
        concat!(
            "ident[T] x: T = x => :T\n",
            "y <= ident[Int](5)\n",
            "stdout(y.toString())\n",
            "pick[T] a: T  b: T = a => :T\n",
            "stdout(pick[Str](\"L\", \"R\"))\n",
        ),
    );
    assert!(
        output.status.success(),
        "explicit calls must run\nstderr={}",
        stderr_text(&output)
    );
    assert_eq!(stdout_text(&output), "5\nL\n");
}

/// A return-position-only type parameter is legal at definition time and
/// callable with explicit arguments; the inference form still fails.
#[test]
fn return_only_type_param_needs_explicit_arguments() {
    let ok = run_interp(
        "f62b021_return_only_ok",
        "make[T] =\n  1\n=> :T\n\nv <= make[Int]()\nstdout(v.toString())\n",
    );
    assert!(
        ok.status.success(),
        "explicit call must bind the return-only param\nstderr={}",
        stderr_text(&ok)
    );
    assert_eq!(stdout_text(&ok), "1\n");

    let bad = run_interp(
        "f62b021_return_only_bad",
        "make[T] =\n  1\n=> :T\n\nv <= make()\n",
    );
    assert!(!bad.status.success());
    assert!(
        stderr_text(&bad).contains("[E1510]")
            && stderr_text(&bad).contains("could not infer type parameter(s): T"),
        "inference form must fail, got: {}",
        stderr_text(&bad)
    );
}

/// Wrong type-argument count and wrong value-argument types are caught.
#[test]
fn explicit_call_arity_and_types_validated() {
    let wrong_types = run_interp(
        "f62b021_wrong_targs",
        "ident[T] x: T = x => :T\ny <= ident[Int, Str](5)\n",
    );
    assert!(!wrong_types.status.success());
    assert!(
        stderr_text(&wrong_types).contains("[E1505]"),
        "type-arg count must be validated, got: {}",
        stderr_text(&wrong_types)
    );

    let wrong_val = run_interp(
        "f62b021_wrong_val",
        "ident[T] x: T = x => :T\ny <= ident[Int](\"s\")\n",
    );
    assert!(!wrong_val.status.success());
    assert!(
        stderr_text(&wrong_val).contains("[E1506]"),
        "value-arg type must be validated, got: {}",
        stderr_text(&wrong_val)
    );
}

// ── stage 2: host-call Out passthrough ───────────────────────────

const QUERY_HELPER: &str = r#"queryAll[T] db: CageBuilder  sql: Str =
  db => InCage[_, "prepare", @[sql]]() => Uncage[_, "all", T]() >=> rows
  rows
=> :T

cap <= HostCapability["CAP", "mock/kind"]()
base <= Cage[cap]()
out <= queryAll[Str](base, "select 1")
out
"#;

fn query_fixture() -> Vec<HostCallMockStep> {
    vec![
        HostCallMockStep {
            method: "prepare".to_string(),
            args: vec![Value::str("select 1".to_string())],
            result: Value::str("stmt".to_string()),
        },
        HostCallMockStep {
            method: "all".to_string(),
            args: Vec::new(),
            result: Value::str("row".to_string()),
        },
    ]
}

/// The headline abstraction: a generic query helper whose body passes T
/// into the Uncage Out slot, resolved per call site.
#[test]
fn generic_query_helper_resolves_through_fixture() {
    let result = eval_with_fixture(QUERY_HELPER, "CAP", query_fixture());
    assert_eq!(result, Value::str("row".to_string()));
}

/// Inference-form calls to a schema-passing generic are rejected at check
/// time — there is no schema to send without the explicit types.
#[test]
fn schema_passing_generic_rejects_inference_calls() {
    let output = run_interp(
        "f62b021_inference_rejected",
        concat!(
            "queryAll[T] db: CageBuilder  sql: Str =\n",
            "  db => InCage[_, \"prepare\", @[sql]]() => Uncage[_, \"all\", T]() >=> rows\n",
            "  rows\n",
            "=> :T\n",
            "\n",
            "cap <= HostCapability[\"DB\", \"mock/kind\"]()\n",
            "base <= Cage[cap]()\n",
            "result <= queryAll(base, \"select 1\")\n",
        ),
    );
    assert!(!output.status.success());
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("[E1510]") && stderr.contains("host-call Out slot"),
        "expected the explicit-arguments requirement, got: {stderr}"
    );
}

/// The native backend lowers the schema through the hidden parameter and
/// reaches the deterministic session-less rejection (proving the whole
/// dictionary-passing path compiles and runs).
#[test]
fn native_generic_out_passthrough_compiles_and_runs() {
    let dir = unique_temp_dir("f62b021_native");
    let src = dir.join("main.td");
    write_file(
        &src,
        concat!(
            "queryAll[T] db: CageBuilder  sql: Str =\n",
            "  db => InCage[_, \"prepare\", @[sql]]() => Uncage[_, \"all\", T]() >=> rows\n",
            "  rows\n",
            "=> :T\n",
            "\n",
            "cap <= HostCapability[\"DB\", \"mock/kind\"]()\n",
            "base <= Cage[cap]()\n",
            "result <= queryAll[Str](base, \"select 1\")\n",
            "stdout(result)\n",
        ),
    );
    let bin = dir.join("main_bin");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("native build");
    assert!(
        build.status.success(),
        "native build must succeed\nstderr={}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("run native");
    let _ = fs::remove_dir_all(&dir);
    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("host capabilities are not available"),
        "expected the session-less rejection, got: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

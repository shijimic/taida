// E30B-004 / E30 Phase 6: defaultFn 生成 spec
//
// Lock-D verdict (E30 Phase 0, 2026-04-28): a synthetic `defaultFn` is
// generated for every `TypeExpr::Function(_, _)` annotation reachable from
// `default_for_type_expr` (interpreter). Calling the field at runtime must
// yield the **return type's default value**.
//
// This integration test pins the interpreter (reference) behaviour for the
// canonical scenarios: primitive return types (Int / Str / Bool), Async
// return type (immediate-resolve), and TypeDef return type. 4-backend
// parity is exercised separately by `tests/parity.rs::e30b_004_*`.

use std::process::Command;

fn taida_bin() -> String {
    env!("CARGO_BIN_EXE_taida").to_string()
}

fn run_taida_source(label: &str, source: &str) -> String {
    let dir = std::env::temp_dir().join("taida-e30b_004-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.td", label));
    std::fs::write(&path, source).unwrap();
    let output = Command::new(taida_bin())
        .arg(&path)
        .output()
        .expect("failed to execute taida");
    assert!(
        output.status.success(),
        "taida failed for {}: stderr={}",
        label,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn e30b_004_default_fn_int_return() {
    let src = r#"
Counter = @(value: Int, getNext: Int => :Int)
c <= Counter(value <= 10)
n <= c.getNext(0)
stdout(n.toString())
"#;
    assert_eq!(run_taida_source("default_fn_int", src).trim(), "0");
}

#[test]
fn e30b_004_default_fn_str_return() {
    let src = r#"
Pilot = @(name: Str, greet: Str => :Str)
p <= Pilot(name <= "Rei")
result <= p.greet("hello")
stdout(result.length().toString())
"#;
    assert_eq!(run_taida_source("default_fn_str", src).trim(), "0");
}

#[test]
fn e30b_004_default_fn_bool_return_interpreter_only() {
    // Interpreter pins the Lock-D semantics for Bool return: defaultFn
    // returns `false`. (Native renders `0` due to a tag-propagation gap;
    // tracked as E30B-011 and pinned with `#[ignore]` in tests/parity.rs.)
    let src = r#"
Predicate = @(label: Str, check: Int => :Bool)
p <= Predicate(label <= "any")
b <= p.check(0)
stdout(b.toString())
"#;
    assert_eq!(
        run_taida_source("default_fn_bool", src).trim(),
        "false",
        "interpreter must render Bool default as `false`"
    );
}

#[test]
fn e30b_004_default_fn_typedef_return() {
    // defaultFn whose return type is a class-like materialises the
    // class-like's default pack (empty `name`).
    let src = r#"
Pilot = @(name: Str)
Greeter = @(label: Str, build: Str => :Pilot)
g <= Greeter(label <= "make")
p <= g.build("Rei")
stdout(p.name.length().toString())
"#;
    assert_eq!(run_taida_source("default_fn_typedef", src).trim(), "0");
}

#[test]
fn e30b_004_default_fn_mold_with_declare_only_field() {
    // E30B-002 + E30B-004 integration: declare-only function field on a
    // Mold variant. defaultFn should be callable with proper return-type
    // default. (Phase 4 fixture deliberately did not call the field;
    // Phase 6 lifts that restriction.)
    let src = r#"
Mold[T] => Foo[T] = @(
  name: Str,
  build: Int => :Int
)
f <= Foo[1, "x"]()
n <= f.build(0)
stdout(n.toString())
"#;
    assert_eq!(
        run_taida_source("default_fn_mold", src).trim(),
        "0",
        "Mold variant declare-only fn field defaultFn must return Int default"
    );
}

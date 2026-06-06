mod common;

use std::process::Command;

use common::{taida_bin, unique_temp_dir};

fn run_way_check(source: &str, label: &str) -> (bool, String) {
    let dir = unique_temp_dir(label);
    let td_path = dir.join("main.td");
    std::fs::write(&td_path, source).expect("write Wired fixture");

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

#[test]
fn wired_constraint_accepts_wire_shapes() {
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)

Thing = @(name: Str, bytes: Bytes)
Mold[T <= :Wired[T]] => WireBox[T <= :Wired[T]] = @(marker: Int <= 0)

wireId[T <= :Wired[T]] value: T =
  value
=> :T

probe req: WebRequest =
  boxReq <= WireBox[req]()
  boxResp <= WireBox[text("ok")]()
  boxPack <= WireBox[@(name <= "n", bytes <= req.body)]()
  boxList <= WireBox[@[1, 2, 3]]()
  thing <= Thing(name <= "item", bytes <= req.body)
  boxNamed <= WireBox[thing]()
  roundTrip <= wireId(@(name <= "round", bytes <= req.body))
  req.rawQuery
=> :Str
"#;

    let (ok, output) = run_way_check(source, "wired_constraint_accept");
    assert!(ok, "expected Wired shapes to type-check:\n{}", output);
    assert!(
        output.contains("errors=0") && !output.contains("[ERROR]"),
        "expected clean Wired check:\n{}",
        output
    );
}

#[test]
fn wired_constraint_rejects_function_values() {
    let source = r#"Mold[T <= :Wired[T]] => WireBox[T <= :Wired[T]] = @(marker: Int <= 0)

bad <= WireBox[_ x: Int = x]()
"#;

    let (ok, output) = run_way_check(source, "wired_constraint_function_reject");
    assert!(
        !ok,
        "function values must not satisfy Wired[T]:\n{}",
        output
    );
    assert!(
        output.contains("[E3601]") && output.contains("Wired"),
        "diagnostic should identify the Wired constraint:\n{}",
        output
    );
}

#[test]
fn wired_constraint_rejects_async_values() {
    let source = r#"Mold[T <= :Wired[T]] => WireBox[T <= :Wired[T]] = @(marker: Int <= 0)

bad <= WireBox[Async[1]()]()
"#;

    let (ok, output) = run_way_check(source, "wired_constraint_async_reject");
    assert!(!ok, "Async values must not satisfy Wired[T]:\n{}", output);
    assert!(
        output.contains("[E3601]") && output.contains("Async"),
        "diagnostic should name the rejected Async value:\n{}",
        output
    );
}

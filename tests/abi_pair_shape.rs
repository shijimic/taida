mod common;

use std::process::Command;

use common::{taida_bin, unique_temp_dir};

fn run_way_check(source: &str, label: &str) -> (bool, String) {
    let dir = unique_temp_dir(label);
    let td_path = dir.join("main.td");
    std::fs::write(&td_path, source).expect("write ABI shape fixture");

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
fn abi_web_request_response_headers_are_pair_lists() {
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, header)

requestProbe req: WebRequest =
  queryPair <= req.query.get(0).getOrDefault(@(name <= "", value <= ""))
  headerPair <= req.headers.get(0).getOrDefault(@(name <= "", value <= ""))
  req.rawQuery + ":" + queryPair.name + "=" + queryPair.value + ":" + headerPair.name + "=" + headerPair.value
=> :Str

responseProbe response: WebResponse =
  headerPair <= response.headers.get(0).getOrDefault(@(name <= "", value <= ""))
  headerPair.name + "=" + headerPair.value
=> :Str

handle req: WebRequest =
  header("set-cookie", "b=2", header("set-cookie", "a=1", text(requestProbe(req))))
=> :WebResponse
"#;

    let (ok, output) = run_way_check(source, "abi_pair_shape_ok");
    assert!(
        ok,
        "expected pair-list ABI shape to type-check:\n{}",
        output
    );
    assert!(
        output.contains("errors=0") && !output.contains("[ERROR]"),
        "expected clean pair-list ABI check:\n{}",
        output
    );
}

#[test]
fn abi_web_request_query_is_not_legacy_hash_map() {
    let source = r#">>> taida-lang/abi => @(WebRequest)

expectMap query: HashMap[Str, Str] =
  query
=> :HashMap[Str, Str]

probe req: WebRequest =
  expectMap(req.query)
=> :HashMap[Str, Str]
"#;

    let (ok, output) = run_way_check(source, "abi_pair_shape_legacy_map_reject");
    assert!(
        !ok,
        "legacy HashMap query shape should be rejected after ABI pair-list change:\n{}",
        output
    );
    assert!(
        output.contains("HashMap") || output.contains("@[@(name: Str, value: Str)]"),
        "diagnostic should describe the shape mismatch:\n{}",
        output
    );
}

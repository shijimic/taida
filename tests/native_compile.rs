/// Integration tests: compile each compile_*.td to a native binary via Cranelift,
/// execute it, and verify its output matches the interpreter (reference implementation).
///
/// These tests ensure that the Cranelift native backend produces the same results
/// as the interpreter for all compile_* example files.
mod common;

use common::{run_interpreter, taida_bin};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Compile a .td file to a native binary, execute it, and return its stdout.
fn compile_and_run(td_path: &Path) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let binary_path = unique_temp_path(&format!("taida_test_{}", stem), "bin");

    // Compile (no global lock needed -- FL-7 ensures unique .o paths)
    let compile_output = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(td_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .ok()?;

    if !compile_output.status.success() {
        let stderr = String::from_utf8_lossy(&compile_output.stderr);
        eprintln!("Compile failed for {}: {}", td_path.display(), stderr);
        return None;
    }

    // Execute
    let run_output = Command::new(&binary_path).output().ok()?;

    // Clean up
    let _ = fs::remove_file(&binary_path);

    if !run_output.status.success() {
        let stderr = String::from_utf8_lossy(&run_output.stderr);
        eprintln!("Execution failed for {}: {}", td_path.display(), stderr);
        return None;
    }

    Some(
        String::from_utf8_lossy(&run_output.stdout)
            .trim_end()
            .to_string(),
    )
}

/// Normalize output for comparison: strip trailing whitespace per line.
///
/// LIMITATION (AT-1): This hides trailing-space differences between backends.
/// See tests/parity.rs normalize() for full documentation.
fn normalize(s: &str) -> String {
    s.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn unique_temp_path(prefix: &str, ext: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}_{}_{}.{}",
        prefix,
        std::process::id(),
        nanos,
        ext
    ))
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

fn run_interpreter_src(source: &str, label: &str) -> Option<String> {
    let tmp = unique_temp_path(&format!("taida_native_interp_{}", label), "td");
    fs::write(&tmp, source).ok()?;
    let out = run_interpreter(&tmp);
    let _ = fs::remove_file(&tmp);
    out
}

fn compile_and_run_src(source: &str, label: &str) -> Option<String> {
    let tmp = unique_temp_path(&format!("taida_native_src_{}", label), "td");
    fs::write(&tmp, source).ok()?;
    let out = compile_and_run(&tmp);
    let _ = fs::remove_file(&tmp);
    out
}

fn assert_native_matches_interpreter(source: &str, label: &str) {
    let interp = run_interpreter_src(source, label)
        .unwrap_or_else(|| panic!("interpreter failed for {}", label));
    let native = compile_and_run_src(source, label)
        .unwrap_or_else(|| panic!("native compile/run failed for {}", label));
    assert_eq!(native, interp, "native/interpreter mismatch for {}", label);
}

fn assert_native_and_interpreter_reject_source(source: &str, label: &str) {
    assert!(
        run_interpreter_src(source, label).is_none(),
        "interpreter unexpectedly accepted {}",
        label
    );
    assert!(
        compile_and_run_src(source, label).is_none(),
        "native unexpectedly accepted {}",
        label
    );
}

#[test]
fn test_native_write_failure_shape_preserves_error_field_names() {
    // TF-16: Native extracts message field from Error BuchiPack (matching interpreter)
    // Note: exact error message format differs (strerror vs Rust io::Error::Display)
    // so we verify structure, not exact string match.
    let bad_path = unique_temp_dir("taida_native_write_failure_shape")
        .join("missing")
        .join("file.txt");
    let source = format!(
        "\
result <= writeFile(\"{}\", \"data\")
stdout(result.isError().toString())
stdout(result.toString())
",
        bad_path.display()
    );

    let native =
        compile_and_run_src(&source, "write_failure_shape").expect("native backend should succeed");
    let native_lines: Vec<&str> = native.lines().collect();
    assert!(
        native_lines.len() >= 2,
        "native output too short: {:?}",
        native_lines
    );
    assert_eq!(native_lines[0], "true");
    // TF-16: message field value is extracted (not full BuchiPack structure)
    assert!(
        native_lines[1].starts_with("Result(throw <= ")
            && native_lines[1].contains("No such file or directory"),
        "native Result toString should show extracted message, got: {}",
        native_lines[1]
    );
}

fn expected_native_reject_examples() -> Vec<&'static str> {
    vec![
        "compile_stream", // Native backend does not provide Stream[T]
    ]
}

// C24 Phase 5 (RC-SLOW-2 / C24B-006): per-fixture decomposition. The
// original `test_native_compile_parity` iterated over every compile_*.td
// sequentially and was the 4th slowest test in CI (33s warm). Now each
// fixture gets its own `#[test]` that forwards into
// `run_native_compile_parity_fixture`, letting nextest parallelize.

/// Run interp vs native parity for a single compile_* fixture.
fn run_native_compile_parity_fixture(stem: &str) {
    // compile_module requires module imports with relative paths which
    // may not resolve correctly from the temp directory used by this
    // test harness — skipped in the original loop, preserved here.
    if stem == "compile_module" {
        return;
    }

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(format!("{}.td", stem));
    let expected_rejects = expected_native_reject_examples();
    let is_expected_reject = expected_rejects.contains(&stem);

    let interp_output =
        run_interpreter(&path).unwrap_or_else(|| panic!("{}: interpreter failed", stem));

    let native_output = match compile_and_run(&path) {
        Some(o) => o,
        None => {
            if is_expected_reject {
                return; // documented rejection
            }
            panic!("{}: compile/run failed", stem);
        }
    };

    if is_expected_reject {
        panic!(
            "{}: expected to be rejected but compiled and ran — update \
             expected_native_reject_examples",
            stem,
        );
    }

    let interp_norm = normalize(&interp_output);
    let native_norm = normalize(&native_output);

    if interp_norm != native_norm {
        panic!(
            "{}: output mismatch\n  interpreter: {:?}\n  native:      {:?}",
            stem,
            interp_output.lines().take(5).collect::<Vec<_>>(),
            native_output.lines().take(5).collect::<Vec<_>>(),
        );
    }
}

/// Guard: allowlist references valid fixtures.
#[test]
fn test_native_compile_parity_allowlist_guard() {
    use common::fixture_lists::COMPILE_TD_FIXTURES;
    for stem in expected_native_reject_examples() {
        assert!(
            COMPILE_TD_FIXTURES.contains(&stem),
            "expected_native_reject_examples references unknown fixture `{}`",
            stem,
        );
    }
    assert!(!COMPILE_TD_FIXTURES.is_empty(), "No compile_*.td fixtures");
}

// Per-fixture tests emitted by build.rs.
macro_rules! c24_fixture_runner {
    ($stem:expr) => {
        run_native_compile_parity_fixture($stem)
    };
}
include!(concat!(env!("OUT_DIR"), "/examples_compile_td_tests.rs"));

#[test]
fn test_native_stream_example_is_explicitly_rejected() {
    let td_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("compile_stream.td");
    let binary_path = unique_temp_path("taida_native_stream_reject", "bin");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&td_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("build native compile_stream");

    let _ = fs::remove_file(&binary_path);

    assert!(
        !output.status.success(),
        "compile_stream should be rejected by native backend"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported mold type: Stream"),
        "expected explicit Stream rejection, got: {}",
        stderr
    );
}

#[test]
fn test_native_hashmap_tostring_large_int_does_not_crash() {
    let source = r#"
m <= hashMap().set("x", 5000)
stdout(m.toString())
"#;
    let out = compile_and_run_src(source, "hashmap_tostring_5000");
    assert!(
        out.is_some(),
        "native run should not crash for HashMap.toString()"
    );
}

#[test]
fn test_native_custom_mold_solidify_override_matches_interpreter() {
    let source = r#"
Mold[T] => PlusOne[T] = @(
  solidify =
    filling + 1
  => :Int
)
stdout(PlusOne[41]().toString())
"#;
    assert_native_matches_interpreter(source, "native_custom_mold_solidify");
}

#[test]
fn test_native_custom_mold_required_positional_binding_matches_interpreter() {
    let source = r#"
Mold[T] => Pair[T, U] = @(
  second: U
  solidify =
    filling + second
  => :Int
)
stdout(Pair[40, 2]().toString())
"#;
    assert_native_matches_interpreter(source, "native_custom_mold_required_positional");
}

#[test]
fn test_native_inherited_custom_mold_required_positional_binding_matches_interpreter() {
    let source = r#"
Mold[T] => PairBase[T] = @()
PairBase[T] => Pair[T, U] = @(
  second: U
  solidify =
    filling + second
  => :Int
)
stdout(Pair[40, 2]().toString())
"#;
    assert_native_matches_interpreter(source, "native_inherited_custom_mold_required_positional");
}

#[test]
fn test_native_inherited_custom_mold_override_parent_field_matches_interpreter() {
    let source = r#"
Mold[T] => Base[T] = @(
  bonus: Int <= 0
  solidify =
    filling + bonus
  => :Int
)
Base[T] => Child[T] = @(
  bonus: Int
)
stdout(Child[40, 2]().toString())
"#;
    assert_native_matches_interpreter(source, "native_inherited_custom_mold_override_parent_field");
}

#[test]
fn test_native_custom_mold_solidify_throw_caught_matches_interpreter() {
    let source = r#"
Mold[T] => AlwaysFail[T] = @(
  solidify =
    Error(type <= "SolidifyError", message <= "fail").throw()
  => :Int
)
check x: Int =
  |== err: Error =
    -1
  => :Int
  AlwaysFail[x]()
=> :Int
stdout(check(7).toString())
"#;
    assert_native_matches_interpreter(source, "native_custom_mold_solidify_throw_caught");
}

#[test]
fn test_native_custom_mold_default_unmold_matches_interpreter() {
    let source = r#"
Mold[T] => Boxed[T] = @()
b <= Boxed[7]()
b ]=> v
stdout(v.toString())
"#;
    assert_native_matches_interpreter(source, "native_custom_mold_default_unmold");
}

#[test]
fn test_native_custom_mold_definition_errors_are_rejected() {
    let cases = [
        (
            "native_custom_mold_def_missing_type_or_default",
            r#"
Mold[T] => BadField[T] = @(
  raw
)
"#,
        ),
        (
            "native_custom_mold_def_unbound_type_param",
            r#"
Mold[T] => BadBind[T, U] = @()
"#,
        ),
    ];

    for (label, source) in cases {
        assert_native_and_interpreter_reject_source(source, label);
    }
}

#[test]
fn test_native_function_default_args_matches_interpreter() {
    let source = r#"
sum3 a: Int b: Int <= 10 c: Int <= a + b =
  a + b + c
=> :Int

stdout(sum3().toString())
stdout(sum3(1).toString())
stdout(sum3(1, 2).toString())
"#;
    assert_native_matches_interpreter(source, "native_function_default_args");
}

#[test]
fn test_native_function_too_many_args_are_rejected() {
    let source = r#"
id x: Int =
  x
=> :Int

stdout(id(1, 2).toString())
"#;
    assert_native_and_interpreter_reject_source(source, "native_function_too_many_args");
}

#[test]
fn test_native_int_mold_from_function_param_matches_interpreter() {
    let source = r#"
parseId arg =
  Int[arg]().getOrDefault(-1)
=> :Int

x <= parseId("1")
stdout(x.toString())
stdout((x + 1).toString())
"#;
    assert_native_matches_interpreter(source, "native_int_mold_from_function_param");
}

#[test]
fn test_native_todo_stub_unmold_matches_interpreter() {
    let source = r#"
a <= TODO[Int](id <= "TASK-1", task <= "use unm", sol <= 7, unm <= 9)
a ]=> av
stdout(av.toString())

b <= TODO[Int](sol <= 5)
b ]=> bv
stdout(bv.toString())

c <= TODO[Stub["User data TBD"]]()
c ]=> cv
g <= Cage[cv, _ m = 1]()
g ]=> gv
stdout(gv.toString())
"#;
    assert_native_matches_interpreter(source, "native_todo_stub_unmold");
}

#[test]
fn test_native_flatten_large_int_matches_interpreter() {
    let source = r#"
flat <= Flatten[@[1234567890123]]()
stdout(flat)
"#;
    assert_native_matches_interpreter(source, "flatten_large_int");
}

#[test]
fn test_native_unmold_large_int_matches_interpreter() {
    let source = r#"
x <= 1234567890123
x ]=> y
stdout(y.toString())
"#;
    assert_native_matches_interpreter(source, "unmold_large_int");
}

#[test]
fn test_native_async_all_large_int_matches_interpreter() {
    let source = r#"
all_res <= All[@[1234567890123]]()
stdout(all_res.toString())
"#;
    assert_native_matches_interpreter(source, "async_all_large_int");
}

#[test]
fn test_native_async_race_large_int_matches_interpreter() {
    let source = r#"
race_res <= Race[@[1234567890123]]()
stdout(race_res.toString())
"#;
    assert_native_matches_interpreter(source, "async_race_large_int");
}

#[test]
fn test_native_hashmap_variable_non_string_key_matches_interpreter() {
    let source = r#"
k <= 1
m <= hashMap().set(k, 2)
stdout(m.get(1).hasValue().toString())
stdout(m.get(1).getOrDefault(99).toString())
"#;
    assert_native_matches_interpreter(source, "hashmap_variable_non_string_key");
}

#[test]
fn test_native_flatten_empty_matches_interpreter() {
    let source = r#"
flat <= Flatten[@[]]()
stdout(flat)
"#;
    assert_native_matches_interpreter(source, "flatten_empty");
}

#[test]
fn test_native_flatten_nested_lists_matches_interpreter() {
    let source = r#"
flat <= Flatten[@[@[1], @[2], @[3], @[4, 5]]]()
stdout(flat)
"#;
    assert_native_matches_interpreter(source, "flatten_nested_lists");
}

#[test]
fn test_native_unmold_negative_boundary_matches_interpreter() {
    let source = r#"
x <= -2147483648
x ]=> y
stdout(y.toString())
"#;
    assert_native_matches_interpreter(source, "unmold_negative_boundary");
}

#[test]
fn test_native_hashmap_many_entries_matches_interpreter() {
    let source = r#"
m <= hashMap()
m1 <= m.set("k1", 1)
m2 <= m1.set("k2", 2)
m3 <= m2.set("k3", 3)
m4 <= m3.set("k4", 4)
m5 <= m4.set("k5", 5)
m6 <= m5.set("k6", 6)
m7 <= m6.set("k7", 7)
m8 <= m7.set("k8", 8)
m9 <= m8.set("k9", 9)
m10 <= m9.set("k10", 10)
m11 <= m10.set("k11", 11)
m12 <= m11.set("k12", 12)
m13 <= m12.set("k13", 13)
m14 <= m13.set("k14", 14)
m15 <= m14.set("k15", 15)
m16 <= m15.set("k16", 16)
m17 <= m16.set("k17", 17)
m18 <= m17.set("k18", 18)
m19 <= m18.set("k19", 19)
m20 <= m19.set("k20", 20)
m21 <= m20.set("k21", 21)
m22 <= m21.set("k22", 22)
m23 <= m22.set("k23", 23)
m24 <= m23.set("k24", 24)
m25 <= m24.set("k25", 25)
m26 <= m25.set("k26", 26)
m27 <= m26.set("k27", 27)
m28 <= m27.set("k28", 28)
m29 <= m28.set("k29", 29)
m30 <= m29.set("k30", 30)
stdout(m30.get("k1").getOrDefault(0).toString())
stdout(m30.get("k15").getOrDefault(0).toString())
stdout(m30.get("k30").getOrDefault(0).toString())
"#;
    assert_native_matches_interpreter(source, "hashmap_many_entries");
}

#[test]
fn test_native_hashmap_long_key_matches_interpreter() {
    let source = r#"
m <= hashMap().set("kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk", 77)
stdout(m.get("kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").getOrDefault(0).toString())
"#;
    assert_native_matches_interpreter(source, "hashmap_long_key");
}

#[test]
fn test_native_async_all_mixed_sync_async_matches_interpreter() {
    let source = r#"
a <= All[@[1, Async[2](), 3, Async[4]()]]()
a ]=> r
stdout(r)
"#;
    assert_native_matches_interpreter(source, "async_all_mixed_sync_async");
}

#[test]
fn test_native_sleep_boundary_errors_match_interpreter() {
    let source = r#"
ok <= sleep(0)
stdout(ok.isRejected().toString())
neg <= sleep(-1)
stdout(neg.isRejected().toString())
big <= sleep(2147483648)
stdout(big.isRejected().toString())
"#;
    assert_native_matches_interpreter(source, "sleep_boundary_errors");
}

#[test]
fn test_native_nowms_sleep_shape() {
    let source = r#"
a <= nowMs()
s <= sleep(20)
s ]=> waited
b <= nowMs()
stdout(a.toString())
stdout(b.toString())
stdout((b >= a).toString())
"#;

    let out = compile_and_run_src(source, "nowms_sleep_shape")
        .expect("native compile/run should succeed for nowMs/sleep");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "unexpected output shape: {:?}", lines);
    let a: i64 = lines[0]
        .parse()
        .unwrap_or_else(|_| panic!("a is not Int: {:?}", lines[0]));
    let b: i64 = lines[1]
        .parse()
        .unwrap_or_else(|_| panic!("b is not Int: {:?}", lines[1]));
    assert_eq!(lines[2], "true", "expected monotonic flag true");
    assert!(b >= a, "expected nowMs monotonic: {} -> {}", a, b);
}

#[test]
fn test_native_hof_capture_callbacks_match_interpreter() {
    let source = r#"
target <= 2
nums <= @[1, 2, 3, 2]
mapped <= Map[nums, _ x = x + target]()
filtered <= Filter[mapped, _ x = x >= target]()
sum <= Fold[filtered, 0, _ acc item = acc + item + target]()

stdout(mapped)
stdout(filtered)
stdout(sum.toString())
"#;
    assert_native_matches_interpreter(source, "native_hof_capture_callbacks");
}

#[test]
fn test_native_result_predicate_capture_matches_interpreter() {
    let source = r#"
limit <= 18

ok <= Result[21, _ x = x >= limit]()
stdout(ok.isSuccess().toString())
ok ]=> v
stdout(v.toString())

ng <= Result[15, _ x = x >= limit]()
stdout(ng.isSuccess().toString())
stdout(ng.getOrDefault(999).toString())
"#;
    assert_native_matches_interpreter(source, "native_result_predicate_capture");
}

#[test]
fn test_native_string_contains_field_access_in_lambda_matches_interpreter() {
    let source = r#"
Todo = @(id: Int, title: Str, done: Bool)
items <= @[
  @(id <= 1, title <= "renamed task", done <= false)
]
matched <= Filter[items, _ x = x.title.contains("renamed")]()
stdout(matched.length().toString())
"#;
    assert_native_matches_interpreter(source, "native_contains_field_access_lambda");
}

#[test]
fn test_native_ptr_heuristic_exploit_edge_cases() {
    // These values can resemble mapped addresses and should still behave as scalars.
    let cases = [
        ("small_ptr", "x <= 4097\nstdout(x.toString())"),
        ("high_int", "x <= 1234567890123\nstdout(x.toString())"),
        (
            "list_lookalike",
            "x <= 65537\nout <= Flatten[@[x]]()\nstdout(out)",
        ),
    ];

    for (label, source) in cases {
        assert_native_matches_interpreter(source, label);
    }
}

/// F-47 regression: lambda capturing outer variables into BuchiPack/TypeInst/ListLit/MoldInst
/// was broken because collect_free_vars_inner did not traverse these Expr variants.
#[test]
fn test_native_f47_lambda_capture_into_buchi_pack() {
    let cases = [
        (
            "capture_into_buchi_pack",
            r#"Item = @(x: Int)
test1 a =
  items <= @[1]
  mapper <= _ item = @(x <= a)
  Map[items, mapper]() => result
  stdout(jsonPretty(result))
  0
=> :Int
test1(42)"#,
        ),
        (
            "capture_into_type_inst",
            r#"Item = @(id: Int, name: Str)
test1 a b =
  items <= @[1]
  mapper <= _ item = @(id <= a, name <= b)
  Map[items, mapper]() => result
  stdout(jsonPretty(result))
  0
=> :Int
test1(42, "hello")"#,
        ),
        (
            "capture_3_args_into_pack",
            r#"Todo = @(id: Int, title: Str, done: Bool)
doUpdate reqId reqTitle reqDone =
  items <= @[@(id <= 1, title <= "original", done <= false)]
  mapper <= _ item = | item.id == reqId |> @(id <= reqId, title <= reqTitle, done <= reqDone) | _ |> item
  newItems <= Map[items, mapper]()
  found <= Find[newItems, _ item = item.id == reqId]()
  jsonPretty(found.__value)
=> :Str
main dummy =
  result <= doUpdate(1, "updated", true)
  stdout(result)
  0
=> :Int
main(0)"#,
        ),
        (
            "capture_into_list_lit",
            r#"test1 a b =
  items <= @[1]
  mapper <= _ item = @[a, b, item]
  Map[items, mapper]() => result
  stdout(jsonPretty(result))
  0
=> :Int
test1(10, 20)"#,
        ),
        (
            "capture_in_nested_lambda",
            r#"test1 x =
  items <= @[1, 2, 3]
  mapper <= _ item = Map[@[item], _ inner = inner + x]()
  Map[items, mapper]() => result
  stdout(jsonPretty(Flatten[result]()))
  0
=> :Int
test1(100)"#,
        ),
    ];

    for (label, source) in cases {
        assert_native_matches_interpreter(source, label);
    }
}

/// F-48: Nested BuchiPack inside list returned from function must not be corrupted.
/// Previously, the inner BuchiPack (`item`) was freed at function exit even though
/// it was still referenced from the returned list via Append.
#[test]
fn test_f48_nested_buchi_pack_in_returned_list() {
    let source = r#"Todo = @(id: Int, title: Str, done: Bool, status: Str)
Store = @(next_id: Int, items: @[Todo])

addItem title store =
  item <= @(id <= store.next_id, title <= title, done <= false, status <= "todo")
  updated <= Append[store.items, item]()
  @(next_id <= store.next_id + 1, items <= updated)
=> :Store

main dummy =
  store <= @(next_id <= 1, items <= @[])
  result <= addItem("test1", store)
  stdout(jsonEncode(result))
  0
=> :Int

main(0)"#;
    assert_native_matches_interpreter(source, "f48_nested_pack_in_list");
}

/// F-48 variant: multiple items added to list in sequence
#[test]
fn test_f48_multiple_nested_packs_in_returned_list() {
    let source = r#"Item = @(id: Int, name: Str)
Container = @(items: @[Item])

addTwo dummy =
  items: @[Item] <= @[]
  a <= @(id <= 1, name <= "first")
  items2 <= Append[items, a]()
  b <= @(id <= 2, name <= "second")
  items3 <= Append[items2, b]()
  @(items <= items3)
=> :Container

main dummy =
  result <= addTwo(0)
  stdout(jsonEncode(result))
  0
=> :Int

main(0)"#;
    assert_native_matches_interpreter(source, "f48_multiple_nested_packs");
}

/// F-48 followup: same field name with different types across BuchiPack definitions
/// (e.g., `Todo.status: Str` vs `HttpResp.status: Int`) must serialize correctly.
/// The global field type registry must not let one type shadow the other.
#[test]
fn test_f48_field_name_type_conflict_json_serialize() {
    let source = r#"Todo = @(id: Int, title: Str, done: Bool, status: Str)
HttpResp = @(status: Int, body: Str)

addIssue title =
  item <= @(id <= 1, title <= title, done <= false, status <= "todo")
  resp <= @(status <= 201, body <= "created")
  stdout(jsonEncode(item))
  stdout(jsonEncode(resp))
  0
=> :Int

main dummy =
  addIssue("test1")
=> :Int

main(0)"#;
    assert_native_matches_interpreter(source, "f48_field_type_conflict");
}

/// F-48 followup: jsonPretty with conflicting field name types
#[test]
fn test_f48_field_name_type_conflict_json_pretty() {
    let source = r#"Todo = @(id: Int, title: Str, done: Bool, status: Str)
HttpResp = @(status: Int, body: Str)

main dummy =
  todo <= @(id <= 1, title <= "hello", done <= false, status <= "in_progress")
  resp <= @(status <= 200, body <= jsonPretty(todo))
  stdout(jsonPretty(resp))
  0
=> :Int

main(0)"#;
    assert_native_matches_interpreter(source, "f48_field_type_conflict_pretty");
}

/// F-52 regression: jsonEncode via module-imported closure returns {}
/// When a library module exports a function that calls jsonEncode on a BuchiPack,
/// the field names must be registered at runtime so taida_json_encode can look them up.
#[test]
fn test_f52_json_encode_via_module_closure() {
    // Create a temp directory for multi-module test
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("taida_f52_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");

    // handlers.td — library module that exports a function using jsonEncode
    let handlers_src = r#"<<< @(handleRoot)

handleRoot dummy =
  jsonEncode(@(ok <= true, service <= "taida.dev"))
=> :Str
"#;

    // router.td — library module that re-exports via import chain
    let router_src = r#">>> ./handlers.td => @(handleRoot)
<<< @(route)

route path =
  handleRoot(0)
=> :Str
"#;

    // main.td — main module that imports and calls
    let main_src = r#">>> ./router.td => @(route)

result <= route("/")
stdout(result)
"#;

    fs::write(dir.join("handlers.td"), handlers_src).expect("write handlers.td");
    fs::write(dir.join("router.td"), router_src).expect("write router.td");
    fs::write(dir.join("main.td"), main_src).expect("write main.td");

    let main_path = dir.join("main.td");

    // Run interpreter
    let interp_output = Command::new(taida_bin())
        .arg(&main_path)
        .output()
        .expect("interpreter run");
    assert!(
        interp_output.status.success(),
        "interpreter failed: {}",
        String::from_utf8_lossy(&interp_output.stderr)
    );
    let interp_result = normalize(String::from_utf8_lossy(&interp_output.stdout).trim_end());

    // Compile and run native
    let binary_path = dir.join("main_native");
    let compile_output = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&main_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("native compile");
    assert!(
        compile_output.status.success(),
        "native compile failed: {}",
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let run_output = Command::new(&binary_path).output().expect("native run");
    assert!(
        run_output.status.success(),
        "native execution failed (exit={}): {}",
        run_output.status,
        String::from_utf8_lossy(&run_output.stderr)
    );
    let native_result = normalize(String::from_utf8_lossy(&run_output.stdout).trim_end());

    // Cleanup
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(
        native_result, interp_result,
        "F-52: native jsonEncode via module closure returned '{}', expected '{}'",
        native_result, interp_result
    );

    // Verify the output contains expected JSON
    assert!(
        native_result.contains("\"ok\"") && native_result.contains("\"service\""),
        "F-52: output should contain ok and service fields, got: {}",
        native_result
    );
}

/// F-57 regression: BuchiPack field function call
/// obj.field(args) where field is a lambda stored in BuchiPack
/// Previously, the native backend returned "unsupported method" for unknown method names.
#[test]
fn test_f57_native_pack_field_call() {
    let cases = [
        (
            "pack_field_call_0_arity",
            r#"actions <= @(greet <= _ = "hi")
stdout(actions.greet())"#,
        ),
        (
            "pack_field_call_1_arg_str",
            r#"echo <= @(say <= _ msg = msg)
stdout(echo.say("hello from field"))"#,
        ),
        (
            "pack_field_call_1_arg_int",
            r#"doubler <= @(run <= _ x = x * 2)
stdout(doubler.run(21).toString())"#,
        ),
        (
            "pack_field_call_2_arg_int",
            r#"calc <= @(plus <= _ a b = a + b)
stdout(calc.plus(3, 7).toString())"#,
        ),
        (
            "pack_field_call_closure",
            r#"tag <= "PREFIX"
box <= @(wrap <= _ x = tag + x)
stdout(box.wrap(":item"))"#,
        ),
        (
            "pack_field_call_kv_pattern",
            r#"kv <= @(
  fetch <= _ key = "value"
  store <= _ key value = "ok"
)
stdout(kv.fetch("test"))
stdout(kv.store("test", "val"))"#,
        ),
    ];

    for (label, source) in cases {
        assert_native_matches_interpreter(source, label);
    }
}

/// F-55 regression: Native string equality comparison
/// String == must use strcmp, not pointer comparison
#[test]
fn test_f55_native_string_equality() {
    let src = r#"
// Variable == literal
x <= "GET"
stdout(x == "GET")
stdout(x != "POST")

// Function param == literal
check a = a == "hello" => :Bool
stdout(check("hello"))
stdout(check("world"))

// Function param == param (polymorphic)
eq a b = a == b => :Bool
stdout(eq("foo", "foo"))
stdout(eq("foo", "bar"))

// Pattern match with string params
route method path =
  | method == "GET" && path == "/" |> "root"
  | _ |> "other"
=> :Str
stdout(route("GET", "/"))
stdout(route("POST", "/"))

// Int equality still works
stdout(42 == 42)
stdout(42 == 0)
"#;
    let dir = std::env::temp_dir().join(format!(
        "taida_f55_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let td = dir.join("test.td");
    fs::write(&td, src).unwrap();

    let interp = run_interpreter(&td).expect("interpreter should succeed");
    let native = compile_and_run(&td).expect("native should succeed");
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(
        normalize(&native),
        normalize(&interp),
        "F-55: string equality mismatch\ninterp: {}\nnative: {}",
        interp,
        native
    );
}

#[test]
fn test_qf18_function_value_call_matches_interpreter() {
    let source = r#"
double x = x * 2 => :Int
fnRef <= double
stdout(fnRef(21).toString())
"#;
    assert_native_matches_interpreter(source, "qf18_function_value_call");
}

#[test]
fn test_qf19_imported_function_sees_private_sibling_value() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("taida_qf19_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("helper.td"),
        r#"base <= 3
add x =
  x + base
=> :Int
<<< @(add)
"#,
    )
    .expect("write helper");
    fs::write(
        dir.join("main.td"),
        r#">>> ./helper.td => @(add)

stdout(add(4).toString())
stdout(add(10).toString())
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("interpreter should succeed");
    let native = compile_and_run(&main_path).expect("native should succeed");
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(normalize(&native), normalize(&interp));
}

#[test]
fn test_qf20_unmold_statement_preserves_global_capture() {
    let source = r#"
items <= @[10, 20, 30]

sumItems =
  Fold[items, 0, _ acc x = acc + x]() ]=> total
  total

stdout(sumItems().toString())
"#;
    assert_native_matches_interpreter(source, "qf20_unmold_global_capture");
}

#[test]
fn test_qf26_imported_value_visible_in_main_function() {
    let dir = unique_temp_dir("taida_qf26");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("helper.td"),
        r#"value <= 41
<<< @(value)
"#,
    )
    .expect("write helper");
    fs::write(
        dir.join("main.td"),
        r#">>> ./helper.td => @(value)

get dummy = value => :Int
stdout(get(0).toString())
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("interpreter should succeed");
    let native = compile_and_run(&main_path).expect("native should succeed");
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(normalize(&native), normalize(&interp));
}

#[test]
fn test_qf27_library_init_resolves_imported_values_and_dependency_init() {
    let dir = unique_temp_dir("taida_qf27");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("mod_b.td"),
        r#"seed <= 41
make dummy = seed + 1 => :Int
<<< @(make)
"#,
    )
    .expect("write mod_b");
    fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b.td => @(make)

answer <= make(0)
get dummy = answer => :Int
<<< @(get)
"#,
    )
    .expect("write mod_a");
    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a.td => @(get)
stdout(get(0).toString())
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("interpreter should succeed");
    let native = compile_and_run(&main_path).expect("native should succeed");
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(normalize(&native), normalize(&interp));
}

#[test]
fn test_qf28_duplicate_export_names_across_modules_do_not_collide() {
    let dir = unique_temp_dir("taida_qf28");
    fs::create_dir_all(dir.join("a")).expect("create a");
    fs::create_dir_all(dir.join("b")).expect("create b");

    fs::write(
        dir.join("a").join("foo.td"),
        r#"value <= "A"
get dummy = value => :Str
<<< @(get)
"#,
    )
    .expect("write a/foo");
    fs::write(
        dir.join("b").join("bar.td"),
        r#"value <= "B"
get dummy = value => :Str
<<< @(get)
"#,
    )
    .expect("write b/bar");
    fs::write(
        dir.join("main.td"),
        r#">>> ./a/foo.td => @(get => getA)
>>> ./b/bar.td => @(get => getB)
stdout(getA(0))
stdout(getB(0))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("interpreter should succeed");
    let native = compile_and_run(&main_path).expect("native should succeed");
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(normalize(&native), normalize(&interp));
}

#[test]
fn test_qf29_same_stem_modules_do_not_collide() {
    let dir = unique_temp_dir("taida_qf29");
    fs::create_dir_all(dir.join("a")).expect("create a");
    fs::create_dir_all(dir.join("b")).expect("create b");

    fs::write(
        dir.join("a").join("util.td"),
        r#"value <= "A"
getA dummy = value => :Str
<<< @(getA)
"#,
    )
    .expect("write a/util");
    fs::write(
        dir.join("b").join("util.td"),
        r#"value <= "B"
getB dummy = value => :Str
<<< @(getB)
"#,
    )
    .expect("write b/util");
    fs::write(
        dir.join("main.td"),
        r#">>> ./a/util.td => @(getA)
>>> ./b/util.td => @(getB)
stdout(getA(0))
stdout(getB(0))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("interpreter should succeed");
    let native = compile_and_run(&main_path).expect("native should succeed");
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(normalize(&native), normalize(&interp));
}

#[test]
fn test_qf31_deep_nested_capture_includes_outer_assignments() {
    let source = r#"f1 a =
  x <= 100
  f2 b =
    y <= 200
    f3 c =
      x.toString() + ":" + y.toString() + ":" + a.toString() + ":" + b.toString() + ":" + c.toString()
    => :Str
    f3(3)
  => :Str
  f2(2)
=> :Str

stdout(f1(1))
"#;
    assert_native_matches_interpreter(source, "qf31_outer_assignment_capture_deep");
}

// ── A-4g: Regression tests for nested BuchiPack/List type tag layout ──

#[test]
fn test_native_a4g_nested_buchi_pack_create_and_access() {
    // A-4g: Nested BuchiPack creation, access, and correct release
    let source = r#"
inner <= @(x <= 10, y <= 20)
outer <= @(name <= "test", data <= inner)
stdout(outer.data.x.toString())
stdout(outer.data.y.toString())
stdout(outer.name)
"#;
    assert_native_matches_interpreter(source, "a4g_nested_pack");
}

#[test]
fn test_native_a4g_list_of_packs() {
    // A-4g: List containing BuchiPacks
    let source = r#"
a <= @(v <= 1)
b <= @(v <= 2)
c <= @(v <= 3)
items <= @[a, b, c]
items.get(0) ]=> first
stdout(first.v.toString())
items.get(2) ]=> last
stdout(last.v.toString())
"#;
    assert_native_matches_interpreter(source, "a4g_list_of_packs");
}

#[test]
fn test_native_a4g_pack_with_list_field() {
    // A-4g: BuchiPack containing a List field
    let source = r#"
data <= @(name <= "test", scores <= @[10, 20, 30])
stdout(data.name)
Sum[data.scores]() ]=> total
stdout(total.toString())
"#;
    assert_native_matches_interpreter(source, "a4g_pack_with_list");
}

#[test]
fn test_native_a4g_typedef_nested_release() {
    // A-4g: TypeDef with nested types, testing correct release
    let source = r#"
inner <= @(a <= 10, b <= @(c <= 20))
stdout(inner.a.toString())
stdout(inner.b.c.toString())
outer <= @(x <= inner, y <= "hello")
stdout(outer.x.a.toString())
stdout(outer.y)
"#;
    assert_native_matches_interpreter(source, "a4g_typedef_nested");
}

#[test]
fn test_native_a4h_memory_leak_loop() {
    // A-4h: Create and discard BuchiPacks in a loop equivalent.
    // Verify no crash (double-free/segfault).
    let source = r#"
a <= @(x <= 1, y <= "hello")
b <= @(x <= 2, y <= "world")
c <= @(x <= 3, y <= "taida")
d <= @(p <= a, q <= b, r <= c)
stdout(d.p.x.toString())
stdout(d.q.y)
stdout(d.r.y)
"#;
    assert_native_matches_interpreter(source, "a4h_memory_loop");
}

// ── retain-on-store: nested Pack/List/Closure recursive release tests ──

#[test]
fn test_retain_on_store_nested_pack_create_access() {
    // Nested BuchiPack creation, field access, and correct release.
    // The inner pack should survive being accessed through the outer pack
    // and be freed exactly once when the outer pack is released.
    let source = r#"
inner <= @(x <= 42, y <= "nested")
outer <= @(child <= inner, name <= "parent")
stdout(outer.child.x.toString())
stdout(outer.child.y)
stdout(outer.name)
"#;
    assert_native_matches_interpreter(source, "retain_nested_pack_basic");
}

#[test]
fn test_retain_on_store_deeply_nested_packs() {
    // 3 levels of nesting: pack -> pack -> pack
    let source = r#"
level3 <= @(val <= "deep")
level2 <= @(inner <= level3, mid <= 2)
level1 <= @(child <= level2, top <= 1)
stdout(level1.child.inner.val)
stdout(level1.child.mid.toString())
stdout(level1.top.toString())
"#;
    assert_native_matches_interpreter(source, "retain_deeply_nested_packs");
}

#[test]
fn test_retain_on_store_shared_nested_pack() {
    // A pack is shared between two outer packs.
    // Both outer packs reference the same inner pack.
    // Correct retain/release ensures no double-free.
    let source = r#"
shared <= @(val <= 99)
a <= @(ref <= shared, label <= "a")
b <= @(ref <= shared, label <= "b")
stdout(a.ref.val.toString())
stdout(b.ref.val.toString())
stdout(a.label)
stdout(b.label)
"#;
    assert_native_matches_interpreter(source, "retain_shared_nested_pack");
}

#[test]
fn test_retain_on_store_nested_pack_loop() {
    // Create and discard nested packs in a loop-like pattern.
    // Tests that retain/release is balanced (no leaks, no double-free).
    let source = r#"
count <= 0
a <= @(inner <= @(v <= 1))
b <= @(inner <= @(v <= 2))
c <= @(inner <= @(v <= 3))
d <= @(inner <= @(v <= 4))
e <= @(inner <= @(v <= 5))
stdout(a.inner.v.toString())
stdout(b.inner.v.toString())
stdout(c.inner.v.toString())
stdout(d.inner.v.toString())
stdout(e.inner.v.toString())
"#;
    assert_native_matches_interpreter(source, "retain_nested_pack_loop");
}

#[test]
fn test_retain_on_store_list_in_pack() {
    // A pack containing a list field.
    // The list should be accessible through the pack.
    let source = r#"
items <= @[10, 20, 30]
container <= @(data <= items, label <= "box")
stdout(container.data.length().toString())
stdout(container.label)
"#;
    assert_native_matches_interpreter(source, "retain_list_in_pack");
}

#[test]
fn test_retain_on_store_typedef_nested_pack() {
    // TypeDef with a nested pack field.
    let source = r#"
Point = @(x: Int, y: Int)
Line = @(start: Point, end: Point)
p1 <= Point(x <= 1, y <= 2)
p2 <= Point(x <= 3, y <= 4)
line <= Line(start <= p1, end <= p2)
stdout(line.start.x.toString())
stdout(line.end.y.toString())
"#;
    assert_native_matches_interpreter(source, "retain_typedef_nested_pack");
}

#[test]
fn test_retain_on_store_list_var_in_pack() {
    // Finding 1: A pack containing a list variable (not literal).
    // expr_type_tag must recognize list_vars for correct tag/retain.
    let source = r#"
items <= @[1, 2, 3]
container <= @(data <= items, count <= items.length())
stdout(container.data.length().toString())
stdout(container.count.toString())
"#;
    assert_native_matches_interpreter(source, "retain_list_var_in_pack");
}

#[test]
fn test_retain_on_store_func_returning_list_in_pack() {
    // Finding 1: A pack containing a function call that returns a list.
    // expr_type_tag FuncCall branch must detect list_returning_funcs.
    let source = r#"
makeList n = @[n, n * 2, n * 3] => :@[Int]
wrapper <= @(nums <= makeList(10), label <= "test")
stdout(wrapper.nums.length().toString())
stdout(wrapper.label)
"#;
    assert_native_matches_interpreter(source, "retain_func_list_in_pack");
}

// Note: range() segfaults in native (pre-existing issue, not retain-on-store related).
// Range test omitted until range() native support is fixed.

#[test]
fn test_a4_str_retain_on_store_in_list_pack_lax() {
    // A-4: 動的生成した文字列を List/Pack に入れて、寿命をまたいでも crash しない
    let source = r#"
name <= "hello" + " world"
items <= @[name, name]
pack <= @(val <= name)
items.first() ]=> f
stdout(f)
stdout(pack.val)
"#;
    assert_native_matches_interpreter(source, "a4_str_retain_list_pack");
}

#[test]
fn test_a4_zip_enumerate_str_elements() {
    // A-4: Zip/Enumerate で Str 要素を運ぶ pair pack の tag/retain
    let source = r#"
names <= @["alice", "bob"]
ages <= @[30, 25]
Zip[names, ages]() ]=> pairs
stdout(pairs.length().toString())

Enumerate[names]() ]=> indexed
stdout(indexed.length().toString())
"#;
    assert_native_matches_interpreter(source, "a4_zip_enumerate_str");
}

#[test]
fn test_a4_runtime_built_pack_heap_string() {
    // A-4: Str 型変換で heap string を生成 → Lax に入る → unmold で取り出す
    let source = r#"
Str[42]() ]=> s
stdout(s)
"#;
    assert_native_matches_interpreter(source, "a4_runtime_lax_heap_str");
}

// ── NO-1: HashMap ownership model ────────────────────────

#[test]
fn test_no1_hashmap_string_values_ownership() {
    // NO-1: HashMap with String values — retain/release on set/clone/drop
    let source = r#"
m <= hashMap().set("a", "hello").set("b", "world")
stdout(m.get("a").getOrDefault("").toString())
stdout(m.get("b").getOrDefault("").toString())
m2 <= m.set("c", "foo")
stdout(m2.get("a").getOrDefault("").toString())
stdout(m2.get("c").getOrDefault("").toString())
stdout(m.get("a").getOrDefault("").toString())
"#;
    assert_native_matches_interpreter(source, "no1_hashmap_str_values");
}

#[test]
fn test_no1_hashmap_keys_values_entries_ownership() {
    // NO-1: keys/values/entries produce owned lists
    let source = r#"
m <= hashMap().set("x", "alpha").set("y", "beta")
ks <= m.keys()
vs <= m.values()
es <= m.entries()
stdout(ks.length().toString())
stdout(vs.length().toString())
stdout(es.length().toString())
"#;
    assert_native_matches_interpreter(source, "no1_hashmap_derived_containers");
}

#[test]
fn test_no1_hashmap_remove_ownership() {
    // NO-1: remove releases key/value
    let source = r#"
m <= hashMap().set("a", "one").set("b", "two").set("c", "three")
m2 <= m.remove("b")
stdout(m2.get("a").getOrDefault("").toString())
stdout(m2.get("b").hasValue().toString())
stdout(m2.get("c").getOrDefault("").toString())
"#;
    assert_native_matches_interpreter(source, "no1_hashmap_remove");
}

#[test]
fn test_no1_hashmap_merge_ownership() {
    // NO-1: merge creates a new map with retained entries from both
    let source = r#"
m1 <= hashMap().set("a", "1").set("b", "2")
m2 <= hashMap().set("b", "X").set("c", "3")
m3 <= m1.merge(m2)
stdout(m3.get("a").getOrDefault("").toString())
stdout(m3.get("b").getOrDefault("").toString())
stdout(m3.get("c").getOrDefault("").toString())
"#;
    assert_native_matches_interpreter(source, "no1_hashmap_merge");
}

#[test]
fn test_no1_hashmap_overwrite_releases_old_value() {
    // NO-1: overwriting a key releases the old value and retains the new one
    let source = r#"
m <= hashMap().set("key", "first")
m2 <= m.set("key", "second")
stdout(m.get("key").getOrDefault("").toString())
stdout(m2.get("key").getOrDefault("").toString())
"#;
    assert_native_matches_interpreter(source, "no1_hashmap_overwrite");
}

#[test]
fn test_no1_hashmap_pack_values_ownership() {
    // NO-1: HashMap with Pack values — recursive ownership
    let source = r#"
m <= hashMap().set("p1", @(name <= "Alice", age <= 30)).set("p2", @(name <= "Bob", age <= 25))
stdout(m.get("p1").getOrDefault(@(name <= "", age <= 0)).name)
stdout(m.get("p2").getOrDefault(@(name <= "", age <= 0)).name)
m2 <= m.set("p3", @(name <= "Charlie", age <= 35))
stdout(m2.get("p3").getOrDefault(@(name <= "", age <= 0)).name)
stdout(m2.size().toString())
"#;
    assert_native_matches_interpreter(source, "no1_hashmap_pack_values");
}

#[test]
fn test_no1_hashmap_int_values_no_crash() {
    // NO-1: HashMap with Int values (non-heap) — should not crash
    let source = r#"
m <= hashMap().set("a", 1).set("b", 2).set("c", 3)
stdout(m.get("a").getOrDefault(0).toString())
stdout(m.get("b").getOrDefault(0).toString())
m2 <= m.remove("b")
stdout(m2.size().toString())
stdout(m.size().toString())
"#;
    assert_native_matches_interpreter(source, "no1_hashmap_int_values");
}

// ── NO-2: Set ownership model ────────────────────────────

#[test]
fn test_no2_set_int_elements_no_crash() {
    // NO-2: Set with Int elements (non-heap) — should not crash
    let source = r#"
s <= setOf(@[1, 2, 3])
stdout(s.size().toString())
s2 <= s.add(4)
stdout(s2.size().toString())
s3 <= s2.remove(2)
stdout(s3.size().toString())
stdout(s.has(1).toString())
"#;
    assert_native_matches_interpreter(source, "no2_set_int_elements");
}

#[test]
fn test_no2_set_union_intersect_diff() {
    // NO-2: Set union/intersect/diff with Int elements
    let source = r#"
a <= setOf(@[1, 2, 3])
b <= setOf(@[2, 3, 4])
u <= a.union(b)
stdout(u.size().toString())
i <= a.intersect(b)
stdout(i.size().toString())
d <= a.diff(b)
stdout(d.size().toString())
"#;
    assert_native_matches_interpreter(source, "no2_set_union_intersect_diff");
}

#[test]
fn test_no2_set_to_list_ownership() {
    // NO-2: toList produces an owned list with retained elements
    let source = r#"
s <= setOf(@[10, 20, 30])
lst <= s.toList()
stdout(lst.length().toString())
stdout(s.size().toString())
"#;
    assert_native_matches_interpreter(source, "no2_set_to_list");
}

#[test]
fn test_no2_set_add_remove_chain() {
    // NO-2: Chained add/remove operations — ensures elem_type_tag propagation
    let source = r#"
s <= setOf(@[1, 2, 3])
s2 <= s.add(4).add(5).remove(1)
stdout(s2.size().toString())
stdout(s2.has(4).toString())
stdout(s2.has(1).toString())
stdout(s.size().toString())
"#;
    assert_native_matches_interpreter(source, "no2_set_add_remove_chain");
}

#[test]
fn test_no2_set_empty_operations() {
    // NO-2: Operations on empty set
    let source = r#"
s <= setOf(@[])
stdout(s.size().toString())
stdout(s.isEmpty().toString())
s2 <= s.add(42)
stdout(s2.size().toString())
stdout(s2.has(42).toString())
"#;
    assert_native_matches_interpreter(source, "no2_set_empty_operations");
}

// ── NO-3: Async ownership model ────────────────────────────

#[test]
fn test_no3_async_int_value_ownership() {
    // NO-3: Async with Int value — should not crash, basic ownership
    let source = r#"
a <= Async[42]()
a ]=> v
stdout(v.toString())
"#;
    assert_native_matches_interpreter(source, "no3_async_int_value");
}

#[test]
fn test_no3_async_string_value_ownership() {
    // NO-3: Async with String value — tagged as STR, released on drop
    let source = r#"
a <= Async["hello world"]()
a ]=> v
stdout(v)
"#;
    assert_native_matches_interpreter(source, "no3_async_string_value");
}

#[test]
fn test_no3_async_pack_value_ownership() {
    // NO-3: Async with Pack (BuchiPack) value — tagged as PACK, recursive release on drop
    let source = r#"
p <= @(name <= "taida", version <= 7)
a <= Async[p]()
a ]=> v
stdout(v.name)
stdout(v.version.toString())
"#;
    assert_native_matches_interpreter(source, "no3_async_pack_value");
}

#[test]
fn test_no3_async_list_value_ownership() {
    // NO-3: Async with List value — tagged as LIST, recursive release on drop
    let source = r#"
lst <= @[1, 2, 3]
a <= Async[lst]()
a ]=> v
stdout(v)
"#;
    assert_native_matches_interpreter(source, "no3_async_list_value");
}

#[test]
fn test_no3_async_all_with_heap_children() {
    // NO-3: All with multiple Asyncs containing values — values collected into list
    let source = r#"
a <= All[@[Async[10](), Async[20](), Async[30]()]]()
a ]=> r
stdout(r)
"#;
    assert_native_matches_interpreter(source, "no3_async_all_heap_children");
}

#[test]
fn test_no3_async_race_with_heap_children() {
    // NO-3: Race — first resolved value is returned
    let source = r#"
a <= Race[@[Async[99]()]]()
a ]=> r
stdout(r.toString())
"#;
    assert_native_matches_interpreter(source, "no3_async_race_heap_children");
}

#[test]
fn test_no3_async_rejected_error_ownership() {
    // NO-3: AsyncReject creates error Pack — tagged as PACK, released on drop
    let source = r#"
a <= AsyncReject[@(type <= "TestError", message <= "something went wrong")]()
stdout(a.isRejected().toString())
"#;
    assert_native_matches_interpreter(source, "no3_async_rejected_error");
}

#[test]
fn test_no3_async_status_methods() {
    // NO-3: Async status methods work correctly with 7-slot layout
    let source = r#"
a <= Async[42]()
stdout(a.isPending().toString())
stdout(a.isFulfilled().toString())
stdout(a.isRejected().toString())
"#;
    assert_native_matches_interpreter(source, "no3_async_status_methods");
}

#[test]
fn test_no3_async_to_string_ownership() {
    // NO-3: toString on Async with 7-slot layout
    let source = r#"
a <= Async[42]()
stdout(a.toString())
b <= Async["hello"]()
stdout(b.toString())
"#;
    assert_native_matches_interpreter(source, "no3_async_to_string");
}

// ── QF-47/48/49 regression: HMAP/SET nested in List and Pack ─────────

#[test]
fn test_qf47_list_of_hashmaps() {
    // QF-47: taida_list_elem_retain/release must handle HMAP tag
    let source = r#"
m1 <= hashMap().set("a", 1)
m2 <= hashMap().set("b", 2)
list <= @[m1, m2]
stdout(list.length().toString())
stdout(list.get(0).getOrDefault(hashMap()).get("a").getOrDefault(0).toString())
stdout(list.get(1).getOrDefault(hashMap()).get("b").getOrDefault(0).toString())
"#;
    assert_native_matches_interpreter(source, "qf47_list_of_hashmaps");
}

#[test]
fn test_qf47_list_of_sets() {
    // QF-47: taida_list_elem_retain/release must handle SET tag
    let source = r#"
s1 <= setOf(@[1, 2, 3])
s2 <= setOf(@[4, 5, 6])
list <= @[s1, s2]
stdout(list.length().toString())
stdout(list.get(0).getOrDefault(setOf(@[])).size().toString())
stdout(list.get(1).getOrDefault(setOf(@[])).size().toString())
"#;
    assert_native_matches_interpreter(source, "qf47_list_of_sets");
}

#[test]
fn test_qf48_pack_with_hmap_field() {
    // QF-48: Pack child release must handle HMAP/SET field tags
    let source = r#"
m <= hashMap().set("x", 42)
p <= @(data <= m, label <= "test")
stdout(p.data.get("x").getOrDefault(0).toString())
stdout(p.label)
"#;
    assert_native_matches_interpreter(source, "qf48_pack_hmap_field");
}

#[test]
fn test_qf58_async_all_retains_values() {
    // QF-58: taida_async_all must retain elements and set elem_type_tag
    let source = r#"
a <= All[@[Async[10](), Async[20](), Async[30]()]]()
a ]=> r
stdout(r)
"#;
    assert_native_matches_interpreter(source, "qf58_async_all_retain");
}

// =========================================================================
// Cross-module quality tests: examples/quality/*/main.td
//
// RCB-214: Sweep multi-module test directories in examples/quality/.
// Each directory contains a main.td (entry point) and optional helper
// modules. Interpreter output is the reference; native must match.
//
// C24 Phase 5 (RC-SLOW-2 / C24B-006): The original
// `test_quality_cross_module_native` looped over 29 module directories
// sequentially (warm 13s). Decomposed to one `#[test]` per directory.
// =========================================================================

/// Directories that intentionally fail (circular imports, etc.) — the
/// original test skipped these entirely.
const QUALITY_CROSS_MODULE_ERROR_TESTS: &[&str] = &[
    "b10a_circular_direct",
    "b10b_circular_indirect",
    "b10d_self_import",
    "b10e_circular_typedef",
    "b10f_circular_closure",
    "b10h_cross_backend_circular",
];

fn run_quality_cross_module_native_fixture(dir_name: &str) {
    if QUALITY_CROSS_MODULE_ERROR_TESTS.contains(&dir_name) {
        return; // Documented interpreter-level error test.
    }

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("quality")
        .join(dir_name);

    // RCB-213: prefer main.td, fall back to main.tdm for versioned imports.
    let main_td = if dir.join("main.td").exists() {
        dir.join("main.td")
    } else {
        dir.join("main.tdm")
    };

    // Interpreter is the reference. If it fails, the original loop silently
    // bumped `skipped`; preserve that behavior.
    let Some(interp) = run_interpreter(&main_td) else {
        return;
    };

    let Some(native) = compile_and_run(&main_td) else {
        panic!("{}: native compile/run failed", dir_name);
    };

    let interp_norm = normalize(&interp);
    let native_norm = normalize(&native);

    if interp_norm != native_norm {
        panic!(
            "{}: output mismatch\n  interpreter: {:?}\n  native:      {:?}",
            dir_name,
            interp.lines().take(5).collect::<Vec<_>>(),
            native.lines().take(5).collect::<Vec<_>>(),
        );
    }
}

#[test]
fn test_quality_cross_module_native_allowlist_guard() {
    use common::fixture_lists::QUALITY_CROSS_MODULE_FIXTURES;
    for t in QUALITY_CROSS_MODULE_ERROR_TESTS {
        assert!(
            QUALITY_CROSS_MODULE_FIXTURES.contains(t),
            "QUALITY_CROSS_MODULE_ERROR_TESTS references unknown dir `{}`",
            t,
        );
    }
}

mod quality_cross_module_native {
    macro_rules! c24_fixture_runner {
        ($stem:expr) => {
            super::run_quality_cross_module_native_fixture($stem)
        };
    }
    include!(concat!(env!("OUT_DIR"), "/quality_cross_module_tests.rs"));
}

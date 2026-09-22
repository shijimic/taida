//! Executable regressions for compiled backend safety and value preservation.
#![cfg(feature = "native")]
mod common;
use common::{run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::{Command, Output};

fn build(profile: &str, td: &Path, bin: &Path) -> Output {
    Command::new(taida_bin())
        .args(["build", profile])
        .arg(td)
        .arg("-o")
        .arg(bin)
        .output()
        .unwrap()
}
fn parity(source: &str, expected: &str) {
    let dir = unique_temp_dir("backend_values");
    let td = dir.join("main.td");
    std::fs::write(&td, source).unwrap();
    assert_eq!(run_interpreter(&td).unwrap().trim_end(), expected);
    for profile in ["native", "wasm-min", "wasm-full"] {
        if profile != "native" && wasmtime_bin().is_none() {
            continue;
        }
        let bin = dir.join(profile);
        let output = build(profile, &td, &bin);
        assert!(
            output.status.success(),
            "{profile}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = if profile == "native" {
            Command::new(&bin).output()
        } else {
            Command::new(wasmtime_bin().unwrap()).arg(&bin).output()
        }
        .unwrap();
        assert!(
            output.status.success(),
            "{profile}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end(),
            expected,
            "{profile}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn float_to_str_preserves_precision() {
    parity(
        r#"Str[0.1234567890123]() >=> a
Str[0.1 + 0.2]() >=> b
Str[1.0]() >=> c
Str[-0.0]() >=> d
Str[0.0000001]() >=> e
stdout(a)
stdout(b)
stdout(c)
stdout(d)
stdout(e)
"#,
        "0.1234567890123\n0.30000000000000004\n1\n-0\n0.0000001",
    );
}

#[test]
fn function_defaults_have_their_own_parameter_scope() {
    parity(
        r#"sum3 a: Int b: Int <= 10 c: Int <= a + b =
  a + b + c
=> :Int
a <= 70
stdout(sum3())
stdout(sum3(1))
stdout(sum3(1, 2))
stdout(sum3(1, 2, 3))
stdout(a)
"#,
        "20\n22\n6\n6\n70",
    );
}

#[test]
fn nested_function_cleanup_stays_in_its_own_scope() {
    parity(
        r#"f1 a: Int =
  x <= 100
  f2 b: Int =
    y <= 200
    f3 c: Int =
      x + y + a + b + c
    => :Int
    f3(3)
  => :Int
  f2(2)
=> :Int
stdout(f1(1))
"#,
        "306",
    );
}

#[test]
#[cfg(target_os = "linux")]
fn discarded_heap_locals_do_not_accumulate_between_calls() {
    if !Path::new("/usr/bin/time").exists() {
        return;
    }
    let dir = unique_temp_dir("native_cleanup_rss");
    let td = dir.join("main.td");
    let values = vec!["0"; 4096].join(", ");
    std::fs::write(
        &td,
        format!(
            r#"scratch n: Int =
  unused <= @[{values}]
  n
=> :Int
loop n: Int =
  | n == 0 |> 0
  | _ |>
    value <= scratch(n)
    loop(value - 1)
=> :Int
stdout(loop(6000))
"#
        ),
    )
    .unwrap();
    let bin = dir.join("main");
    let built = build("native", &td, &bin);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let rss = dir.join("rss");
    let out = Command::new("/usr/bin/time")
        .args(["-f", "%M", "-o"])
        .arg(&rss)
        .arg(&bin)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0");
    let peak_kib: u64 = std::fs::read_to_string(rss)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        peak_kib < 64 * 1024,
        "discarded lists accumulated: peak RSS {peak_kib} KiB"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn large_strings_keep_their_contents_and_key_identity() {
    let source = format!(
        r#"s <= "{}"
stdout(jsonEncode(@[s]).length())
stdout(Chars[s]().length())
stdout(Split[s, ""]().length())
k1 <= s + "z"
k2 <= s + "z"
m <= hashMap().set(k1, 7).set(k2, 9)
stdout(m.size())
m.get(k1) >=> n
stdout(n)
"#,
        "x".repeat(70_000)
    );
    parity(&source, "70004\n70000\n70000\n1\n9");
}

#[test]
fn undefined_bare_variables_fail_during_compilation() {
    let dir = unique_temp_dir("undefined_name");
    let td = dir.join("main.td");
    for source in [
        "stdout(undefinedBare)",
        "stdout(`v=${undefinedBare}`)",
        "f undefinedBare: Int <= 5 =\n  undefinedBare\n=> :Int\nstdout(f())\nstdout(undefinedBare)\n",
    ] {
        std::fs::write(&td, source).unwrap();
        for profile in ["native", "wasm-min"] {
            for check in [true, false] {
                let mut cmd = Command::new(taida_bin());
                if !check {
                    cmd.arg("--no-check");
                }
                let result = cmd
                    .args(["build", profile])
                    .arg(&td)
                    .arg("-o")
                    .arg(dir.join(profile))
                    .output()
                    .unwrap();
                assert!(!result.status.success(), "{source}: {profile}");
                let err = String::from_utf8_lossy(&result.stderr);
                assert!(
                    err.contains("Undefined variable: 'undefinedBare'")
                        || (check && err.contains("[E1542]")),
                    "{err}"
                );
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn native_json_rejects_deep_input_without_overflow() {
    let json = format!("{}0{}", "[".repeat(1000), "]".repeat(1000));
    parity(
        &format!("x <= JSON[\"{json}\", Int]()\nstdout(x.hasValue())\n"),
        "false",
    );
}

fn native_harness(body: &str, sanitize: bool) -> Output {
    let dir = unique_temp_dir("runtime_harness");
    let src = dir.join("check.c");
    let bin = dir.join("check");
    let runtime = *taida::codegen::native_runtime::NATIVE_RUNTIME_C;
    std::fs::write(&src, format!("#define main embedded_main\n{runtime}\n#undef main\nint64_t _taida_main(void) {{ return 0; }}\n{body}")).unwrap();
    let mut cmd = Command::new("cc");
    cmd.args(["-O1", "-ffunction-sections", "-fdata-sections"]);
    if sanitize {
        cmd.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
    }
    let built = cmd
        .arg(&src)
        .args(["-Wl,--gc-sections", "-lm", "-lpthread", "-ldl", "-o"])
        .arg(&bin)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = Command::new(&bin)
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    out
}

#[test]
#[cfg(target_os = "linux")]
fn short_foreign_objects_do_not_trigger_sanitizer_overreads() {
    let out = native_harness(
        r#"
static const char tiny[2] __attribute__((aligned(8))) = "x";
int main(void) {
    if (taida_is_molten((int64_t)(intptr_t)tiny)) return 1;
    if (taida_is_moltenized((int64_t)(intptr_t)tiny)) return 2;
    return 0;
}
"#,
        true,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[cfg(unix)]
fn native_field_registration_is_safe_during_parallel_first_use() {
    let out = native_harness(
        r#"
static const char *names[] = {"audit_a", "audit_b", "audit_c", "audit_d"};
static void *register_fields(void *arg) {
    for (int i = 0; i < 200; i++) {
        taida_register_builtin_error_field_names();
        taida_register_lax_field_names();
        taida_register_stream_field_names();
        taida_register_result_field_names();
        taida_register_zip_enumerate_field_names();
        for (int j = 0; j < 4; j++) {
            taida_register_field_name(123000 + j, (int64_t)(intptr_t)names[j]);
            if (strcmp(taida_lookup_field_name(123000 + j), names[j])) abort();
        }
    }
    return arg;
}
int main(void) {
    pthread_t threads[8];
    for (int i = 0; i < 8; i++) if (pthread_create(&threads[i], NULL, register_fields, NULL)) return 1;
    for (int i = 0; i < 8; i++) pthread_join(threads[i], NULL);
    return 0;
}
"#,
        false,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn native_json_serializer_checks_depth() {
    let out = native_harness(
        r#"
int main(void) {
    int64_t v = taida_list_new();
    for (int i = 0; i < 300; i++) {
        int64_t parent = taida_list_new();
        taida_list_set_elem_tag(parent, TAIDA_TAG_LIST);
        v = taida_list_push(parent, v);
    }
    taida_json_encode(v);
    return 0;
}
"#,
        false,
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("JSON nesting depth limit exceeded"));
}

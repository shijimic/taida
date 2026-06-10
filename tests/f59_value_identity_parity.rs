/// Cross-backend pins for three silent value-corruption holes found
/// while chasing wasm string-pipeline numbers. All three are
/// reference-correct on the interpreter and corrupted values silently
/// on compiled backends.
mod common;

use common::{run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::Command;

fn build_and_run_native(td: &Path, dir: &Path, stem: &str) -> String {
    let bin = dir.join(format!("{stem}_native"));
    let status = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("taida build native runs");
    assert!(status.success(), "native build failed for {stem}");
    let out = Command::new(&bin).output().expect("native binary runs");
    assert!(out.status.success(), "native run failed for {stem}");
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn build_and_run_wasm(td: &Path, dir: &Path, stem: &str) -> Option<String> {
    let wasmtime = wasmtime_bin()?;
    let wasm = dir.join(format!("{stem}.wasm"));
    let status = Command::new(taida_bin())
        .args(["build", "wasm-min"])
        .arg(td)
        .arg("-o")
        .arg(&wasm)
        .status()
        .expect("taida build wasm-min runs");
    assert!(status.success(), "wasm build failed for {stem}");
    let out = Command::new(&wasmtime)
        .arg(&wasm)
        .output()
        .expect("wasmtime runs");
    assert!(out.status.success(), "wasm run failed for {stem}");
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn assert_parity(dir: &Path, stem: &str, source: &str) -> String {
    let td = dir.join(format!("{stem}.td"));
    std::fs::write(&td, source).expect("write fixture");
    let interp = run_interpreter(&td).expect("interpreter runs");
    let native = build_and_run_native(&td, dir, stem);
    assert_eq!(interp, native, "{stem}: interp vs native");
    if let Some(wasm) = build_and_run_wasm(&td, dir, stem) {
        assert_eq!(interp, wasm, "{stem}: interp vs wasm-min");
    } else {
        eprintln!("SKIP: wasmtime not found, wasm leg skipped for {stem}");
    }
    interp
}

/// A top-level `>=>` binding referenced from a function body: the
/// free-variable collector filtered on a set that only Assignment
/// targets entered, so the global slot was never written (and never
/// read) — the function saw 0 instead of the bound value, on native
/// and wasm alike.
#[test]
fn unmold_bound_top_level_is_visible_inside_functions() {
    let dir = unique_temp_dir("f59_global_unmold");
    let out = assert_parity(
        &dir,
        "global_unmold",
        r#"Lax[42]() >=> gv
Split["a-b-c", "-"]() >=> parts

f n: Int =
  gv + n
=> :Int

g n: Int =
  Join[parts, "+"]() >=> j
  j.length() + n
=> :Int

stdout(f(1))
stdout(g(0))
"#,
    );
    assert_eq!(out, "43\n5");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A function-local `>=>` binding whose name shadows a top-level
/// variable must stay local: the bound-variable collector now records
/// unmold bindings, so the global restore cannot clobber the local.
#[test]
fn local_unmold_binding_shadows_top_level_name() {
    let dir = unique_temp_dir("f59_unmold_shadow");
    let out = assert_parity(
        &dir,
        "unmold_shadow",
        r#"Lax[100]() >=> v

f n: Int =
  Lax[7]() >=> v
  v + n
=> :Int

stdout(f(1))
stdout(v)
"#,
    );
    assert_eq!(out, "8\n100");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `stdout(intReturningFn(...))` carried tag UNKNOWN because the
/// FuncCall arm of the static tag table consulted every return kind
/// except Int. With the tag missing, display re-detected the value at
/// runtime — and an Int whose value coincides with a live string's
/// data address carries that string's REAL magic word at v-8, so even
/// positive identification printed the string. The accumulator value
/// 200200 lands exactly on a Split fragment after enough iterations.
#[test]
fn int_returning_function_result_displays_as_int() {
    let dir = unique_temp_dir("f59_int_tag");
    let out = assert_parity(
        &dir,
        "int_tag",
        r#"gsrc <= Repeat["abcdefghij", 1000]()
replaced <= Replace[gsrc, "abc", "xyz"](all <= true)

lp n: Int acc: Int =
  | n == 0 |> acc
  | _ |>
    Split[replaced, "xyz"]() >=> parts
    lp(n - 1, acc + parts.length())
=> :Int

stdout(lp(200, 0))
"#,
    );
    assert_eq!(out, "200200");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A module-level `>=>` binding that is exported: the facade
/// classifier fell through to Function (link failure on native/wasm),
/// and the module-init lowering only stored Assignment targets into
/// the module globals (importers read 0). Both layers are pinned.
#[test]
fn module_unmold_export_carries_its_value() {
    let dir = unique_temp_dir("f59_mod_unmold");
    std::fs::write(
        dir.join("m.td"),
        "Lax[41]() >=> exportedVal\n<<< exportedVal\n",
    )
    .expect("write module");
    let main = dir.join("main.td");
    std::fs::write(
        &main,
        ">>> ./m.td => @(exportedVal)\nstdout(exportedVal + 1)\n",
    )
    .expect("write main");
    let interp = run_interpreter(&main).expect("interpreter runs");
    assert_eq!(interp, "42");
    let native = build_and_run_native(&main, &dir, "mod_unmold");
    assert_eq!(native, "42", "module unmold export: native");
    if let Some(wasm) = build_and_run_wasm(&main, &dir, "mod_unmold") {
        assert_eq!(wasm, "42", "module unmold export: wasm");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Empty-separator Split follows the LOCKED `.split("")` method
/// semantics on every backend and through both spellings: chars split
/// with no empty fragments, empty input gives the empty list. The
/// interpreter's MOLD path used to leak Rust's split("") semantics
/// (leading/trailing empty fragments), splitting the language in two
/// against the method and every compiled backend; the wasm runtime
/// additionally tore multibyte UTF-8 into bytes (codepoints now).
#[test]
fn empty_separator_split_matches_locked_semantics() {
    let dir = unique_temp_dir("f59_split_empty");
    let out = assert_parity(
        &dir,
        "split_empty",
        r#"Split["abc", ""]() >=> a
stdout(a)
m <= "abc".split("")
stdout(m)
Split["", ""]() >=> b
stdout(b)
Split["", "-"]() >=> c
stdout(c)
Split["aXbXc", "X"]() >=> d
stdout(d)
Split["aあb", ""]() >=> u
stdout(u)
"#,
    );
    assert_eq!(
        out,
        "@[\"a\", \"b\", \"c\"]\n@[\"a\", \"b\", \"c\"]\n@[]\n@[\"\"]\n@[\"a\", \"b\", \"c\"]\n@[\"a\", \"あ\", \"b\"]"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Profile-runtime strings must carry the hidden header too: the Lax
/// default `""` of a failed EnvVar/Read used to enter the value space
/// as a bare C literal and render as a pointer-valued integer
/// ("x1692y"), and the BytesCursor `__type` displayed as an integer.
/// wasm-wasi specific (the profile runtime under test).
#[test]
fn wasi_profile_string_defaults_carry_headers() {
    let Some(wasmtime) = wasmtime_bin() else {
        eprintln!("SKIP: wasmtime not found");
        return;
    };
    let dir = unique_temp_dir("f59_wasi_defaults");
    let td = dir.join("defaults.td");
    std::fs::write(
        &td,
        r#"EnvVar["TAIDA_NO_SUCH_VAR_F59"]() >=> v
stdout("x" + v + "y")
Bytes["abc"]() >=> raw
cur <= BytesCursor[raw]()
stdout(cur)
"#,
    )
    .expect("write fixture");
    let wasm = dir.join("defaults.wasm");
    let status = Command::new(taida_bin())
        .args(["build", "wasm-wasi"])
        .arg(&td)
        .arg("-o")
        .arg(&wasm)
        .status()
        .expect("taida build wasm-wasi runs");
    assert!(status.success(), "wasm-wasi build failed");
    let out = Command::new(&wasmtime)
        .arg(&wasm)
        .env_remove("TAIDA_NO_SUCH_VAR_F59")
        .output()
        .expect("wasmtime runs");
    assert!(out.status.success(), "wasm-wasi run failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some("xy"),
        "EnvVar default must be the empty string"
    );
    let cursor_line = lines.next().unwrap_or_default();
    assert!(
        cursor_line.contains("__type <= \"BytesCursor\""),
        "BytesCursor __type must display as a string, got: {cursor_line}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

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

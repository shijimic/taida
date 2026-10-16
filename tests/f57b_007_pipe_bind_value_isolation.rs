// F57B-007: a pipeline `=> name` bind-and-forward must not clobber an outer
// same-named variable's value on native/WASM.
//
// The interpreter binds `=> name` in a child scope, so an outer `name` keeps
// its value after the pipeline. The native/WASM lowerer used to emit
// `DefVar(name, current)`, overwriting the outer slot — a parity violation
// (and, for a String outer value, a native segfault). The fix lowers each
// `=> name` to a fresh synthetic (`#pipe_bind_<id>_<name>`) and rewrites the
// steps that consume the binding to read the synthetic, leaving the outer
// `name` untouched.
//
// interp / native / WASM の一致を pin する（JS は元から interp と一致）。
// native は `taida build native`、WASM は `taida build wasm-wasi` + wasmtime。
// wasmtime が無い環境では WASM チェックを skip する。

mod common;

use common::{taida_bin, unique_temp_dir, wasmtime_bin, write_file};
use std::fs;
use std::process::Command;

fn interp_out(label: &str, src: &str) -> String {
    let dir = unique_temp_dir(label);
    let f = dir.join("main.td");
    write_file(&f, src);
    let out = Command::new(taida_bin())
        .arg(&f)
        .output()
        .expect("run interpreter");
    let _ = fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "interp failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn native_out(label: &str, src: &str) -> String {
    let dir = unique_temp_dir(label);
    let f = dir.join("main.td");
    write_file(&f, src);
    let bin = dir.join("out.bin");
    let comp = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&f)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("native build");
    assert!(
        comp.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&comp.stderr)
    );
    let run = Command::new(&bin).output().expect("native run");
    let _ = fs::remove_dir_all(&dir);
    assert!(
        run.status.success(),
        "native run failed (a String-clobber crash is the original bug): {}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).trim_end().to_string()
}

/// WASM output via `wasm-wasi` + wasmtime, or `None` when wasmtime is absent.
fn wasm_out(label: &str, src: &str) -> Option<String> {
    let wasmtime = wasmtime_bin()?;
    let dir = unique_temp_dir(label);
    let f = dir.join("main.td");
    write_file(&f, src);
    let wasm = dir.join("out.wasm");
    let comp = Command::new(taida_bin())
        .arg("build")
        .arg("wasm-wasi")
        .arg(&f)
        .arg("-o")
        .arg(&wasm)
        .output()
        .expect("wasm build");
    assert!(
        comp.status.success(),
        "wasm build failed: {}",
        String::from_utf8_lossy(&comp.stderr)
    );
    let run = Command::new(&wasmtime)
        .arg(&wasm)
        .output()
        .expect("run wasmtime");
    let _ = fs::remove_dir_all(&dir);
    assert!(
        run.status.success(),
        "wasm run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8_lossy(&run.stdout).trim_end().to_string())
}

/// Assert interp == native (== wasm when available) for `src`, and that the
/// interpreter baseline equals `expect`.
fn assert_parity(label: &str, src: &str, expect: &str) {
    let i = interp_out(&format!("{label}_i"), src);
    assert_eq!(i, expect, "{label}: interp baseline");
    let n = native_out(&format!("{label}_n"), src);
    assert_eq!(
        n, i,
        "{label}: native must match interp (outer var must survive the bind)"
    );
    if let Some(w) = wasm_out(&format!("{label}_w"), src) {
        assert_eq!(w, i, "{label}: wasm must match interp");
    }
}

/// A List outer value survives a same-named pipeline bind: the pipeline prints
/// the forwarded `5`, then the outer `p` is still `@[9]` afterward.
#[test]
fn bind_does_not_clobber_outer_list() {
    assert_parity(
        "f57b_007_clobber_list",
        "p <= @[9]\n5 => p => stdout(p.toString())\nstdout(p.toString())\n",
        "5\n@[9]",
    );
}

/// An Int outer value survives a same-named pipeline bind that forwards a Bool.
#[test]
fn bind_does_not_clobber_outer_int() {
    assert_parity(
        "f57b_007_clobber_int",
        "p <= 42\ntrue => p => stdout(p.toString())\nstdout(p.toString())\n",
        "true\n42",
    );
}

/// A String outer value survives a same-named pipeline bind. On main this
/// crashed native at run time (the clobbering `DefVar` overwrote a heap String
/// slot with a scalar); it must now print the forwarded `5` then the intact
/// outer `"outer"`.
#[test]
fn bind_does_not_clobber_outer_string() {
    assert_parity(
        "f57b_007_clobber_string",
        "p <= \"outer\"\n5 => p => stdout(p.toString())\nstdout(p.toString())\n",
        "5\nouter",
    );
}

/// Because the bind never touches the outer name, a later plain reassignment of
/// that name is its first (clean) definition: no forwarded kind leaks past it.
/// `flag => x => stdout(_.toString())` prints `true`; after `x <= 5`,
/// `x => stdout(_.toString())` prints `5` (Int), not the stale Bool `true`.
#[test]
fn bind_forward_kind_does_not_leak_past_reassignment() {
    assert_parity(
        "f57b_007_no_leak",
        "flag <= true\n\
         flag => x => stdout(_.toString())\n\
         x <= 5\n\
         x => stdout(_.toString())\n",
        "true\n5",
    );
}

/// A cond-branch step that references the bound name reads the binding (rewritten
/// to the synthetic through the `CondArm` expression arms), while an outer
/// same-named variable keeps its value. With an outer `x <= 9` and
/// `5 => x => (| x > 0 |> x | x < 0 |> 0 | _ |> 100) => stdout(_.toString())`,
/// the arm sees the bound `5` (> 0 → yields it); the outer `x` is still `9`.
/// This also guards the rewrite's `CondBranch` arm: were the bound name read
/// from the outer variable, the arm would branch on `9` and the trailing read
/// would diverge.
#[test]
fn bind_in_cond_branch_reads_binding_not_outer() {
    assert_parity(
        "f57b_007_cond_bind",
        "x <= 9\n\
         5 => x => (| x > 0 |> x | x < 0 |> 0 | _ |> 100) => stdout(_.toString())\n\
         stdout(x.toString())\n",
        "5\n9",
    );
}

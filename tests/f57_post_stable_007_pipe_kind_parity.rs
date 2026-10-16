// POST-STABLE-007 (F57 T2): pipeline placeholder の動的 kind 運搬 parity。
//
// `flag => stdout(_.toString())` の placeholder `_` が運ぶ値の静的 kind が、
// native/WASM lowering の `#pipe_prev` 合成束縛（kind を持たない `DefVar`）
// を経由して UNKNOWN に落ち、Bool/Float が Int 表示の polymorphic 経路に
// 縮退していた（native=`1` / wasm=`1`、`x <= 3.0 => ...` は f64 bit-pattern）。
// `lower_pipeline_step` が前段の静的型を `#pipe_prev` に追跡することで解消。
//
// F57B-007 で `=> name` bind-and-forward は実名ではなく合成名
// (`#pipe_bind_<id>_<name>`) に束縛されるようになった。前段の式は active な
// bind を通して rename されてから kind 追跡されるので、bind-forward 経由でも、
// 連鎖 forward (`true => a => b => ...`) でも kind が正しく伝播する。kind 集合は
// 6 種すべて（Bool/Float/String/Pack/List/Int）を追跡する。
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
        "native run failed: {}",
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
    assert_eq!(n, i, "{label}: native must match interp");
    if let Some(w) = wasm_out(&format!("{label}_w"), src) {
        assert_eq!(w, i, "{label}: wasm must match interp");
    }
}

// ── bind-なし placeholder kind (POST-STABLE-007 の本来スコープ) ──────────

/// A Bool piped into a placeholder consumer must render as `true`, not `1`.
#[test]
fn pipe_placeholder_bool_kind_parity() {
    assert_parity(
        "f57_007_bool",
        "flag <= true\nflag => stdout(_.toString())\n",
        "true",
    );
}

/// A Float piped into a placeholder consumer must render as `3.0`, not the
/// Int-display of its f64 bit pattern.
#[test]
fn pipe_placeholder_float_kind_parity() {
    assert_parity(
        "f57_007_float",
        "x <= 3.0\nx => stdout(_.toString())\n",
        "3.0",
    );
}

// ── bind-and-forward `=> name` を跨いだ kind 運搬 ───────────────────────

/// The kind survives an intermediate `=> name` bind-and-forward stage when the
/// consumer uses the placeholder: `flag => x => stdout(_.toString())`. The
/// forwarded value's kind is recorded on the synthetic bind, so `_` still
/// dispatches Bool (regressed to native=`1` before the kind propagation).
#[test]
fn bind_forward_bool_kind_parity_placeholder() {
    assert_parity(
        "f57_007_bf_bool",
        "flag <= true\nflag => x => stdout(_.toString())\n",
        "true",
    );
}

#[test]
fn bind_forward_float_kind_parity_placeholder() {
    assert_parity(
        "f57_007_bf_float",
        "v <= 3.0\nv => y => stdout(_.toString())\n",
        "3.0",
    );
}

/// The kind also survives when the consumer references the bound name directly
/// (`flag => x => stdout(x.toString())`): the reference is rewritten to the
/// synthetic, which carries the Bool kind.
#[test]
fn bind_forward_bool_kind_parity_named() {
    assert_parity(
        "f57_007_bf_named",
        "flag <= true\nflag => x => stdout(x.toString())\n",
        "true",
    );
}

/// A chained forward propagates the kind stage by stage: in
/// `true => a => b => stdout(_.toString())` the Bool kind reaches `b`'s
/// synthetic via `a`'s, so the placeholder consumer still renders `true`.
#[test]
fn chained_bind_forward_bool_kind_parity() {
    assert_parity(
        "f57_007_chain_bool",
        "true => a => b => stdout(_.toString())\n",
        "true",
    );
}

// ── 6-set 追跡: Pack / List も bind-forward を跨いで正しく扱う ───────────

/// A Pack forwarded through a bind keeps its kind, so a field access on the
/// bound name resolves correctly: `p => q => stdout(q.n.toString())` → `7`.
#[test]
fn bind_forward_pack_kind_parity() {
    assert_parity(
        "f57_007_bf_pack",
        "p <= @(n <= 7)\np => q => stdout(q.n.toString())\n",
        "7",
    );
}

/// A List forwarded through a bind keeps its kind, so a list method on the
/// bound name resolves correctly: `lst => m => stdout(m.length().toString())`.
#[test]
fn bind_forward_list_kind_parity() {
    assert_parity(
        "f57_007_bf_list",
        "lst <= @[1, 2, 3]\nlst => m => stdout(m.length().toString())\n",
        "3",
    );
}

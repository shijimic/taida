//! Pack display and HashMap/Set JSON representation across backends.
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

/// runtime-built packs (ABI descriptors, monadic
/// shapes) render their full field set including the `__` namespace on
/// every backend. Before the renderer-side field-name registration the
/// compiled backends printed `@` or dropped `__type`.
#[test]
fn internal_pack_display_matches_interpreter() {
    let dir = unique_temp_dir("f64_pack_display");
    let out = assert_parity(
        &dir,
        "packs",
        r#"db <= HostCapability["DB", "mock/kind"]()
stdout(db)
s <= "a1b2".indexOfLax("b")
stdout(s)
p <= @(f <= 1.5, b <= false)
stdout(p)"#,
    );
    assert!(
        out.contains("__type <= \"HostCapability\""),
        "HostCapability must render __type: {out}"
    );
    assert!(
        out.contains("__default <= 0"),
        "Lax must render its full monadic shape: {out}"
    );
}

///  (Regex leg): RegexMatch packs carry has_value / full /
/// groups / start / __type identically on interp and native. wasm-min
/// does not support Regex, so this pin is two-backend by design.
#[test]
fn regex_match_display_matches_interpreter() {
    let dir = unique_temp_dir("f64_regex_display");
    let td = dir.join("re.td");
    std::fs::write(
        &td,
        r#"re <= Regex("a(b+)c", "")
m <= "xxabbbcyy".match(re)
stdout(m)
n <= "zzz".match(re)
stdout(n)"#,
    )
    .expect("write fixture");
    let interp = run_interpreter(&td).expect("interpreter runs");
    let bin = dir.join("re_native");
    let status = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(&td)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("taida build native runs");
    assert!(status.success(), "native build failed");
    let out = Command::new(&bin).output().expect("native binary runs");
    assert!(out.status.success(), "native run failed");
    let native = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    assert_eq!(interp, native);
    assert!(
        interp.contains("__type <= \"RegexMatch\""),
        "RegexMatch must render __type: {interp}"
    );
}

/// homogeneous HashMap values keep their Float/Bool kind in
/// display on every backend (the kind-aware branch of the HashMap
/// renderer). Heterogeneous maps fall to the pointer heuristics — that
/// is the per-entry-tag representation track, not a display fix.
#[test]
fn hashmap_homogeneous_value_kinds_render_on_all_backends() {
    let dir = unique_temp_dir("f64_hm_kinds");
    let out = assert_parity(
        &dir,
        "hmk",
        r#"mf <= hashMap().set("a", 1.5).set("b", 2.5)
stdout(mf)
mb <= hashMap().set("x", true).set("y", false)
stdout(mb)
mi <= hashMap().set("p", 7).set("q", 8)
stdout(mi)
s <= setOf(@[2.5, 1.5])
stdout(s)"#,
    );
    assert!(
        out.contains("\"a\": 1.5") && out.contains("\"x\": true"),
        "HashMap values must keep Float/Bool kinds: {out}"
    );
}

/// jsonEncode/jsonPretty emit the public wire shape —
/// HashMap as a plain object, Set as a plain array — with no
/// `__entries` / `__items` machinery on any backend, and nested
/// Float elements inside a HashMap's List value keep their kind.
#[test]
fn json_encode_emits_public_shapes_for_containers() {
    let dir = unique_temp_dir("f64_json_public");
    let out = assert_parity(
        &dir,
        "jsonpub",
        r#"m <= hashMap().set("a", 1).set("b", "x")
stdout(jsonEncode(m))
s <= setOf(@[1, 2, 3])
stdout(jsonEncode(s))
mn <= hashMap().set("k", @[1.5])
stdout(jsonEncode(mn))
p <= @(name <= "Asuka", active <= true, score <= 1.5)
stdout(jsonPretty(p))"#,
    );
    assert!(
        !out.contains("__entries"),
        "no internal entries leak: {out}"
    );
    assert!(!out.contains("__items"), "no internal items leak: {out}");
    assert!(out.contains("{\"a\":1,\"b\":\"x\"}"), "map shape: {out}");
    assert!(out.contains("[1,2,3]"), "set shape: {out}");
    assert!(out.contains("{\"k\":[1.5]}"), "nested float kind: {out}");
}

/// the runtime-constructed TODO mold renders all
/// seven fields — including the `__value` / `__default` monadic slots —
/// with the constructed values (not zero-truncated) on every backend.
/// Before the per-slot tag stamping the full renderer's Int branch ate
/// the tag-less `sol` / `__value` slots on native.
#[test]
fn todo_mold_display_matches_interpreter() {
    let dir = unique_temp_dir("f64_todo_display");
    let out = assert_parity(
        &dir,
        "todo",
        r#"t <= TODO[](
  id <= "TASK-1",
  task <= "first",
  sol <= 7,
  unm <= 9
)
stdout(t)"#,
    );
    assert!(
        out.contains(
            "id <= \"TASK-1\", task <= \"first\", sol <= 7, unm <= 9, \
             __value <= 7, __default <= 9, __type <= \"TODO\""
        ),
        "TODO must render its full seven-field shape: {out}"
    );
}

/// jsonEncode keeps the map's insertion order (the
/// insertion-order contract) even for key sets whose insertion order differs
/// from alphabetical order, and keeps the homogeneous Float value kind.
/// Pack fields still encode in alphabetical order on every backend.
/// (Mixed-kind maps are the per-entry-tag representation track: their
/// compiled-backend encoding falls to the single-tag latch like their
/// display does.)
#[test]
fn json_encode_key_order_and_kinds_match_across_backends() {
    let dir = unique_temp_dir("f64_json_order");
    let out = assert_parity(
        &dir,
        "jsonord",
        r#"m <= hashMap().set("zeta", 1).set("alpha", 2).set("mid", 3).set("omega", "end")
stdout(jsonEncode(m))
mf <= hashMap().set("a", 1.5).set("b", 2.5)
stdout(jsonEncode(mf))
p <= @(zeta <= 1, alpha <= "two", mid <= @[1, 2])
stdout(jsonEncode(p))"#,
    );
    assert!(
        out.contains("{\"zeta\":1,\"alpha\":2,\"mid\":3,\"omega\":\"end\"}"),
        "insertion order preserved: {out}"
    );
    assert!(
        out.contains("{\"a\":1.5,\"b\":2.5}"),
        "homogeneous Float kind kept: {out}"
    );
    assert!(
        out.contains("{\"alpha\":\"two\",\"mid\":[1,2],\"zeta\":1}"),
        "pack fields encode alphabetically: {out}"
    );
}

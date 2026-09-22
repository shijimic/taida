//! Type checking, canonical caught errors and explicit Lax unmolding.
mod common;

use common::{taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::Command;

fn way_check(td: &Path) -> (bool, String) {
    let out = Command::new(taida_bin())
        .args(["way", "check"])
        .arg(td)
        .output()
        .expect("taida way check runs");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

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

/// an Int arm followed by a Float arm must be a type error.
/// The removed numeric-pair exception kept the FIRST arm's type as the
/// branch type while the interpreter returned the dynamic value, so
/// native printed bit-reinterpreted garbage (4609434218613702656).
#[test]
fn f64b009_mixed_numeric_cond_rejected() {
    let dir = unique_temp_dir("f64_c9_reject");
    let td = dir.join("c9.td");
    std::fs::write(
        &td,
        "f x: Int =
  | x == 1 |> 2
  | _ |> 1.5
=> :Int
stdout(f(0))",
    )
    .expect("write fixture");
    let (ok, output) = way_check(&td);
    assert!(!ok, "mixed numeric cond must not pass the checker");
    assert!(
        output.contains("[E1603]"),
        "expected [E1603], got: {output}"
    );
}

///  positive leg: uniform-typed cond branches keep identical
/// output across all three backends.
#[test]
fn f64b009_uniform_cond_parity() {
    let dir = unique_temp_dir("f64_c9_uniform");
    let td = dir.join("c9u.td");
    std::fs::write(
        &td,
        r#"f x: Int =
  | x == 1 |> 2
  | _ |> 5
=> :Int
g x: Int =
  | x == 1 |> 2.5
  | _ |> 7.5
=> :Float
stdout(f(0))
stdout(f(1))
stdout(g(0))
stdout(g(1))"#,
    )
    .expect("write fixture");
    let interp_out = Command::new(taida_bin())
        .arg(&td)
        .output()
        .expect("interpreter runs");
    assert!(interp_out.status.success());
    let interp = String::from_utf8_lossy(&interp_out.stdout)
        .trim_end()
        .to_string();
    assert_eq!(interp.lines().count(), 4);
    let native = build_and_run_native(&td, &dir, "c9u");
    assert_eq!(interp, native, "uniform Int/Float conds: interp vs native");
    if let Some(wasm) = build_and_run_wasm(&td, &dir, "c9u") {
        assert_eq!(interp, wasm, "uniform Int/Float conds: interp vs wasm-min");
    }
    assert!(interp.starts_with("5\n2\n7.5\n2.5"), "outputs: {interp}");
}

/// every caught error exposes type/message/kind/code inside the
/// handler scope — user throws included. Before the catch-site
/// canonicalization a bare `Error(...).throw` pack had no kind slot and
/// `.kind` crashed at runtime with "Field 'kind' does not exist".
#[test]
fn f64b010_user_throw_exposes_errorinfo_shape() {
    let dir = unique_temp_dir("f64_k3_canon");
    let td = dir.join("k3.td");
    std::fs::write(
        &td,
        r#"f x: Int =
  |== e: Error =
    stdout(e)
    stdout(e.type)
    stdout(e.message)
    stdout(e.kind)
    stdout(e.code.toString())
    "caught"
  => :Str
  | x == 1 |> Error(type <= "Boom", message <= "exploded").throw()
  | _ |> "no"
=> :Str
stdout(f(1))"#,
    )
    .expect("write fixture");
    let interp_out = Command::new(taida_bin())
        .arg(&td)
        .output()
        .expect("interpreter runs");
    assert!(interp_out.status.success(), "interp failed");
    let interp = String::from_utf8_lossy(&interp_out.stdout)
        .trim_end()
        .to_string();
    let native = build_and_run_native(&td, &dir, "k3");
    assert_eq!(interp, native, "caught-error shape: interp vs native");
    if let Some(wasm) = build_and_run_wasm(&td, &dir, "k3") {
        assert_eq!(interp, wasm, "caught-error shape: interp vs wasm-min");
    }
    // Canonicalization only ADDS the missing kind/code slots to the thrown
    // pack — it never rebuilds it. The thrown builtin Error pack keeps its
    // own slot order (type, message, __type) and its own __type value
    // ("Error", the constructor name); `e.type` stays the user-supplied
    // "Boom". The old expectation (`__type <= "Boom"`, kind/code before
    // __type) pinned the pre--review rebuild shape.
    assert!(
        interp.contains(
            "@(type <= \"Boom\", message <= \"exploded\", __type <= \"Error\", \
             kind <= \"Boom\", code <= 0)"
        ),
        "canonical ErrorInfo shape: {interp}"
    );
}

///  checker leg: kind/code access on a caught Error passes the
/// checker; fields outside the canonicalized ErrorInfo shape stay rejected.
#[test]
fn f64b010_checker_declared_shape_pins() {
    let dir = unique_temp_dir("f64_k4_checker");
    let ok_td = dir.join("ok.td");
    std::fs::write(
        &ok_td,
        r#"f x: Int =
  |== e: Error =
    "kind: " + e.kind + "/" + e.code.toString()
  => :Str
  | x == 1 |> "no"
  | _ |> "no"
=> :Str
stdout(f(0))"#,
    )
    .expect("write fixture");
    let (ok, output) = way_check(&ok_td);
    assert!(
        ok,
        "kind/code access on caught Error must pass, got: {output}"
    );

    let bad_td = dir.join("bad.td");
    std::fs::write(
        &bad_td,
        r#"f x: Int =
  |== e: Error =
    "sev: " + e.severity
  => :Str
  | x == 1 |> "no"
  | _ |> "no"
=> :Str
stdout(f(0))"#,
    )
    .expect("write fixture");
    let (ok, output) = way_check(&bad_td);
    assert!(!ok, "undeclared error field must be rejected");
    assert!(
        output.contains("[E1602]"),
        "expected [E1602], got: {output}"
    );
}

/// a `_`-free pipeline stage applies the piped value as the
/// stage function's only argument, so the checker mirrors the direct-call
/// rules there — wrong parameter type is [E1506], zero-param functions are
/// [E1301]. Arity beyond one stays legal (defaults fill the rest), matching
/// `f(5)` for a two-param `f`.
#[test]
fn f64b004_pipeline_stage_injection_checked() {
    let dir = unique_temp_dir("f64_pipe_stage");

    let type_bad = dir.join("type_bad.td");
    std::fs::write(
        &type_bad,
        r#"g s: Str =
  s.length()
=> :Int
5 => g => r2
stdout(r2)"#,
    )
    .expect("write fixture");
    let (ok, output) = way_check(&type_bad);
    assert!(!ok, "Int pipe into Str param must be rejected");
    assert!(
        output.contains("[E1506]"),
        "expected [E1506], got: {output}"
    );

    let zero_bad = dir.join("zero_bad.td");
    std::fs::write(
        &zero_bad,
        r#"t =
  42
=> :Int
5 => t => r
stdout(r)"#,
    )
    .expect("write fixture");
    let (ok, output) = way_check(&zero_bad);
    assert!(!ok, "zero-param bare stage must be rejected");
    assert!(
        output.contains("[E1301]"),
        "expected [E1301], got: {output}"
    );

    // Positive legs: defaults fill extra params (matches `f(5)`), partial
    // applications receive the pipe as their remaining argument, and mold
    // stages keep working.
    let ok_td = dir.join("ok.td");
    std::fs::write(
        &ok_td,
        r#"f a: Int b: Int =
  a + b
=> :Int
add2 <= f(, 1)
7 => add2 => incd
stdout(incd)
5 => f => defaulted
stdout(defaulted)
"  hello  " => Trim[_]() => Upper[_]() => s
stdout(s)"#,
    )
    .expect("write fixture");
    let (ok, output) = way_check(&ok_td);
    assert!(ok, "legal bare stages must pass, got: {output}");
}

/// pack elements in a list literal must be width subtypes of the
/// first element — the unified list type IS the first element's shape, so a
/// later element missing a field would crash on access while the checker
/// claimed the field exists.
#[test]
fn f64b011_pack_list_width_subtype() {
    let dir = unique_temp_dir("f64_pack_list");

    let bad_td = dir.join("bad.td");
    std::fs::write(
        &bad_td,
        "packs <= @[@(x <= 1), @(y <= 2)]\nstdout(packs.length())",
    )
    .expect("write fixture");
    let (ok, output) = way_check(&bad_td);
    assert!(!ok, "disjoint-field pack list must be rejected");
    assert!(
        output.contains("[E0401]"),
        "expected [E0401], got: {output}"
    );

    let ok_td = dir.join("ok.td");
    std::fs::write(
        &ok_td,
        r#"packs <= @[@(x <= 1), @(x <= 2, y <= "a")]
stdout(packs.length())"#,
    )
    .expect("write fixture");
    let (ok, output) = way_check(&ok_td);
    assert!(ok, "width-subtype pack list must pass, got: {output}");
}

/// setOf types its element from the source list, so a mismatched
/// Set annotation is rejected through the Generic×Generic argument
/// comparison. Element-opaque sources (hashMap, empty lists) keep flowing
/// through the permissive Named↔Generic rule.
#[test]
fn f64b012_set_element_type_checked() {
    let dir = unique_temp_dir("f64_set_elem");

    let bad_td = dir.join("bad.td");
    std::fs::write(&bad_td, "s: Set[Str] <= setOf(@[1, 2])\nstdout(s.size())")
        .expect("write fixture");
    let (ok, output) = way_check(&bad_td);
    assert!(!ok, "Set[Str] <= setOf(@[1, 2]) must be rejected");
    assert!(
        output.contains("Set[Int]"),
        "expected Set[Int] in the diagnostic, got: {output}"
    );

    let ok_td = dir.join("ok.td");
    std::fs::write(
        &ok_td,
        r#"s: Set[Str] <= setOf(@["a", "b"])
stdout(s.size())
m: HashMap[Str, Int] <= hashMap()
stdout(m.length())
e <= setOf(@[])
stdout(e.size())"#,
    )
    .expect("write fixture");
    let (ok, output) = way_check(&ok_td);
    assert!(
        ok,
        "matching / element-opaque sources must pass, got: {output}"
    );

    // A heterogeneous literal fed straight into setOf is a legitimate
    // value-tagged Set (the runtime keeps per-element kinds), so the
    // literal's own [E0401] must not reject the program. The same literal
    // bound directly still reports [E0401].
    let mixed_ok = dir.join("mixed_ok.td");
    std::fs::write(
        &mixed_ok,
        "s <= setOf(@[1, true])\nstdout(s.size())\nxs <= @[1, \"x\"]",
    )
    .expect("write fixture");
    let (ok, output) = way_check(&mixed_ok);
    assert!(
        !ok,
        "setOf mixed literal passes but the direct binding still errors"
    );
    assert!(
        output.contains("[E0401]"),
        "direct mixed binding keeps its [E0401], got: {output}"
    );

    let pure_mixed_ok = dir.join("pure_mixed.td");
    std::fs::write(
        &pure_mixed_ok,
        "Enum => Color = :Red\nEnum => Size = :Small\ns <= setOf(@[1, true])\nstdout(s.size())\ns2 <= setOf(@[Color:Red(), Size:Small()])\nstdout(s2.size())",
    )
    .expect("write fixture");
    let (ok, output) = way_check(&pure_mixed_ok);
    assert!(ok, "mixed setOf literals must pass, got: {output}");
}

/// the E1525 "cannot infer operand type" diagnostic must not be
/// gated on the whole-program error state (one unrelated earlier error used
/// to swallow every later E1525), and must name the actual operator.
#[test]
fn f64b013_e1525_not_whack_a_mole() {
    let dir = unique_temp_dir("f64_e1525_gate");

    let multi_td = dir.join("multi.td");
    std::fs::write(
        &multi_td,
        "oops <= 1 + \"x\"\nr <= unknownFn() * 2\nstdout(r)",
    )
    .expect("write fixture");
    let (ok, output) = way_check(&multi_td);
    let _ = ok;
    assert!(
        output.contains("[E1525] Cannot infer operand type for `*`"),
        "E1525 must survive an earlier unrelated error and name `*`, got: {output}"
    );

    let solo_td = dir.join("solo.td");
    std::fs::write(&solo_td, "r <= unknownFn() * 2\nstdout(r)").expect("write fixture");
    let (_ok, output) = way_check(&solo_td);
    assert!(
        output.contains("[E1525] Cannot infer operand type for `*`"),
        "solo E1525 must name `*`, got: {output}"
    );
}

/// the arithmetic operand mismatch diagnostic carries the [E1619]
/// code (comparison E1605 / logic E1606 / unary E1607 already do) and names
/// the operator symbol.
#[test]
fn f64b015_arithmetic_mismatch_has_code() {
    let dir = unique_temp_dir("f64_e1619");

    let td = dir.join("e.td");
    std::fs::write(&td, "x <= 1 + @[2]\nstdout(x)").expect("write fixture");
    let (_ok, output) = way_check(&td);
    assert!(
        output.contains("[E1619] Cannot apply `+` to Int and @[Int]"),
        "expected coded E1619 diagnostic, got: {output}"
    );
}

/// a method call directly on a Lax receiver must be rejected by
/// the checker (the accurate OS-mold return types make E1509 fire); the
/// sanctioned `>=>` unmold route keeps working with parity.
#[test]
fn f64b016_lax_direct_method_rejected_unmold_parity() {
    let dir = unique_temp_dir("f64_lax_method");

    let bad_td = dir.join("bad.td");
    std::fs::write(
        &bad_td,
        "s <= \"a1b2\".indexOfLax(\"b\")\nstdout(s.length())",
    )
    .expect("write fixture");
    let (ok, output) = way_check(&bad_td);
    assert!(!ok, "direct method call on Lax must be rejected");
    assert!(
        output.contains("[E1509]"),
        "expected [E1509], got: {output}"
    );

    let ok_td = dir.join("good.td");
    std::fs::write(
        &ok_td,
        r#"r <= "a1b2".indexOfLax("b")
r >=> v
stdout(v.toString())
miss <= "zzz".indexOfLax("b")
stdout(miss.has_value.toString())
m2 <= "zzz".indexOfLax("b")
m2 >=> d
stdout(d.toString())"#,
    )
    .expect("write fixture");
    let interp_out = Command::new(taida_bin())
        .arg(&ok_td)
        .output()
        .expect("interpreter runs");
    assert!(interp_out.status.success());
    let interp = String::from_utf8_lossy(&interp_out.stdout)
        .trim_end()
        .to_string();
    let native = build_and_run_native(&ok_td, &dir, "laxunmold");
    assert_eq!(interp, native, "unmold route: interp vs native");
    if let Some(wasm) = build_and_run_wasm(&ok_td, &dir, "laxunmold") {
        assert_eq!(interp, wasm, "unmold route: interp vs wasm-min");
    }
}

/// cyclic type aliases get a dedicated [E1632] naming the cycle
/// chain, instead of only the baffling half-expanded mismatch downstream
/// (`expected @[@[@[B]]], got @[@[Int]]`). Acyclic aliases — forward
/// references included — keep resolving through the fixpoint.
#[test]
fn f64b014_cyclic_alias_dedicated_diagnostic() {
    let dir = unique_temp_dir("f64_alias_cycle");

    let mutual_td = dir.join("mutual.td");
    std::fs::write(
        &mutual_td,
        "A = @[B]\nB = @[A]\nxs: A <= @[@[1]]\nstdout(xs.length())",
    )
    .expect("write fixture");
    let (ok, output) = way_check(&mutual_td);
    assert!(!ok, "mutual alias cycle must be rejected");
    assert!(
        output.contains("[E1632] Type alias expansion cycle detected: A -> B -> A"),
        "expected coded cycle diagnostic, got: {output}"
    );

    let self_td = dir.join("self.td");
    std::fs::write(&self_td, "A = @[A]\nx <= 5\nstdout(x)").expect("write fixture");
    let (ok, output) = way_check(&self_td);
    assert!(!ok, "self alias cycle must be rejected");
    assert!(
        output.contains("[E1632] Type alias expansion cycle detected: A -> A"),
        "expected coded self-cycle diagnostic, got: {output}"
    );

    let ok_td = dir.join("acyclic.td");
    std::fs::write(
        &ok_td,
        "Outer = @[InnerList]\nInnerList = @[Int]\nxs: Outer <= @[@[1], @[2]]\nstdout(xs.length())",
    )
    .expect("write fixture");
    let (ok, output) = way_check(&ok_td);
    assert!(
        ok,
        "forward-referencing acyclic aliases must still resolve, got: {output}"
    );

    // Review-fix: the cycle DFS bails out mid-walk and used to leave its
    // nodes GRAY — a later root then saw a phantom back edge. With the
    // real cycle `A -> B` reachable from `C = @[B]`... the concrete case:
    // D depends on A; A/B/C form the cycle B -> C -> B. Only that one
    // cycle may be reported; `D -> A` must NOT appear.
    let tail_td = dir.join("tail.td");
    std::fs::write(
        &tail_td,
        "D = @[A]\nA = @[B]\nB = @[C]\nC = @[B]\nx <= 5\nstdout(x)",
    )
    .expect("write fixture");
    let (ok, output) = way_check(&tail_td);
    assert!(!ok, "the B/C alias cycle must be rejected");
    assert!(
        output.contains("[E1632] Type alias expansion cycle detected: B -> C -> B"),
        "expected the real cycle diagnostic, got: {output}"
    );
    assert!(
        !output.contains("D -> A"),
        "stale-GRAY marking must not invent a phantom D -> A cycle, got: {output}"
    );
}

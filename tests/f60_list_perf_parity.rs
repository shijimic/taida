/// Cross-backend pins for the list-performance track's correctness
/// findings.
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

/// A function whose body ends in an unmolded Fold could not be written
/// at all: the static return-type table read the LIST argument instead
/// of the INIT accumulator, so `=> :Int` failed with "body returns
/// @[Int]" on every such function.
#[test]
fn function_ending_in_fold_typechecks_and_runs() {
    let dir = unique_temp_dir("f60_fold_ret");
    let out = assert_parity(
        &dir,
        "fold_ret",
        r#"sumIt u: Int =
  xs <= @[1, 2, 3]
  Fold[xs, 0, _ acc: Int x: Int = acc + x]() >=> t
  t
=> :Int

joinIt u: Int =
  ws <= @["a", "b"]
  Foldr[ws, "", _ acc: Str w: Str = acc + w]() >=> s
  s.length()
=> :Int

stdout(sumIt(1))
stdout(joinIt(1))
"#,
    );
    assert_eq!(out, "6\n2");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The consume-append rewrite: a tail-recursive `f(n-1, Append[acc, x]())`
/// pushes in place once the loop owns its accumulator. The FIRST
/// activation must NOT consume — the accumulator may be the caller's
/// list. This is the pin for that detach.
#[test]
fn consume_append_detaches_callers_list() {
    let dir = unique_temp_dir("f60_consume_detach");
    let out = assert_parity(
        &dir,
        "consume_detach",
        r#"build n: Int acc: @[Int] =
  | n == 0 |> acc
  | _ |> build(n - 1, Append[acc, n]())
=> :@[Int]

xs <= @[100]
ys <= build(3, xs)
stdout(xs)
stdout(ys)
zs <= build(2, xs)
stdout(xs)
stdout(zs)
"#,
    );
    assert_eq!(out, "@[100]\n@[100, 3, 2, 1]\n@[100]\n@[100, 2, 1]");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fail-closed shapes: anything that could observe the old list keeps
/// the copy semantics on every backend.
#[test]
fn consume_append_fails_closed_on_unsafe_shapes() {
    let dir = unique_temp_dir("f60_consume_failclosed");
    // acc used twice in the recursive arm (second use after the append).
    let out = assert_parity(
        &dir,
        "twice",
        r#"build n: Int acc: @[Int] =
  | n == 0 |> acc
  | _ |>
    grown <= Append[acc, n]()
    last <= acc.length()
    build(n - 1, Append[grown, last]())
=> :@[Int]

stdout(build(2, @[7]).length())
"#,
    );
    assert_eq!(out, "5");
    // Append[p, p-derived] — the item reads the accumulator.
    let out2 = assert_parity(
        &dir,
        "self_item",
        r#"build n: Int acc: @[Int] =
  | n == 0 |> acc
  | _ |> build(n - 1, Append[acc, acc.length()]())
=> :@[Int]

stdout(build(3, @[]))
"#,
    );
    assert_eq!(out2, "@[0, 1, 2]");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The method-form unmold fusion: `Mold[...]().unmold()` inside a
/// lambda body must produce the exact values of the materialised path
/// — negative operands pin the truncated Div/Mod semantics, the
/// variable-divisor shape pins the NON-fused path against the fused
/// one, and the top-level direct form covers the non-lambda receiver.
#[test]
fn method_form_unmold_fusion_matches_materialised_path() {
    let dir = unique_temp_dir("f60_unmold_fusion");
    let out = assert_parity(
        &dir,
        "fusion",
        r#"applyMod xs: @[Int] = Map[xs, _ x: Int = Mod[x, 4]().unmold()]() => :@[Int]
applyDiv xs: @[Int] = Map[xs, _ x: Int = Div[x, 3]().unmold()]() => :@[Int]
applyLax xs: @[Int] = Map[xs, _ x: Int = Lax[x * 10]().unmold()]() => :@[Int]
applyVar xs: @[Int] d: Int = Map[xs, _ x: Int = Mod[x, d]().unmold()]() => :@[Int]

nums <= @[7, -7, 12, -12, 0, 5]
stdout(applyMod(nums))
stdout(applyDiv(nums))
stdout(applyLax(nums))
stdout(applyVar(nums, 4))
stdout(Mod[17, 5]().unmold())
stdout(Div[-17, 5]().unmold())
"#,
    );
    assert_eq!(
        out,
        "@[3, -3, 0, 0, 0, 1]\n@[2, -2, 4, -4, 0, 1]\n@[70, -70, 120, -120, 0, 50]\n@[3, -3, 0, 0, 0, 1]\n2\n-3"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Float and Bool list display/join: the containers record element
/// kinds, but the native/wasm display paths used to render through
/// tag-blind value heuristics — a Float element printed as its raw f64
/// bit pattern and a Bool as 0/1. Pins the kind-aware rendering plus
/// the borrowed-string join (string elements joined without per-element
/// materialisation) against the interpreter.
#[test]
fn list_display_and_join_render_kinds() {
    let dir = unique_temp_dir("f60_kind_display");
    let out = assert_parity(
        &dir,
        "kinds",
        r#"fl <= @[1.5, 2.5]
stdout(fl)
bl <= @[true, false]
stdout(bl)
Join[fl, ";"]() >=> jf
stdout(jf)
Join[bl, ","]() >=> jb
stdout(jb)
sl <= @["a", "bb"]
stdout(sl)
Join[sl, "-"]() >=> js
stdout(js)
nested <= @[@[1.5], @[2.5]]
stdout(nested)
Join[@[1, 2, 3], ""]() >=> jn
stdout(jn)
"#,
    );
    assert_eq!(
        out,
        "@[1.5, 2.5]\n@[true, false]\n1.5;2.5\ntrue,false\n@[\"a\", \"bb\"]\na-bb\n@[@[1.5], @[2.5]]\n123"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Appending into an empty (kindless) list must establish the element
/// kind: `Append[@[], 1.5]()` and the tail-recursive `build(n, @[])`
/// pattern used to leave the result list untagged, so Float/Bool
/// elements displayed as raw bits even after the renderer learned to
/// read kinds. Also pins the full-form pack rendering of a nested
/// Float list (a separate display path from the plain list renderer).
#[test]
fn append_establishes_element_kind() {
    let dir = unique_temp_dir("f60_append_kind");
    let out = assert_parity(
        &dir,
        "append_kind",
        r#"a <= Append[@[], 1.5]()
stdout(a)
build n: Int acc: @[Float] =
  | n == 0 |> acc
  | _ |> build(n - 1, Append[acc, 1.5]())
=> :@[Float]
fl <= build(3, @[])
stdout(fl)
Join[fl, ";"]() >=> j
stdout(j)
bb <= Append[@[], true]()
stdout(bb)
p <= @(xs <= @[1.5, 2.5], ok <= true)
stdout(p)
"#,
    );
    assert_eq!(
        out,
        "@[1.5]\n@[1.5, 1.5, 1.5]\n1.5;1.5;1.5\n@[true]\n@(xs <= @[1.5, 2.5], ok <= true)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Large sequential builds complete on wasm (the O(n^2) copies used to
/// exhaust the 2GB boxed-value address space on the second call) and
/// agree across backends.
#[test]
fn large_sequential_build_completes_everywhere() {
    let dir = unique_temp_dir("f60_consume_large");
    let out = assert_parity(
        &dir,
        "large",
        r#"build n: Int acc: @[Int] =
  | n == 0 |> acc
  | _ |> build(n - 1, Append[acc, n]())
=> :@[Int]

p u: Int =
  nums <= build(10000, @[])
  nums.length()
=> :Int

stdout(p(1) + p(2))
"#,
    );
    assert_eq!(out, "20000");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Negative Float literals in pack fields: the F64 return of a runtime
/// call (taida_float_neg) used to escape into the val_map unboxed and
/// reach taida_pack_set's i64 slot — a Cranelift verifier error that
/// made `@(z <= -0.5)` fail to BUILD on native.
#[test]
fn negative_float_pack_field_builds_and_runs() {
    let dir = unique_temp_dir("f61_neg_float_pack");
    let out = assert_parity(
        &dir,
        "neg_pack",
        r#"p <= @(z <= -0.5)
stdout(p)
nested <= @(inner <= @(w <= -1.5))
stdout(nested)
"#,
    );
    assert_eq!(out, "@(z <= -0.5)\n@(inner <= @(w <= -1.5))");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sum over Float lists: the payload of a Float element is its f64 bit
/// pattern, which the raw i64 accumulation added as garbage integers.
/// Pins the f64 switch (literal, tracked variable, parameter) and the
/// exact i64 path for Int-only lists.
#[test]
fn sum_over_float_lists_accumulates_as_f64() {
    let dir = unique_temp_dir("f61_sum_float");
    let out = assert_parity(
        &dir,
        "sum_float",
        r#"stdout(Sum[@[1.0, 2.0]]().toString())
fl <= @[2.5, 1.5, 0.5]
stdout(Sum[fl]().toString())
ints <= @[1, 2, 3]
stdout(Sum[ints]().toString())
sumIt xs: @[Float] = Sum[xs]() => :Float
stdout(sumIt(@[0.5, 0.25]).toString())
"#,
    );
    assert_eq!(out, "3.0\n4.5\n6\n0.75");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sort[] and list membership compare by VALUE: raw i64 comparison
/// made Sort[] on a Str list a pointer-order no-op, inverted the order
/// of negative Floats, and contains/indexOfLax never matched a computed
/// string or an equal pack/nested list.
#[test]
fn sort_and_membership_compare_by_value() {
    let dir = unique_temp_dir("f61_sort_membership");
    let out = assert_parity(
        &dir,
        "sort_member",
        r#"w <= @["cherry", "apple", "banana"]
stdout(Sort[w]())
stdout(Sort[@["b", "c", "a"]](reverse <= true))
nf <= @[1.5, -0.5, 0.5, -1.5]
stdout(Sort[nf]())
stdout(Sort[nf](reverse <= true))
needle <= "a" + "a"
ws <= @["aa", "bb"]
stdout(ws.contains(needle))
ws.indexOfLax("bb").getOrDefault(-1) >=> i1
stdout(i1)
ps <= @[@(a <= 1), @(a <= 2)]
stdout(ps.contains(@(a <= 2)))
ls <= @[@[1, 2], @[3]]
ls.indexOfLax(@[1, 2]).getOrDefault(-1) >=> i2
stdout(i2)
stdout(@["x"].lastIndexOf("x"))
"#,
    );
    assert_eq!(
        out,
        "@[\"apple\", \"banana\", \"cherry\"]\n@[\"c\", \"b\", \"a\"]\n@[-1.5, -0.5, 0.5, 1.5]\n@[1.5, 0.5, -0.5, -1.5]\ntrue\n1\ntrue\n0\n0"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

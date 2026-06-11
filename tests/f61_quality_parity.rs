/// Cross-backend pins for the quality-hardening track: checker type
/// hints for HOF-mold callbacks, and the Min/Max mold family.
mod common;

use common::{node_available, run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
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

/// Unannotated lambdas in HOF-mold positions infer their parameter
/// types from the list (the same expected-type hint method-position
/// lambdas get); Map's checker return type follows the callback (a
/// type-changing map used to read back as the INPUT list type and fail
/// downstream); Find returns Lax of the element type (it was a
/// hardcoded Lax[Unknown] that made every bare `>=>` unresolvable).
#[test]
fn mold_callbacks_infer_without_annotations() {
    let dir = unique_temp_dir("f61_mold_hints");
    let out = assert_parity(
        &dir,
        "hints",
        r#"nums <= @[1, 2, 3]
Map[nums, _ x = x * 2]() >=> doubled
stdout(doubled)
Map[nums, _ x = x.toString()]() >=> strs
stdout(strs)
Filter[doubled, _ x = x > 2]() >=> big
stdout(big)
Find[@[1, 5, 3], _ x = x > 2]() >=> found
stdout(found)
Fold[nums, 0, _ acc x = acc + x]() >=> total
stdout(total)
useStrs xs: @[Int] = Map[xs, _ x = x.toString()]() => :@[Str]
stdout(useStrs(nums))
stdout(Count[nums, _ x = x > 1]())
"#,
    );
    assert_eq!(
        out,
        "@[2, 4, 6]\n@[\"1\", \"2\", \"3\"]\n@[4, 6]\n5\n6\n@[\"1\", \"2\", \"3\"]\n2"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A lambda with no context to take a type from must still be
/// rejected: annotations are "write them when the context cannot
/// supply the type", not never.
#[test]
fn contextless_ambiguous_lambda_still_rejected() {
    let dir = unique_temp_dir("f61_lambda_neg");
    let td = dir.join("ambiguous.td");
    std::fs::write(&td, "f <= _ x y = x + y\nstdout(\"no\")\n").expect("write fixture");
    let out = Command::new(taida_bin())
        .arg(&td)
        .output()
        .expect("taida runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("[E1527]"),
        "ambiguous lambda must keep E1527, got: {combined}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Min[]/Max[] molds: registered in the spec table but unimplemented on
/// every backend (the interpreter leaked the raw MoldInst pack, JS
/// crashed on ReferenceError, native/wasm failed to lower). Pins the
/// mold form, the kind-aware ordering (Float / Str), the empty-list
/// Lax, and the method twins' element-kind display.
#[test]
fn min_max_molds_work_on_all_backends() {
    let dir = unique_temp_dir("f61_min_max");
    let out = assert_parity(
        &dir,
        "min_max",
        r#"mn <= Min[@[3, 1, 2]]()
stdout(mn.getOrDefault(0))
mx <= Max[@[3, 1, 2]]()
stdout(mx.getOrDefault(0))
Min[@[1.5, 2.5, 0.5]]() >=> fmin
stdout(fmin)
Max[@[-0.5, -1.5]]() >=> fmax
stdout(fmax)
Min[@["b", "a", "c"]]() >=> smin
stdout(smin)
e: Lax[Int] <= Min[@[]]()
stdout(e.hasValue())
fl <= @[2.5, 0.5]
fl.min() >=> mmin
stdout(mmin)
mixed <= @[3, 1, 2]
stdout(Max[mixed]().getOrDefault(0))
"#,
    );
    assert_eq!(out, "1\n3\n0.5\n-0.5\na\nfalse\n0.5\n3");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Float kind through read paths: pack-field reads, getOrDefault with
/// a Float default, If/Abs mold results — each used to display as raw
/// f64 bits on the compiled backends (the value heuristics cannot
/// identify a Float payload; the static classifiers now carry it).
#[test]
fn float_kind_flows_through_read_paths() {
    let dir = unique_temp_dir("f61_float_reads");
    let out = assert_parity(
        &dir,
        "float_reads",
        r#"p <= @(x <= 1.5)
stdout(p.x.toString())
m <= hashMap().set("a", 0.5)
m.get("a").getOrDefault(0.0) >=> g
stdout(g)
If[true, 1.5, 2.5]() >=> iv
stdout(iv)
Abs[-1.5]() >=> av
stdout(av)
"#,
    );
    assert_eq!(out, "1.5\n0.5\n1.5\n1.5");
    let _ = std::fs::remove_dir_all(&dir);
}

/// jsonEncode renders Float fields as numbers, not their raw f64 bit
/// patterns ({"score":4611686018427387904} corrupted every serialised
/// artifact downstream). The per-slot pack tags carry the kind; the
/// serialiser reads them for plain packs too, and the Float hint
/// formats through the shared Rust-Display-compatible path.
#[test]
fn json_encode_renders_float_fields() {
    let dir = unique_temp_dir("f61_json_float");
    let out = assert_parity(
        &dir,
        "json_float",
        r#"whole <= @(score <= 2.0, ok <= false, n <= 7, s <= "hi")
stdout(jsonEncode(whole))
nested <= @(inner <= @(rate <= -0.5))
stdout(jsonEncode(nested))
"#,
    );
    assert_eq!(
        out,
        "{\"n\":7,\"ok\":false,\"s\":\"hi\",\"score\":2.0}\n{\"inner\":{\"rate\":-0.5}}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// HashMap/Set displays agree on every backend: native printed raw f64
/// bits in toString and Set, wasm lost the tag in values(), and the
/// interpreter leaked the internal carrier pack
/// (`@(__entries <= ..., __type <= "HashMap")`) from stdout / template
/// interpolation even though the `__` namespace is declared
/// compiler-internal and `.toString()` already printed the public
/// shape.
#[test]
fn hashmap_set_display_renders_floats_on_compiled_backends() {
    let dir = unique_temp_dir("f61_hm_set_display");
    let out = assert_parity(
        &dir,
        "hm",
        r#"m <= hashMap().set("a", 1.5).set("b", 2.5)
stdout(m.values())
stdout(m)
s <= setOf(@[1.5, 2.5])
stdout(s)
stdout(s.toList())
"#,
    );
    assert_eq!(
        out,
        "@[1.5, 2.5]\nHashMap({\"a\": 1.5, \"b\": 2.5})\nSet({1.5, 2.5})\n@[1.5, 2.5]"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Extreme Float magnitudes format identically on interp/native/wasm:
/// wasm printed 1e20 as "0" (the u64 cast is UB past 2^64), native
/// chose %g's scientific notation ("1e+20" / "1e-07") where the
/// reference expands every finite f64 in fixed notation.
#[test]
fn extreme_float_magnitudes_format_consistently() {
    let dir = unique_temp_dir("f61_float_extreme");
    let out = assert_parity(
        &dir,
        "extremes",
        r#"big <= 100000000000000000000.5
stdout(big.toString())
huge <= 123456789012345678901234567890.0
stdout(huge.toString())
tiny <= 0.0000001
stdout(tiny.toString())
neg <= -100000000000000000000.5
stdout(neg.toString())
"#,
    );
    assert_eq!(
        out,
        "100000000000000000000.0\n123456789012345677877719597056.0\n0.0000001\n-100000000000000000000.0"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The interpreter and JS backends now share the native backend's
/// consumable-Append analysis (the tail-recursive accumulator build).
/// Pins the three value-semantics hazards of an in-place push — the
/// caller's binding surviving detach, an alias of that binding, and
/// re-feeding a consume-produced list into the same build — plus the
/// O(n) wall-clock shape: 100k elements on the interpreter completes
/// orders of magnitude inside the old O(n²) time (which needed seconds
/// for 5k).
#[test]
fn append_consume_keeps_value_semantics_on_interp_and_js() {
    let dir = unique_temp_dir("f61_consume");
    let td = dir.join("consume.td");
    std::fs::write(
        &td,
        r#"build acc: @[Int] i: Int =
  | i >= 3 |> acc
  | _ |> build(Append[acc, i](), i + 1)
=> :@[Int]
xs <= @[100]
build(xs, 0) >=> r1
stdout(xs)
stdout(r1)
build(r1, 0) >=> r2
stdout(r1)
stdout(r2)
ys <= xs
build(xs, 1) >=> r3
stdout(ys)
stdout(r3)
"#,
    )
    .expect("write fixture");
    let expected =
        "@[100]\n@[100, 0, 1, 2]\n@[100, 0, 1, 2]\n@[100, 0, 1, 2, 0, 1, 2]\n@[100]\n@[100, 1, 2]";
    let interp = run_interpreter(&td).expect("interpreter runs");
    assert_eq!(interp, expected, "interp consume value semantics");

    if node_available() {
        let mjs = dir.join("consume.mjs");
        let status = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&td)
            .arg("-o")
            .arg(&mjs)
            .status()
            .expect("taida build js runs");
        assert!(status.success(), "js build failed");
        let out = Command::new("node").arg(&mjs).output().expect("node runs");
        assert!(out.status.success(), "js run failed");
        let js = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
        assert_eq!(js, expected, "js consume value semantics");
    } else {
        eprintln!("SKIP: node not found, js leg skipped");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 100k-element sequential build on the interpreter: O(n²) cloning
/// needed ~2s for 5k elements, so 100k would take minutes; the consume
/// path finishes in well under the suite timeout. No wall-clock assert
/// — completing at all is the pin.
#[test]
fn append_consume_interp_100k_completes() {
    let dir = unique_temp_dir("f61_consume_perf");
    let td = dir.join("build100k.td");
    std::fs::write(
        &td,
        r#"build acc: @[Int] i: Int =
  | i >= 100000 |> acc
  | _ |> build(Append[acc, i](), i + 1)
=> :@[Int]
build(@[], 0) >=> result
stdout(result.length())
"#,
    )
    .expect("write fixture");
    let interp = run_interpreter(&td).expect("interpreter runs");
    assert_eq!(interp, "100000");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Float-bearing Set semantics across all four backends: ±0.0 collapses
/// to one element (native used to dedup homogeneous Float sets by raw
/// bit pattern and report 2), the Int↔Float crossing holds through
/// membership, and union/intersect/diff agree. The same fixture pins
/// the JS runtime (SameValueZero already collapses ±0.0 there).
#[test]
fn float_set_semantics_agree_on_all_backends() {
    let dir = unique_temp_dir("f61_float_set");
    let src = r#"a <= setOf(@[0.0, -0.0])
stdout(a.size())
b <= setOf(@[1.0, 2.0])
c <= setOf(@[1, 2])
stdout(b.has(1))
stdout(c.has(1.0))
stdout(Unique[@[1.5, 2.5, 1.5]]())
u <= setOf(@[1.5, 2.5]).union(setOf(@[2.5, 3.5]))
stdout(u.size())
x <= setOf(@[1.5, 2.5]).intersect(setOf(@[2.5, 3.5]))
stdout(x.size())
d <= setOf(@[1.5, 2.5]).diff(setOf(@[2.5, 3.5]))
stdout(d.size())
"#;
    let expected = "1\ntrue\ntrue\n@[1.5, 2.5]\n3\n1\n1";
    let out = assert_parity(&dir, "float_set", src);
    assert_eq!(out, expected);
    if node_available() {
        let td = dir.join("float_set.td");
        let mjs = dir.join("float_set.mjs");
        let status = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&td)
            .arg("-o")
            .arg(&mjs)
            .status()
            .expect("taida build js runs");
        assert!(status.success(), "js build failed");
        let jsout = Command::new("node").arg(&mjs).output().expect("node runs");
        assert!(jsout.status.success(), "js run failed");
        assert_eq!(
            String::from_utf8_lossy(&jsout.stdout).trim_end(),
            expected,
            "js float set semantics"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Float Set operations leave the O(n²) linear fallback: 16k-element
/// union/intersect/diff complete in milliseconds on native (the old
/// tagged linear scan needed ~1.2s and doubled 3.8× per size doubling).
/// The accumulator build uses the consume path with the Append in the
/// FIRST argument slot — the IR analysis now tolerates the scalar-pure
/// evaluation of later tail-call arguments between the append and the
/// hand-off, which that argument order produces.
#[test]
fn float_set_ops_scale_linearly_on_native() {
    use std::time::{Duration, Instant};
    let dir = unique_temp_dir("f61_float_set_perf");
    let td = dir.join("set_perf.td");
    std::fs::write(
        &td,
        r#"mk acc: @[Float] x: Float n: Float =
  | x >= n |> acc
  | _ |> mk(Append[acc, x](), x + 1.0, n)
=> :@[Float]
a <= setOf(mk(@[], 0.5, 16000.5))
b <= setOf(mk(@[], 8000.5, 24000.5))
stdout(a.union(b).size())
stdout(a.intersect(b).size())
stdout(a.diff(b).size())
"#,
    )
    .expect("write fixture");
    let bin = dir.join("set_perf_native");
    let status = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(&td)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("taida build native runs");
    assert!(status.success(), "native build failed");
    let started = Instant::now();
    let out = Command::new(&bin).output().expect("native binary runs");
    let elapsed = started.elapsed();
    assert!(out.status.success(), "native run failed");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        "24000\n8000\n8000"
    );
    // O(n) finishes in ~10ms; the old fallback needed ~1.2s at this
    // size. 10s leaves two orders of magnitude for CPU-saturated CI.
    assert!(
        elapsed < Duration::from_secs(10),
        "Float set ops took {elapsed:?} — linear fallback regression?"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// String-conversion parses agree with the reference on every backend:
/// `Float["NaN"/"Infinity"]` succeeds (Rust f64::from_str semantics —
/// js rejected NaN, wasm rejected both), out-of-range `Int[...]` fails
/// (native clamped via strtol, js wrapped through BigInt.asIntN), and
/// the IEEE specials display as `NaN` / `inf` / `-inf` (js used
/// `Infinity`; the Float kind now flows through getOrDefault into the
/// display dispatch on the compiled backends too).
#[test]
fn conversion_parse_acceptance_matches_reference() {
    let dir = unique_temp_dir("f61_conv_parse");
    let src = r#"a <= Float["NaN"]()
stdout(a.hasValue())
b <= Float["Infinity"]()
stdout(b.hasValue())
c <= Int["9999999999999999999999"]()
stdout(c.hasValue())
n <= Float["NaN"]().getOrDefault(0.0)
stdout(n)
i <= Float["-inf"]().getOrDefault(0.0)
stdout(i)
w <= Float[" 1.5"]()
stdout(w.hasValue())
"#;
    let expected = "true\ntrue\nfalse\nNaN\n-inf\nfalse";
    let out = assert_parity(&dir, "conv_parse", src);
    assert_eq!(out, expected);
    if node_available() {
        let td = dir.join("conv_parse.td");
        let mjs = dir.join("conv_parse.mjs");
        let status = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&td)
            .arg("-o")
            .arg(&mjs)
            .status()
            .expect("taida build js runs");
        assert!(status.success(), "js build failed");
        let jsout = Command::new("node").arg(&mjs).output().expect("node runs");
        assert!(jsout.status.success(), "js run failed");
        assert_eq!(
            String::from_utf8_lossy(&jsout.stdout).trim_end(),
            expected,
            "js conversion parses"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An uncaught throw reports identically on every backend: one
/// `Runtime error: Unhandled error: ...` line on stderr and exit code 1
/// (js dumped a raw Node stack + object internals, wasm wrote a generic
/// note to stdout and trapped with exit 134; packs render as
/// `Error[type]: message`).
#[test]
fn unhandled_throw_reports_identically_everywhere() {
    let dir = unique_temp_dir("f61_throw_report");
    let td = dir.join("boom.td");
    std::fs::write(
        &td,
        "Error => MyErr = @(message: Str)\nthrow(MyErr(message <= \"oops\"))\n",
    )
    .expect("write fixture");
    let expected_line = "Runtime error: Unhandled error: Error[MyErr]: oops";

    let interp = Command::new(taida_bin())
        .arg(&td)
        .output()
        .expect("interp runs");
    assert_eq!(interp.status.code(), Some(1), "interp exit code");
    assert!(
        String::from_utf8_lossy(&interp.stderr).contains(expected_line),
        "interp report: {}",
        String::from_utf8_lossy(&interp.stderr)
    );

    let bin = dir.join("boom_native");
    let status = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(&td)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("native build");
    assert!(status.success());
    let native = Command::new(&bin).output().expect("native runs");
    assert_eq!(native.status.code(), Some(1), "native exit code");
    assert!(
        String::from_utf8_lossy(&native.stderr).contains(expected_line),
        "native report: {}",
        String::from_utf8_lossy(&native.stderr)
    );

    if node_available() {
        let mjs = dir.join("boom.mjs");
        let status = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&td)
            .arg("-o")
            .arg(&mjs)
            .status()
            .expect("js build");
        assert!(status.success());
        let js = Command::new("node").arg(&mjs).output().expect("node runs");
        assert_eq!(js.status.code(), Some(1), "js exit code");
        let js_err = String::from_utf8_lossy(&js.stderr);
        assert!(js_err.contains(expected_line), "js report: {js_err}");
        assert!(
            !js_err.contains("at __taida_throw"),
            "js must not dump a raw stack: {js_err}"
        );
    }

    if let Some(wasmtime) = wasmtime_bin() {
        let wasm = dir.join("boom.wasm");
        let status = Command::new(taida_bin())
            .args(["build", "wasm-min"])
            .arg(&td)
            .arg("-o")
            .arg(&wasm)
            .status()
            .expect("wasm build");
        assert!(status.success());
        let out = Command::new(&wasmtime)
            .arg(&wasm)
            .output()
            .expect("wasmtime runs");
        assert_eq!(out.status.code(), Some(1), "wasm exit code");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(expected_line),
            "wasm report: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Definition order is the language semantics: a top-level statement
/// that calls a function defined later in the file is rejected at check
/// time ([E1539]) — the interpreter would fail at runtime while the
/// compiled backends hoist and silently succeed, making program success
/// backend-dependent. Mutual recursion (forward references from inside
/// function bodies) stays legal.
#[test]
fn toplevel_forward_function_reference_is_rejected() {
    let dir = unique_temp_dir("f61_forward_ref");
    let bad = dir.join("forward.td");
    std::fs::write(&bad, "stdout(double(21))\ndouble x: Int = x * 2 => :Int\n")
        .expect("write fixture");
    let out = Command::new(taida_bin())
        .arg(&bad)
        .output()
        .expect("taida runs");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("[E1539]"),
        "forward call must be E1539, got: {combined}"
    );

    let ok = dir.join("mutual.td");
    std::fs::write(
        &ok,
        "f x: Int = g(x) => :Int\ng x: Int = x + 1 => :Int\nstdout(f(1))\n",
    )
    .expect("write fixture");
    let out = assert_parity(
        &dir,
        "mutual",
        "f x: Int = g(x) => :Int\ng x: Int = x + 1 => :Int\nstdout(f(1))\n",
    );
    assert_eq!(out, "2");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The string unit is the Unicode code point on every backend:
/// length / get / CharAt / Slice / Reverse used to count bytes on
/// native+wasm and UTF-16 units in js ("héllo日本".length() was 12 / 7
/// / 12; a byte Reverse shredded every multibyte sequence).
#[test]
fn string_apis_use_code_point_units_everywhere() {
    let dir = unique_temp_dir("f61_codepoints");
    let src = r#"s <= "héllo日本"
stdout(s.length())
stdout(s.get(1).getOrDefault(""))
stdout(Slice[s, 1, 3]())
stdout(CharAt[s, 5]().getOrDefault(""))
stdout(Reverse[s]())
e <= "a🎈b"
stdout(e.length())
stdout(e.get(1).getOrDefault(""))
stdout(Reverse[e]())
stdout(s.indexOf("日"))
stdout(s.lastIndexOf("l"))
"#;
    let expected = "7\né\nél\n日\n本日olléh\n3\n🎈\nb🎈a\n5\n3";
    let out = assert_parity(&dir, "codepoints", src);
    assert_eq!(out, expected);
    if node_available() {
        let td = dir.join("codepoints.td");
        let mjs = dir.join("codepoints.mjs");
        let status = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&td)
            .arg("-o")
            .arg(&mjs)
            .status()
            .expect("taida build js runs");
        assert!(status.success());
        let jsout = Command::new("node").arg(&mjs).output().expect("node runs");
        assert!(jsout.status.success());
        assert_eq!(
            String::from_utf8_lossy(&jsout.stdout).trim_end(),
            expected,
            "js code point units"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `Str[container]()` renders the public HashMap({...}) / Set({...})
/// shape on every backend (native/js rebuilt the internal carrier-pack
/// text), and a Lax bound to a variable prints as the Lax pack — the
/// string classifier no longer mistakes `Str[x]()`'s Lax[Str] for a
/// raw string (which printed the pack pointer's magic header as text).
#[test]
fn str_conversion_and_lax_binding_display_agree() {
    let dir = unique_temp_dir("f61_str_conv");
    let src = r#"v <= Str[setOf(@[1, 2, 3])]()
stdout(v.getOrDefault(""))
w <= Str[42]()
stdout(w)
m <= Str[hashMap().set("a", 1)]()
stdout(m.getOrDefault(""))
"#;
    let expected = "Set({1, 2, 3})\n@(has_value <= true, __value <= \"42\", __default <= \"\", __type <= \"Lax\")\nHashMap({\"a\": 1})";
    let out = assert_parity(&dir, "str_conv", src);
    assert_eq!(out, expected);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Slice agrees on every backend for every target type: the
/// interpreter gained the List arm (it used to leak the raw MoldInst
/// pack), JS sliced lists to '' and treated negative ends as
/// tail-relative, native/wasm treated them as "to the end". The
/// reference semantics: an omitted end means the end, an explicit
/// negative end clamps to 0 (an empty slice).
#[test]
fn slice_agrees_for_all_target_types_and_bounds() {
    let dir = unique_temp_dir("f61_slice");
    let src = r#"stdout(Slice["hello", 1, -1]())
stdout(Slice["hello", 1]())
stdout(Slice[@[1, 2, 3, 4], 1, 3]())
stdout(Slice[@[1, 2, 3, 4], 1]())
stdout(Slice[@[1, 2, 3, 4], 1, -1]())
"#;
    let expected = "\nello\n@[2, 3]\n@[2, 3, 4]\n@[]";
    let out = assert_parity(&dir, "slice", src);
    assert_eq!(out, expected);
    if node_available() {
        let td = dir.join("slice.td");
        let mjs = dir.join("slice.mjs");
        let status = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&td)
            .arg("-o")
            .arg(&mjs)
            .status()
            .expect("taida build js runs");
        assert!(status.success());
        let jsout = Command::new("node").arg(&mjs).output().expect("node runs");
        assert!(jsout.status.success());
        assert_eq!(
            String::from_utf8_lossy(&jsout.stdout)
                .strip_suffix('\n')
                .unwrap_or_default(),
            expected,
            "js slice semantics"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An undefined JSON schema is rejected at check time ([E1541]) — it
/// used to slip through to a runtime error even though the JSON guide
/// promises compile-time rejection. Undefined variables now carry
/// their own code ([E1542]) instead of overloading the deprecated
/// partial-application diagnostic.
#[test]
fn json_schema_and_undefined_variable_diagnostics() {
    let dir = unique_temp_dir("f61_diag");
    let td = dir.join("schema.td");
    std::fs::write(
        &td,
        "raw <= \"{}\"\nJSON[raw, NotDefined]() >=> v\nstdout(v)\n",
    )
    .expect("write fixture");
    let out = Command::new(taida_bin())
        .arg(&td)
        .output()
        .expect("taida runs");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("[E1541]"), "schema: {combined}");

    let td2 = dir.join("undef.td");
    std::fs::write(&td2, "x <= notDefinedAnywhere\nstdout(x)\n").expect("write fixture");
    let out2 = Command::new(taida_bin())
        .arg(&td2)
        .output()
        .expect("taida runs");
    let combined2 = format!(
        "{}{}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(combined2.contains("[E1542]"), "undefined: {combined2}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The kind-aware numeric fingerprint lives in the f64 domain the
/// equality widens to: past 2^53, Int(2^60 + 1) and Float(2^60) are
/// equal after widening, and the hash path must not split what the
/// equality merges (Set/Unique used to report 2 elements on the
/// compiled backends where the interpreter reports 1).
#[test]
fn numeric_fingerprint_matches_equality_past_2_53() {
    let dir = unique_temp_dir("f61_fp53");
    let src = r#"bigf <= 1152921504606846976.0
u <= setOf(@[bigf]).union(setOf(@[1152921504606846977]))
stdout(u.size())
"#;
    let out = assert_parity(&dir, "fp53", src);
    assert_eq!(out, "1");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Order-sensitive checks see template interpolation bodies and
/// error-ceiling handlers: `${later(1)}` is parsed and evaluated by
/// the interpreter like any expression (forward references failed at
/// runtime there and silently succeeded on the hoisting backends),
/// and a ceiling handler runs when a later statement throws, where
/// only the functions defined up to the throw exist.
#[test]
fn template_and_ceiling_forward_references_are_rejected() {
    let dir = unique_temp_dir("f61_fwd_edges");
    let t1 = dir.join("template.td");
    std::fs::write(&t1, "stdout(`${later(1)}`)\nlater x: Int = x * 2 => :Int\n")
        .expect("write fixture");
    let o1 = Command::new(taida_bin())
        .arg(&t1)
        .output()
        .expect("taida runs");
    let c1 = format!(
        "{}{}",
        String::from_utf8_lossy(&o1.stdout),
        String::from_utf8_lossy(&o1.stderr)
    );
    assert!(c1.contains("[E1539]"), "template: {c1}");

    let t2 = dir.join("ceiling.td");
    std::fs::write(
        &t2,
        "|== error: Error =\n  recover(1)\n=> :Int\nthrow(\"x\")\nrecover x: Int = x => :Int\n",
    )
    .expect("write fixture");
    let o2 = Command::new(taida_bin())
        .arg(&t2)
        .output()
        .expect("taida runs");
    let c2 = format!(
        "{}{}",
        String::from_utf8_lossy(&o2.stdout),
        String::from_utf8_lossy(&o2.stderr)
    );
    assert!(c2.contains("[E1539]"), "ceiling: {c2}");

    let t3 = dir.join("template_undef.td");
    std::fs::write(&t3, "s <= `v=${missing}`\nstdout(s)\n").expect("write fixture");
    let o3 = Command::new(taida_bin())
        .arg(&t3)
        .output()
        .expect("taida runs");
    let c3 = format!(
        "{}{}",
        String::from_utf8_lossy(&o3.stdout),
        String::from_utf8_lossy(&o3.stderr)
    );
    assert!(c3.contains("[E1542]"), "template undefined: {c3}");
    let _ = std::fs::remove_dir_all(&dir);
}

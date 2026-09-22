//! Throw propagation, repeated await, scope isolation and exact arithmetic.
mod common;

use common::{run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::Command;

/// Run the interpreter with extra CLI flags, returning stdout, stderr,
/// and the exit status.
fn run_interp_with(td: &Path, flags: &[&str]) -> (String, String, Option<i32>) {
    let output = Command::new(taida_bin())
        .args(flags)
        .arg(td)
        .output()
        .expect("interpreter runs");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code(),
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

fn assert_parity(dir: &Path, stem: &str, source: &str) -> String {
    let td = dir.join(format!("{stem}.td"));
    std::fs::write(&td, source).expect("write fixture");
    let interp = run_interpreter(&td).unwrap_or_else(|| panic!("{stem}: interpreter runs"));
    let native = build_and_run_native(&td, dir, stem);
    assert_eq!(interp.trim_end(), native, "{stem}: interp vs native");
    if let Some(wasm) = build_and_run_wasm(&td, dir, stem) {
        assert_eq!(interp.trim_end(), wasm, "{stem}: interp vs wasm-min");
    } else {
        eprintln!("SKIP: wasmtime not found, wasm leg skipped for {stem}");
    }
    interp
}

// ── a Taida throw inside `${...}` template interpolation must
// reach the enclosing error ceiling instead of being silently dropped. ──
#[test]
fn f64b_019_template_interpolation_throw_reaches_ceiling() {
    let dir = unique_temp_dir("f64p3_t019");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"boom n: Int =
  Error(type <= "Boom", message <= "no zero")
=> :Error
f n: Int =
  |== e: Error =
    stdout("caught outside")
    "caught"
  => :Str
  | n == 0 |> boom(0).throw()
  | _ |> "no"
=> :Str
s <= `v=${f(0)}`
stdout(s)
"#,
    )
    .expect("write fixture");
    let out = run_interpreter(&td).expect("interpreter runs");
    assert_eq!(out.trim_end(), "caught outside\nv=caught");
}

// ── a Taida throw inside a mold option argument stays
// catchable by the caller's error ceiling. ──
#[test]
fn f64b_020_mold_option_throw_catchable() {
    let dir = unique_temp_dir("f64p3_t020");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"boom n: Int =
  Error(type <= "OptBoom", message <= "no zero").throw()
=> :Int
mk s: Str =
  |== e: Error =
    stdout("caught: " + e.message)
    "fallback"
  => :Str
  | s == "go" |> Slice["Hello World"](start <= boom(0))
  | _ |> "no"
=> :Str
stdout(mk("go"))
"#,
    )
    .expect("write fixture");
    let out = run_interpreter(&td).expect("interpreter runs");
    assert_eq!(out.trim_end(), "caught: no zero\nfallback");
}

// ── undeclared named fields keep source order (Vec, not
// HashMap), so pack display is deterministic. Requires --no-check
// because undeclared options are rejected with [E1406] under the
// checker. ──
#[test]
fn f64b_021_named_fields_keep_source_order() {
    let dir = unique_temp_dir("f64p3_t021");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"Mold[T] => Box[T] = @(
  value: T
)
b <= Box[7](alpha <= 1, zeta <= 2, mid <= 3)
stdout(b)
"#,
    )
    .expect("write fixture");
    // Run twice: identical output proves determinism; the source-order
    // sequence proves the Vec insertion path.
    let mut first: Option<String> = None;
    for _ in 0..2 {
        let (out, err, code) = run_interp_with(&td, &["--no-check"]);
        assert_eq!(code, Some(0), "interp --no-check failed: {err}");
        let text = out.trim_end().to_string();
        assert!(
            text.contains("alpha <= 1, zeta <= 2, mid <= 3"),
            "source order lost: {text}"
        );
        if let Some(prev) = &first {
            assert_eq!(&text, prev, "pack display is nondeterministic");
        }
        first = Some(text);
    }
}

// ── after a ceiling consumes a callback throw, the next
// ceiling must see its own error, never the stale one. ──
#[test]
fn f64b_022_pending_throw_not_stale_across_ceilings() {
    let dir = unique_temp_dir("f64p3_t022");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"boom n: Int =
  Error(type <= "Boom", message <= "first").throw()
=> :Int
boom2 n: Int =
  Error(type <= "Boom", message <= "second").throw()
=> :Int
a s: Str =
  |== e: Error =
    stdout("A:" + e.message)
    "ra"
  => :Str
  | s == "go" |> Slice["hello"](start <= boom(0))
  | _ |> "no"
=> :Str
b s: Str =
  |== e: Error =
    stdout("B:" + e.message)
    "rb"
  => :Str
  | s == "go" |> Slice["world"](start <= boom2(0))
  | _ |> "no"
=> :Str
stdout(a("go"))
stdout(b("go"))
"#,
    )
    .expect("write fixture");
    let out = run_interpreter(&td).expect("interpreter runs");
    assert_eq!(out.trim_end(), "A:first\nra\nB:second\nrb");
}

// ── awaiting the same pending Async twice replays the
// resolved value instead of handing back a placeholder. ──
#[test]
fn f64b_023_double_await_replays_resolved_value() {
    let dir = unique_temp_dir("f64p3_t023");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"t <= sleep(10)
t >=> v1
u <= t
u >=> v2
stdout("first: " + v1.toString())
stdout("second: " + v2.toString())
t >=> v3
t >=> v4
stdout("third: " + v3.toString())
stdout("fourth: " + v4.toString())
"#,
    )
    .expect("write fixture");
    let out = run_interpreter(&td).expect("interpreter runs");
    assert!(out.contains("first: 10"), "got: {out}");
    assert!(out.contains("second: 10"), "replayed await lost: {out}");
    assert!(out.contains("third: 10"), "third await lost: {out}");
    assert!(out.contains("fourth: 10"), "fourth await lost: {out}");
}

// ── redefining an Enum in the same scope fails loudly at the
// runtime define step too (--no-check reaches the runtime path). ──
#[test]
fn f64b_025_enum_redefinition_rejected_at_runtime() {
    let dir = unique_temp_dir("f64p3_t025");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"Enum => Color = :Red :Green
Enum => Color = :Blue
"#,
    )
    .expect("write fixture");
    let (_out, err, _code) = run_interp_with(&td, &["--no-check"]);
    assert!(
        err.contains("already defined in this scope"),
        "runtime redefinition not rejected: {err}"
    );
}

// ── Pad measures characters, not bytes, and repeats the whole
// pad string -- multi-byte text pads identically on all backends. ──
#[test]
fn f64b_026_pad_counts_characters_parity() {
    let dir = unique_temp_dir("f64p3_t026");
    let source = concat!(
        "p <= Pad[\"あ\", 5]()\n",
        "stdout(p)\n",
        "stdout(p.length())\n",
        "p2 <= Pad[\"あああ\", 5](char <= \"0\")\n",
        "stdout(p2)\n",
        "stdout(p2.length())\n",
        "q <= Pad[\"あ\", 5](char <= \"・\")\n",
        "stdout(q)\n",
        "stdout(q.length())\n",
        "q2 <= PadLeft[\"い\", 4, \"★\"]()\n",
        "stdout(q2)\n",
        "stdout(q2.length())\n",
        "p3 <= Pad[\"abc\", 5]()\n",
        "stdout(p3)\n",
    );
    let out = assert_parity(&dir, "f64b_026_pad", source);
    assert!(out.contains("・・・・あ"), "multibyte pad lost: {out}");
    assert!(out.contains("★★★い"), "multibyte pad-left lost: {out}");
}

// ── ①: socket ports outside 0..=65535 are rejected instead of
// silently truncating to another port. ──
#[test]
fn f64b_027_out_of_range_port_rejected() {
    for port in ["-1", "65536"] {
        let dir = unique_temp_dir("f64p3_t027port");
        let td = dir.join("main.td");
        std::fs::write(
            &td,
            format!(">>> taida-lang/os => @(udpBind)\nr <= udpBind(\"127.0.0.1\", {port})\n"),
        )
        .expect("write fixture");
        let (_out, err, _code) = run_interp_with(&td, &[]);
        assert!(
            err.contains("port must be within 0..=65535"),
            "port {port} not rejected: {err}"
        );
    }
}

// ── ②: an absurd recv size resolves to a Lax failure with kind
// `too_large` (64MB ceiling) instead of aborting the process. Parity pin:
// the Native backend shares the ceiling and the error surface. (The os
// package is out of scope for wasm-min, so no wasm leg here.) ──
#[test]
fn f64b_027_recv_over_cap_is_lax_too_large_parity() {
    let dir = unique_temp_dir("f64p3_t027recv");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        concat!(
            ">>> taida-lang/os => @(socketRecvExact)\n",
            "b <= socketRecvExact(999999, 999999999999)\n",
            "b >=> lax\n",
            "stdout(lax.hasValue().toString())\n",
            "info <= lax.errorInfo()\n",
            "info >=> err\n",
            "stdout(err.kind)\n",
        ),
    )
    .expect("write fixture");
    let interp = run_interpreter(&td).expect("interpreter runs");
    let native = build_and_run_native(&td, &dir, "f64b_027_recv");
    assert_eq!(interp.trim_end(), native, "interp vs native");
    assert_eq!(interp.trim_end(), "false\ntoo_large");
}

// ── rounding molds are the identity on Int (no f64 detour),
// all-Int Sum stays exact above 2^53, and unary neg wraps like the
// binary operators. Parity pin across backends. ──
#[test]
fn f64b_028_exact_int_arithmetic_parity() {
    let dir = unique_temp_dir("f64p3_t028");
    let source = concat!(
        "stdout(Floor[9007199254740993]().toString())\n",
        "stdout(Ceil[9007199254740993]().toString())\n",
        "stdout(Round[9007199254740993]().toString())\n",
        "stdout(Truncate[9007199254740993]().toString())\n",
        "stdout(Floor[3.7]().toString())\n",
        "stdout(Ceil[-2.3]().toString())\n",
        "stdout(Sum[@[9007199254740993, 1]]().toString())\n",
        "stdout(Sum[@[1.5, 2.5]]().toString())\n",
        "n <= 0 - 9223372036854775807 - 1\n",
        "m <= -n\n",
        "stdout(m.toString())\n",
    );
    let out = assert_parity(&dir, "f64b_028_intmath", source);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 9, "unexpected line count: {out}");
    for line in &lines[..4] {
        assert_eq!(*line, "9007199254740993", "identity broken: {line}");
    }
    assert_eq!(lines[6], "9007199254740994", "all-Int Sum lost bits");
    assert_eq!(lines[8], "-9223372036854775808", "neg wrap mismatch");
}

// ── importing a module that privately defines a same-name
// Enum must not leak its registry entry -- ordinals of the importer's
// own Enum stay intact. ──
#[test]
fn f64b_017_private_import_enum_does_not_leak_ordinals() {
    let dir = unique_temp_dir("f64p3_t017");
    std::fs::write(
        dir.join("colorlib.td"),
        "Enum => Color = :Green :Red\ndummy <= 1\n",
    )
    .expect("write module");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"Enum => Color = :Red :Green
>>> ./colorlib.td => @(dummy)
stdout(Color:Red())
"#,
    )
    .expect("write fixture");
    let out = run_interpreter(&td).expect("interpreter runs");
    assert_eq!(
        out.trim_end(),
        "0",
        "imported private enum clobbered ordinals"
    );
}

// ── a shadowed self-call inside a function body is NOT a tail
// call -- it must terminate with a runtime error instead of looping the
// trampoline forever. ──
#[test]
fn f64b_018_shadowed_self_call_is_not_tail_call() {
    let dir = unique_temp_dir("f64p3_t018");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"f x: Int =
  f <= 3
  f(x)
=> :Int
stdout(f(-1))
"#,
    )
    .expect("write fixture");
    let started = std::time::Instant::now();
    let (_out, err, _code) = run_interp_with(&td, &[]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "shadowed self-call looped instead of terminating"
    );
    assert!(
        err.contains("Cannot call non-function value"),
        "expected shadowing semantics error, got: {err}"
    );
}

#[test]
fn local_function_shadow_and_type_scope_are_preserved() {
    let dir = unique_temp_dir("local_scope_audit");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"Thing = @(a: Int)
f x: Int =
  Thing = @(b: Str)
  f <= _ n: Int =
    n + 1
  f(x)
=> :Int
stdout(f(8))
p <= Thing(a <= 5)
stdout(p)
"#,
    )
    .unwrap();
    let (out, err, code) = run_interp_with(&td, &["--no-check"]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(out.trim_end(), "9\n@(a <= 5, __type <= \"Thing\")");
    std::fs::remove_dir_all(dir).unwrap();
}

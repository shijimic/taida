//! C26B-019: `TypeName(field <= value, ...)` constructor must parse across
//! multiple lines, matching the parity already provided by `@(...)` buchi
//! pack literals.
//!
//! # Root cause
//!
//! `parse_arg_list` in `src/parser/parser_expr.rs` did not call
//! `skip_newlines()` at the start or after separators, so the very first
//! Newline inside `Ctx(\n  a <= x, ...)` tripped the `parse_expression`
//! entry and the subsequent commas / closing paren surfaced as stray
//! tokens. Additionally, the TypeInst detection branch at
//! `parse_primary_expr::Ident(_)` did not skip newlines before its
//! `check_ident()` + `LtEq` lookahead, so `Ctx(\n ...)` was never even
//! recognised as a TypeInst — it fell through to the plain `FuncCall`
//! branch whose `parse_arg_list` then failed on the `<=` token.
//!
//! # Fix
//!
//! Two paired `skip_newlines()` insertions:
//!
//! 1. `parse_primary_expr` — after consuming `(` in the `TypeInst` lookahead
//!    (`Name(` uppercase), skip newlines so the `ident <= ...` peek-2
//!    heuristic fires even when the first field is on the next line.
//! 2. `parse_arg_list` — skip newlines on entry, after every slot, and
//!    after every comma. This allows every multi-line call form (positional
//!    arg list, trailing-comma + closing paren on next line, empty
//!    `f(\n)`) to parse identically to the single-line form.
//!
//! # Why this is a widening (§ 6.2), not a breaking change
//!
//! Pre-fix, all of these forms produced parse errors. Post-fix they parse
//! as the obvious single-line equivalent. No previously-valid surface
//! parses to a different AST. The `tests/parity.rs` existing assertions
//! and `parser_tests.rs` single-line fixtures are untouched.
//!
//! # Checker / build parser parity
//!
//! `src/parser/parser.rs::parse` is the single public entry point invoked
//! by `taida way check` (`SemanticAnalyzer` → `parse`) and `taida <file>` /
//! `taida build` (`Interpreter` → `parse`). Both paths share the same
//! parser struct and recovery behaviour — there is no separate "checker
//! parser". Any divergence between `check` result and build result on the
//! same input is therefore a symptom of something other than duplicated
//! parser implementations (e.g. lexer input normalisation,
//! token-stream trimming at a higher layer). The regression tests below
//! pin the 2-way invariant: `taida way check` and `taida run` must report the
//! same parse-error count on the same input.
//!
//! # Scope
//!
//! 3-backend parity (interpreter / JS / native). WASM lowering inherits
//! the AST so is covered transitively.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn write_fixture(body: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("taida_c26b019_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create fixture dir");
    let path = dir.join("main.td");
    fs::write(&path, body).expect("write fixture");
    path
}

fn run_interp(path: &Path) -> (String, i32) {
    let output = Command::new(taida_bin())
        .arg(path)
        .output()
        .expect("run interp");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn run_check(path: &Path) -> (String, String, i32) {
    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(path)
        .output()
        .expect("run check");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

const FIXTURE_MULTILINE_SIMPLE: &str = r#"Ctx = @(a: Str, b: Str, c: Str)

mkOne x: Str = Ctx(a <= x, b <= x, c <= x) => :Ctx

mkMulti x: Str =
  Ctx(
    a <= x,
    b <= x,
    c <= x
  )
=> :Ctx

one <= mkOne("one")
multi <= mkMulti("multi")
stdout(one.a)
stdout(multi.b)
"#;

const FIXTURE_TRAILING_COMMA: &str = r#"Point = @(x: Int, y: Int)

mkPt a: Int b: Int =
  Point(
    x <= a,
    y <= b,
  )
=> :Point

p <= mkPt(3, 4)
stdout(p.x + p.y)
"#;

const FIXTURE_NESTED_MULTILINE: &str = r#"Inner = @(v: Int)
Outer = @(i: Inner, tag: Str)

mkBoth x: Int =
  Outer(
    i <= Inner(
      v <= x
    ),
    tag <= "nested"
  )
=> :Outer

r <= mkBoth(42)
stdout(r.tag)
stdout(r.i.v)
"#;

const FIXTURE_MULTILINE_FUNC_CALL: &str = r#"add a: Int b: Int c: Int = a + b + c => :Int

r <= add(
  1,
  2,
  3
)
stdout(r)
"#;

/// Core repro: the exact pattern from hono-inspired HI-006 /
/// `probe_typedef_multiline.td` must parse on all backends.
#[test]
fn c26b_019_multiline_typedef_constructor_parses_interp() {
    let path = write_fixture(FIXTURE_MULTILINE_SIMPLE, "simple");
    let (stdout, code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(code, 0, "interpreter must exit 0, got {}", code);
    assert_eq!(
        stdout.trim(),
        "one\nmulti",
        "multi-line constructor stdout must match single-line equivalent"
    );
}

/// Trailing-comma variant: closing paren on its own line, last field has a
/// trailing comma. Pre-fix this hit the same Newline parse error and also
/// produced a stray Hole after the comma.
#[test]
fn c26b_019_trailing_comma_parses() {
    let path = write_fixture(FIXTURE_TRAILING_COMMA, "trailing_comma");
    let (stdout, code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(code, 0, "interpreter must exit 0, got {}", code);
    assert_eq!(stdout.trim(), "7");
}

/// Nested constructors: each nesting level spans multiple lines. Pre-fix
/// only the outermost `(` was tolerated (no lookahead path), so every
/// level broke independently.
#[test]
fn c26b_019_nested_multiline_constructors_parse() {
    let path = write_fixture(FIXTURE_NESTED_MULTILINE, "nested");
    let (stdout, code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(code, 0, "interpreter must exit 0, got {}", code);
    assert_eq!(stdout.trim(), "nested\n42");
}

/// Multi-line ordinary function call (positional args). The same
/// `parse_arg_list` newline tolerance must apply to `fn(\n  1,\n  2\n)`
/// not just TypeDef constructors — they share the same helper.
#[test]
fn c26b_019_multiline_function_call_parses() {
    let path = write_fixture(FIXTURE_MULTILINE_FUNC_CALL, "fncall");
    let (stdout, code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(code, 0, "interpreter must exit 0, got {}", code);
    assert_eq!(stdout.trim(), "6");
}

/// Checker / build parser parity: `taida way check` and `taida <file>` must
/// agree on parse success/failure for the same input. C26B-019 secondary
/// observation flagged "check passes, build fails" as a diagnostics-trust
/// issue; the repository has a single `parse` entry point, and this test
/// pins that invariant so any future divergence (e.g. if a wrapper layer
/// were added) will fail CI.
#[test]
fn c26b_019_check_and_run_parser_parity_on_multiline() {
    let path = write_fixture(FIXTURE_MULTILINE_SIMPLE, "parity");
    let (check_stdout, _check_stderr, check_code) = run_check(&path);
    let (run_stdout, run_code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(
        check_code, 0,
        "taida way check must pass on the fixture (got exit {}, stdout: {})",
        check_code, check_stdout
    );
    assert_eq!(
        run_code, 0,
        "taida <file> must run the same fixture (got exit {}, stdout: {})",
        run_code, run_stdout
    );
    assert_eq!(run_stdout.trim(), "one\nmulti");
}

/// 3-backend parity: interpreter + JS + native must emit byte-identical
/// stdout on the multi-line TypeDef constructor fixture. This is the
/// stable-surface gate for C26B-019.
#[test]
fn c26b_019_multiline_3backend_parity() {
    let path = write_fixture(FIXTURE_NESTED_MULTILINE, "parity3");
    let dir = path.parent().unwrap().to_path_buf();

    let (interp_out, interp_code) = run_interp(&path);
    assert_eq!(interp_code, 0, "interp exit non-zero");

    if node_available() {
        let js_path = dir.join("out.mjs");
        let build = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&path)
            .arg("-o")
            .arg(&js_path)
            .output()
            .expect("build js");
        assert!(
            build.status.success(),
            "js build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new("node").arg(&js_path).output().expect("run js");
        assert!(run.status.success(), "node exit non-zero");
        let js_out = String::from_utf8_lossy(&run.stdout).to_string();
        assert_eq!(
            js_out.trim(),
            interp_out.trim(),
            "JS backend must match interpreter on multi-line nested TypeDef constructor"
        );
    }

    if cc_available() {
        let bin_path = dir.join("out.bin");
        let build = Command::new(taida_bin())
            .args(["build", "native"])
            .arg(&path)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .expect("build native");
        assert!(
            build.status.success(),
            "native build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&bin_path).output().expect("run native bin");
        assert!(run.status.success(), "native exit non-zero");
        let nat_out = String::from_utf8_lossy(&run.stdout).to_string();
        assert_eq!(
            nat_out.trim(),
            interp_out.trim(),
            "Native backend must match interpreter on multi-line nested TypeDef constructor"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

/// Regression guard: single-line form (which already worked pre-fix) must
/// keep working. Protects against any over-correction in `parse_arg_list`
/// that would break the canonical `Ctx(a <= x, b <= x, c <= x)` shape.
#[test]
fn c26b_019_single_line_constructor_still_parses() {
    const FIXTURE: &str = r#"Ctx = @(a: Str, b: Str)
mk x: Str = Ctx(a <= x, b <= x) => :Ctx
r <= mk("single")
stdout(r.a)
"#;
    let path = write_fixture(FIXTURE, "single");
    let (stdout, code) = run_interp(&path);
    let _ = fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "single");
}

/// Regression tests for the wasm scalar-intrinsic expansion.
///
/// Scalar operations (int/float arithmetic and comparisons, bool
/// composition, int->float) are expanded by the wasm-min emitter into
/// C expressions instead of runtime calls; the return-tag access is
/// expanded into a direct slot swap. These tests pin two things:
/// the value semantics stay identical on every backend (the expanded
/// expressions mirror the runtime bodies, including the small-int
/// lift on float paths and wrapping integer arithmetic), and the hot
/// loop structurally loses its operation calls (the WAT layer, which
/// needs wasm-tools and skips where unavailable — the always-on
/// textual guard lives in the emitter's unit tests).
mod common;

use common::{run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::{Path, PathBuf};
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

fn build_wasm(td: &Path, dir: &Path, stem: &str) -> PathBuf {
    let wasm = dir.join(format!("{stem}.wasm"));
    let status = Command::new(taida_bin())
        .args(["build", "wasm-min"])
        .arg(td)
        .arg("-o")
        .arg(&wasm)
        .status()
        .expect("taida build wasm-min runs");
    assert!(status.success(), "wasm build failed for {stem}");
    wasm
}

fn run_wasm(wasm: &Path) -> Option<String> {
    let wasmtime = wasmtime_bin()?;
    let out = Command::new(&wasmtime)
        .arg(wasm)
        .output()
        .expect("wasmtime runs");
    assert!(
        out.status.success(),
        "wasm run failed for {}",
        wasm.display()
    );
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn assert_parity(dir: &Path, stem: &str, source: &str) -> String {
    let td = dir.join(format!("{stem}.td"));
    std::fs::write(&td, source).expect("write fixture");
    let interp = run_interpreter(&td).expect("interpreter runs");
    let native = build_and_run_native(&td, dir, stem);
    assert_eq!(interp, native, "{stem}: interp vs native");
    let wasm = build_wasm(&td, dir, stem);
    if let Some(wasm_out) = run_wasm(&wasm) {
        assert_eq!(interp, wasm_out, "{stem}: interp vs wasm-min");
    } else {
        eprintln!("SKIP: wasmtime not found, wasm leg skipped for {stem}");
    }
    interp
}

fn wasm_tools_bin() -> Option<PathBuf> {
    let candidate = std::env::var_os("HOME").map(|h| {
        PathBuf::from(h)
            .join(".cargo")
            .join("bin")
            .join("wasm-tools")
    });
    if let Some(p) = candidate
        && p.exists()
    {
        return Some(p);
    }
    let ok = Command::new("wasm-tools")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    ok.then(|| PathBuf::from("wasm-tools"))
}

/// The expanded expressions preserve the exact value semantics on
/// every backend: wrapping integer arithmetic at the i64 boundaries,
/// large-Int operands entering float comparisons/arithmetic (the
/// small-int lift), IEEE NaN / infinity / signed-zero comparisons,
/// bool composition, and the int->float rounding at 2^53.
#[test]
fn scalar_semantics_parity_across_backends() {
    let dir = unique_temp_dir("f59_w1_semantics");
    let out = assert_parity(
        &dir,
        "semantics",
        r#"stdout(9223372036854775807 + 1)
stdout(0 - 9223372036854775807 - 2)
stdout(4611686018427387904 * 2)
stdout(2097153 > 2.5)
stdout(2097153 * 1.5)
Sqrt[0.0 - 1.0]() >=> nan
stdout(nan == nan)
stdout(nan != nan)
stdout(nan < 1.0)
stdout(nan > 1.0)
big <= 1.0e308 * 10.0
stdout(big > 1.0e308)
nz <= 0.0 - 0.0
stdout(nz == 0.0)
stdout(nz < 0.0)
stdout(true && false)
stdout(false || true)
stdout(!true)
Float[9007199254740993]() >=> f
stdout(f)
stdout(1 < 2)
stdout(2.5 >= 2.5)
"#,
    );
    assert_eq!(
        out,
        "-9223372036854775808\n9223372036854775807\n-9223372036854775808\ntrue\n3145729.5\nfalse\ntrue\nfalse\nfalse\ntrue\ntrue\nfalse\nfalse\ntrue\nfalse\n9007199254740992.0\ntrue\ntrue"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Structural pin (optional layer — needs wasm-tools): the hot loop
/// of a scalar tail-recursive function contains zero `call`
/// instructions and performs its arithmetic inline. The loop function
/// is located through a unique sentinel constant.
#[test]
fn hot_loop_has_no_operation_calls_in_wat() {
    let Some(wasm_tools) = wasm_tools_bin() else {
        eprintln!("SKIP: wasm-tools not found, WAT structural pin skipped");
        return;
    };
    let dir = unique_temp_dir("f59_w1_wat");
    let td = dir.join("loop.td");
    // `acc * 3 + n` is a multiplicative recurrence: LLVM's scalar
    // evolution folds additive recurrences to closed forms (a plain
    // sum loop disappears entirely, sentinel included) but cannot fold
    // this one, so the loop body provably survives into the WAT.
    std::fs::write(
        &td,
        r#"mash n: Int acc: Int =
  | n == 0 |> acc
  | _ |> mash(n - 1, acc * 3 + n)
=> :Int

stdout(mash(100003, 0))
"#,
    )
    .expect("write fixture");
    let interp = run_interpreter(&td).expect("interpreter runs");
    let wasm = build_wasm(&td, &dir, "loop");
    let out = Command::new(&wasm_tools)
        .arg("print")
        .arg(&wasm)
        .output()
        .expect("wasm-tools print runs");
    assert!(out.status.success(), "wasm-tools print failed");
    let wat = String::from_utf8_lossy(&out.stdout);

    // The user function gets inlined and the loop peel/unroll shape
    // may transform the start constant away, so the loop cannot be
    // located by a sentinel. Instead: collect every `loop` block (loop
    // line to its matching `end` by indentation) and look at those
    // containing `i64.mul` — the multiplicative recurrence survives in
    // every shape, and no current runtime loop both multiplies and
    // calls. The recurrence loop must be there and every such loop
    // must be call-free (a regression re-introducing operation calls
    // trips the assertion).
    let mut mul_loops = Vec::new();
    let lines: Vec<&str> = wat.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("loop") {
            let loop_indent = line.len() - line.trim_start().len();
            let mut block = Vec::new();
            let mut j = i + 1;
            // Collect only the loop's own lines: a nested loop and its
            // body are skipped (they are examined by their own pass of
            // the outer scan), so a runtime loop nesting a multiplying
            // inner loop is not misattributed.
            while j < lines.len() {
                let l = lines[j];
                let ind = l.len() - l.trim_start().len();
                if l.trim_start().starts_with("end") && ind <= loop_indent {
                    break;
                }
                if l.trim_start().starts_with("loop") {
                    let inner_indent = ind;
                    j += 1;
                    while j < lines.len() {
                        let il = lines[j];
                        let iind = il.len() - il.trim_start().len();
                        if il.trim_start().starts_with("end") && iind <= inner_indent {
                            break;
                        }
                        j += 1;
                    }
                    j += 1;
                    continue;
                }
                block.push(l);
                j += 1;
            }
            let text = block.join("\n");
            // The recurrence loop is the one doing i64 multiply AND
            // i64 add in the same block; the runtime's own loops
            // (decimal formatters etc.) multiply with i32 index
            // arithmetic only and must not be matched here.
            if text.contains("i64.mul") && text.contains("i64.add") {
                mul_loops.push(text);
            }
            // Do not jump past the block: nested loops are picked up
            // by their own iteration of this scan.
        }
        i += 1;
    }
    assert!(
        !mul_loops.is_empty(),
        "no loop block does inline i64.mul + i64.add — the recurrence arithmetic is not expanded"
    );
    for loop_text in &mul_loops {
        assert!(
            !loop_text.contains("call "),
            "the recurrence hot loop must not contain calls, got:\n{loop_text}"
        );
    }
    if let Some(run) = run_wasm(&wasm) {
        assert_eq!(run, interp, "loop value: interp vs wasm");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

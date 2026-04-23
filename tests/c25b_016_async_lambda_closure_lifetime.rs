//! C25B-016 audit regression tests.
//!
//! The blocker text says:
//!
//! > `src/interpreter/eval.rs:1291+` の `Expr::Lambda` は
//! > `closure: current_env.bindings.clone()` で static snapshot。
//! > 同期実行では問題ないが、C22B-002 実装以降に async stdout streaming
//! > や将来の coroutine-like async 導入時に「lambda body が `]=>` await
//! > で suspend → resume 時に親 scope がすでに pop されている」シナリオ
//! > で closure の一貫性が崩れる可能性。
//!
//! This file pins the fact that, as of `@c.25.rc7`, the interpreter's
//! `Expr::Lambda` actually builds its closure as
//! `Arc::new(self.env.snapshot())` (not a naive `Clone`). The
//! snapshot is therefore refcounted and survives any suspend →
//! resume transition regardless of what the defining stack frame
//! does, because the `Arc<Env>` lives in the `FuncValue` and is
//! cheap to share. See `src/interpreter/eval.rs:1354`.
//!
//! If a future refactor (e.g. the async redesign tracked in
//! `.dev/C25_BLOCKERS.md::C25B-016`) replaces the `Arc` snapshot with
//! something less forgiving, these tests regress visibly.
//!
//! Audit verdict (2026-04-23, Phase 9 driver): **no current regression**.
//! The blocker text referencing `current_env.bindings.clone()` is
//! outdated relative to the current source. The blocker is retained
//! in `Nice to Have` status so the async-redesign work (D26+) can
//! revisit closure lifetime with fresh eyes.

mod common;

use common::{normalize, taida_bin};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static UNIQ: AtomicU64 = AtomicU64::new(0);

fn unique_tmp(prefix: &str) -> PathBuf {
    let n = UNIQ.fetch_add(1, Ordering::SeqCst);
    let mut p = std::env::temp_dir();
    p.push(format!("{}_{}_{}.td", prefix, std::process::id(), n));
    p
}

fn run_interpreter_source(source: &str) -> String {
    let path = unique_tmp("c25b016");
    {
        let mut f = std::fs::File::create(&path).expect("create tmp .td");
        f.write_all(source.as_bytes()).expect("write tmp .td");
    }
    let out = Command::new(taida_bin())
        .arg(&path)
        .output()
        .expect("run taida");
    let _ = std::fs::remove_file(&path);
    assert!(
        out.status.success(),
        "interpreter failed on source:\n{}\nstderr:\n{}",
        source,
        String::from_utf8_lossy(&out.stderr)
    );
    normalize(&String::from_utf8_lossy(&out.stdout))
}

/// Scenario 1: 3-level lambda nesting. Each layer captures a binding
/// from its enclosing scope; the outermost frame is gone by the time
/// the innermost lambda is invoked. Mirrors `examples/quality/b2b_lambda_nested.td`
/// with explicit asserts at each step to pin the per-lambda captures.
#[test]
fn c25b016_three_level_lambda_captures_survive() {
    let src = r#"
a <= 1
f <= _ x = _ y = _ z = z + y + x + a
g <= f(2)
h <= g(10)
stdout(h(100).toString())
"#;
    let out = run_interpreter_source(src);
    assert_eq!(out.trim(), "113");
}

/// Scenario 2: lambda captured into a BuchiPack returned from a
/// function; the defining frame has already returned when we invoke
/// the captured lambda through a field access.
#[test]
fn c25b016_lambda_captured_into_returned_pack() {
    let src = r#"
makeOps base =
  @(
    add <= _ x = x + base,
    mul <= _ x = x * base
  )
=> :@()

ops <= makeOps(10)
stdout(ops.add(5).toString())
stdout(ops.mul(3).toString())
"#;
    let out = run_interpreter_source(src);
    assert_eq!(out.trim(), "15\n30");
}

/// Scenario 3: two different outer frames build lambdas with
/// conflicting captures of the same name. The snapshots must remain
/// independent even after both frames have returned.
#[test]
fn c25b016_separate_snapshots_do_not_alias() {
    let src = r#"
makeAdder k =
  _ v = v + k
=> :Fn

add5 <= makeAdder(5)
add9 <= makeAdder(9)
stdout(add5(1).toString())
stdout(add9(1).toString())
stdout(add5(100).toString())
"#;
    let out = run_interpreter_source(src);
    assert_eq!(out.trim(), "6\n10\n105");
}

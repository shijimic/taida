//! D28B-020 (Option A) — production `panic!` baseline meta regression test.
//!
//! ## 背景 (Round 1 wB session、2026-04-26)
//!
//! D28B-020 contract 起案時、`src/graph/query.rs` 11 箇所 + `src/addon/value_bridge.rs`
//! 11 箇所 の panic! を「production audit 対象 22 箇所」として `Result<T, Err>` 化する
//! 前提だった。Round 1 wB の探索で、これら 22 箇所は**全て `#[cfg(test)] mod tests`
//! 内 (test idiom `match { Ok(...) => ..., other => panic!("expected X") }`)** であり、
//! `src/graph/` + `src/addon/` の production region に panic! が残っていない事実が判明。
//!
//! User verdict: **Option A** — production panic! baseline を pin して forward
//! protection に転換 (`scripts/lint/panic_baseline.sh`)、D28B-020 を CLOSED に flip。
//!
//! ## 全 src/ tree audit による補足
//!
//! Round 1 wB 実装中に `src/graph/` + `src/addon/` 以外も含めた full audit を実施。
//! production region (filename が `*_tests.rs` / `tests.rs` でない、かつ
//! `#[cfg(test)]` line より前) に残る panic! は次の 2 箇所のみ:
//!
//! - `src/codegen/driver.rs` — IR cache invariant 違反時の `BUG: ...` panic
//! - `src/parser/ast.rs` — `body_expr()` の precondition 違反 panic
//!   (`debug_assert_eq!` で contract を明示後の defensive panic)
//!
//! どちらも user-input 由来ではなく、compiler 内部の invariant 違反を signal する
//! defensive panic で、D28B-020 の「invariant 違反の internal panic は限定的に許容
//! され得る」判断に整合する。これら 2 箇所を baseline として pin し、新規 panic!
//! 追加 / 既存 panic! の silent な行ずれ・除去を CI で検出する。
//!
//! ## このテストの役割
//!
//! 1. `production_panic_baseline_holds`: `scripts/lint/panic_baseline.sh` を invoke し
//!    exit code 0 を確認 (production panic! 数とサイトが pin と一致)。`cargo test
//!    --release` から CI hard-fail として走らせる経路を提供する。
//! 2. `panic_baseline_script_exists_and_executable`: gate script の存在と x bit を
//!    確認 (リポジトリ移動・packaging 段階での欠落事故を防ぐ smoke check)。

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR は `cargo test` 実行時に必ず set される。
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn baseline_script() -> PathBuf {
    repo_root().join("scripts/lint/panic_baseline.sh")
}

#[test]
fn panic_baseline_script_exists_and_executable() {
    let script = baseline_script();
    assert!(
        script.exists(),
        "panic_baseline.sh missing at {}",
        script.display()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&script)
            .expect("stat panic_baseline.sh")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "panic_baseline.sh is not executable (mode={:o})",
            mode
        );
    }
}

#[test]
fn production_panic_baseline_holds() {
    let script = baseline_script();
    let output = Command::new("bash")
        .arg(&script)
        .current_dir(repo_root())
        .output()
        .expect("invoke panic_baseline.sh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "panic_baseline.sh failed (exit={:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status.code(),
        stdout,
        stderr,
    );
}

//! C27B-018 Option B (@c.27 Round 3, wf018B): codegen lifetime tracking
//! pass for short-lived bindings.
//!
//! ## 背景
//!
//! Pre-Option-B (@c.27 Round 2 wH 完了時点): native codegen は短命 binding
//! (`s <= Repeat["x", N]()` 等) に対し `taida_release` IR を emit せず、
//! 関数末尾の一括 Release 列に頼っていた。しかし末尾再帰 (`iter(n - 1)`)
//! は `TailCall` に置換されて entry block にジャンプするため末尾の Release
//! を skip し、CondBranch arm 内で生まれて死ぬ binding は `current_heap_vars`
//! に登録される pass にも辿り着かない。結果、`iter n = | _ |> s <= Repeat
//! ["x", 512](); iter(n - 1)` の 1M iter native binary は peak RSS が
//! **533 MB** まで膨らんでいた。
//!
//! ## Option B の修正
//!
//! - `taida_release_any` runtime helper を core.c に追加: hidden header
//!   (heap-string) と magic header (Pack/List/Closure 等) を runtime
//!   dispatch して適切な release path を呼ぶ。
//! - `IrInst::ReleaseAuto(IrVar)` を IR に追加: emit 時に
//!   `taida_release_any` を呼ぶ。
//! - `src/codegen/lifetime.rs` lifetime tracking pass を追加: 関数 body /
//!   CondArm body 内の `DefVar(name, value_var)` を走査し、後続が
//!   `name` を参照しない & `value_var` を escape させない場合、
//!   DefVar 直後に `ReleaseAuto(value_var)` を挿入する。
//!
//! ## このテストの役割
//!
//! 1. **`option_b_repeat_1m_peak_rss_capped`**: `iter n = | _ |>
//!    s <= Repeat["x", 512](); iter(n - 1)` の 1M iter native binary を
//!    実行し、peak RSS が 50 MB 以下であることを確認する。fix 前は 533 MB。
//! 2. **`option_b_runtime_helper_referenced`**: emit された C source に
//!    `taida_release_any` の参照が現れることを観測 (codegen pass が
//!    実際に動作している証拠)。
//! 3. **`option_b_recycle_fixture_invariant`**: 既存の 1000-iter recycle
//!    fixture が引き続き正常出力かつ wall-clock budget 内に収まることを
//!    確認 (Option B が freelist recycle path を破壊していないことを保証)。

mod common;

use common::{normalize, taida_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp_artifact(stem: &str, suffix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "c27b_018_option_b_{}_{}_{}.{}",
        std::process::id(),
        seq,
        stem,
        suffix
    ))
}

fn build_native(td_path: &Path) -> Option<PathBuf> {
    let bin_path = tmp_artifact(
        td_path.file_stem().unwrap().to_string_lossy().as_ref(),
        "bin",
    );
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .ok()?;
    if !build.status.success() {
        eprintln!(
            "native build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }
    Some(bin_path)
}

fn write_temp_td(content: &str, stem: &str) -> PathBuf {
    let p = tmp_artifact(stem, "td");
    std::fs::write(&p, content).expect("write temp td");
    p
}

/// (1) Peak-RSS smoke: 1M iter `Repeat["x", 512]()` short-lived binding.
/// Pre-fix: 533 MB peak RSS. Post-fix target: < 50 MB.
///
/// Implementation note: we do not invoke `/usr/bin/time -v` portably from
/// Rust; instead we use `getrusage(RUSAGE_CHILDREN)` after waiting on the
/// child. Linux returns ru_maxrss in KB, BSD/macOS in bytes — for CI we
/// assume Linux (the only soak target). On non-Linux the test is skipped.
#[test]
#[cfg(target_os = "linux")]
fn option_b_repeat_1m_peak_rss_capped() {
    let td = write_temp_td(
        "iter n =\n  | n == 0 |> 0\n  | _ |>\n      s <= Repeat[\"x\", 512]()\n      iter(n - 1)\n=> :Int\n\nstdout(iter(1000000))\n",
        "repeat_1m",
    );
    let bin = build_native(&td).expect("native build should succeed");

    let out = Command::new(&bin)
        .output()
        .expect("native binary should run");
    assert!(out.status.success(), "native binary failed: {:?}", out);
    let stdout = normalize(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(stdout.trim(), "0", "iter output drift");

    // Read peak RSS from /proc/self/children — instead, easier: use
    // getrusage RUSAGE_CHILDREN after wait. Since we used .output(),
    // the child has been reaped; getrusage RUSAGE_CHILDREN gives the
    // accumulated max_rss across all reaped children in this process.
    // To isolate just THIS child's RSS we re-run via a wrapper script
    // or read /proc/<pid>/status before exit — both fragile in tests.
    //
    // Pragmatic approach: spawn the binary again under `/usr/bin/time -v`
    // and parse the "Maximum resident set size" line. /usr/bin/time -v
    // is a standard Linux package available in CI runners.
    let timed = Command::new("/usr/bin/time")
        .arg("-v")
        .arg(&bin)
        .output()
        .expect("/usr/bin/time -v should be available on Linux CI");
    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(&td);
    let stderr = String::from_utf8_lossy(&timed.stderr);
    let peak_rss_kb: u64 = stderr
        .lines()
        .find_map(|line| {
            let line = line.trim();
            line.strip_prefix("Maximum resident set size (kbytes): ")
                .and_then(|s| s.parse::<u64>().ok())
        })
        .expect("/usr/bin/time -v should report Maximum resident set size");

    // Pre-fix: 533,572 KB. Post-fix observed: ~2,400 KB. Gate at 50 MB
    // to leave headroom for CI jitter while still catching catastrophic
    // regression of the lifetime pass.
    assert!(
        peak_rss_kb < 50_000,
        "1M iter Repeat['x', 512]() peak RSS {} KB exceeds 50 MB cap — \
         lifetime tracking pass may have regressed (pre-fix baseline: 533 MB)",
        peak_rss_kb
    );
}

/// (2) Codegen lifetime-pass evidence via `nm`: a build that triggers
/// `ReleaseAuto` (the `iter` fixture's `s <= Repeat["x", 32]()` binding
/// is dead-after-DefVar inside the recursive arm body) must reference
/// the `taida_release_any` runtime helper from the user's code object.
/// We verify the symbol presence in the linked binary; if the lifetime
/// pass were disabled, the helper would not be referenced and the
/// (unused) static could be DCE'd by the linker. This is a coarse but
/// robust runtime-side check that the pass fires.
#[test]
#[cfg(target_os = "linux")]
fn option_b_runtime_helper_present_in_binary() {
    let td = write_temp_td(
        "iter n =\n  | n == 0 |> 0\n  | _ |>\n      s <= Repeat[\"x\", 32]()\n      iter(n - 1)\n=> :Int\n\nstdout(iter(10))\n",
        "helper_observe",
    );
    let bin = build_native(&td).expect("native build should succeed");
    let nm = Command::new("nm")
        .arg(&bin)
        .output()
        .expect("nm should be available on Linux CI");
    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(&td);
    let symbols = String::from_utf8_lossy(&nm.stdout);
    assert!(
        symbols.contains("taida_release_any"),
        "binary should reference taida_release_any helper (lifetime pass evidence). \
         If this fails, the lifetime pass is not inserting ReleaseAuto for the iter \
         fixture's `s <= Repeat[\"x\", 32]()` binding, or the helper was DCE'd."
    );
}

/// (3) Recycle fixture invariant: existing 1000-iter recycle should still
/// produce "0" and complete fast. Guards against the case where Option B
/// breaks the freelist recycle path that wH installed.
#[test]
fn option_b_recycle_fixture_invariant() {
    let td = PathBuf::from("examples/quality/c27b_028_async_str_rc/case_04_freelist_recycle.td");
    if !td.exists() {
        eprintln!("recycle fixture missing — skipping");
        return;
    }
    let bin = build_native(&td).expect("native build should succeed");
    let start = std::time::Instant::now();
    let out = Command::new(&bin)
        .output()
        .expect("recycle binary should run");
    let elapsed = start.elapsed();
    let _ = std::fs::remove_file(&bin);
    assert!(out.status.success(), "recycle binary failed: {:?}", out);
    let stdout = normalize(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(
        stdout.trim(),
        "0",
        "recycle fixture output drift after Option B"
    );
    assert!(
        elapsed.as_secs() < 5,
        "recycle fixture took {} ms — possible Option B regression in freelist path",
        elapsed.as_millis()
    );
}

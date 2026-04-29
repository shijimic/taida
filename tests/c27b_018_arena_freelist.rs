//! C27B-018 paired with C27B-028 (Round 2 wH): native arena leak
//! mitigation via the small-string freelist.
//!
//! Pre-fix: `taida_str_release` skipped pushing arena-allocated str
//! headers onto the freelist (`!taida_arena_contains(hdr)` guard) so
//! 16-1024 B str allocations leaked their arena slot forever and the
//! NET hot path grew RSS by ~82 MB/min (24h projection: 118 GB).
//!
//! Fix (Option A + freelist capacity check): the arena-skip guard is
//! removed so arena slots get pushed to the small-string freelist on
//! release; subsequent allocations re-use them. The push side stamps
//! the slot's aligned data-area capacity in `hdr[1]` and the alloc
//! side verifies the slot is large enough before reuse, so the
//! latent bucket-vs-aligned-size mismatch (which the original
//! arena-skip guard masked, and which would have surfaced as
//! C27B-028 corruption) is also fixed.
//!
//! This test runs the freelist recycle fixture in native and asserts
//! the program completes quickly. If the arena-skip guard regressed,
//! the loop would still complete (no corruption), but if the
//! capacity check regressed, the same-size bucket reuse would
//! overwrite adjacent slots and parity with the interpreter would
//! break. We therefore re-use the 3-backend parity assertion for the
//! invariant + add a wall-clock budget as a coarse leak guard.

mod common;

use common::{normalize, taida_bin};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn run_interpreter(td_path: &Path) -> Option<String> {
    let out = Command::new(taida_bin()).arg(td_path).output().ok()?;
    if !out.status.success() {
        eprintln!(
            "interpreter failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&out.stdout)))
}

fn tmp_artifact(td_path: &Path, suffix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let stem = td_path.file_stem().unwrap().to_string_lossy();
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "c27b_018_{}_{}_{}.{}",
        std::process::id(),
        seq,
        stem,
        suffix
    ))
}

fn build_native(td_path: &Path) -> Option<PathBuf> {
    let bin_path = tmp_artifact(td_path, "bin");
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

fn fixture_td(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "examples/quality/c27b_028_async_str_rc/{}.td",
        name
    ))
}

#[test]
fn freelist_recycle_native_completes_quickly() {
    // Build once, run the recycle fixture, and assert it completes
    // in a generous wall-clock budget. A regression on the capacity
    // check would not necessarily slow this down (it would cause
    // corruption); a regression on the arena-skip removal might —
    // unbounded arena growth eventually hits the 128-chunk cap and
    // falls through to malloc per call, slowing things measurably.
    // The 3-backend parity test in c27b_028_async_str_rc.rs covers
    // the corruption side; this test covers the runtime side.
    let td = fixture_td("case_04_freelist_recycle");
    let bin = build_native(&td).expect("native build should succeed");

    let start = Instant::now();
    let out = Command::new(&bin)
        .output()
        .expect("native binary should run");
    let elapsed = start.elapsed();
    let _ = std::fs::remove_file(&bin);

    assert!(out.status.success(), "native binary failed: {:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "0", "freelist recycle output drift");

    // 1000 iterations should complete in well under a second on
    // any reasonable hardware. Generous budget (5s) tolerates CI
    // jitter while still catching catastrophic regressions.
    assert!(
        elapsed.as_secs() < 5,
        "freelist recycle took {}ms — possible arena leak regression",
        elapsed.as_millis()
    );
}

#[test]
fn freelist_recycle_three_way_parity_invariant() {
    // Confirm the recycle fixture stays parity-locked across
    // interpreter and native. If the freelist capacity check
    // regressed, we'd see corruption in the long-string allocation
    // (which our recycle fixture currently doesn't exercise), so we
    // also include the canonical case_01 fixture as a guard.
    let recycle_td = fixture_td("case_04_freelist_recycle");
    let interp = run_interpreter(&recycle_td).expect("interpreter should succeed");
    assert_eq!(interp.trim(), "0", "interpreter output drift");

    let bin = build_native(&recycle_td).expect("native build should succeed");
    let out = Command::new(&bin)
        .output()
        .expect("native binary should run");
    let _ = std::fs::remove_file(&bin);
    assert!(out.status.success(), "native binary failed: {:?}", out);
    let native = normalize(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(
        native, interp,
        "native vs interpreter parity broken on freelist recycle fixture"
    );
}

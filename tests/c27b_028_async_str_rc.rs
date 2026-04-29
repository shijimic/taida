//! C27B-028 paired with C27B-018 (Round 2 wH): Async/Str RC + arena
//! freelist correctness.
//!
//! Pre-fix (Option A applied to `taida_str_release` without the
//! freelist capacity check): the small-string freelist bucketed by
//! requested `total` allowed pop callers to receive a slot whose
//! actual aligned data area was smaller than the requested length.
//! With Option A pushing arena slots, the bug became deterministic
//! and surfaced as silent byte corruption in `Async[rejected: ...]`
//! rendering (PHILOSOPHY I violation).
//!
//! Fix:
//!
//!   * `src/codegen/native_runtime/core.c::taida_str_release` records
//!     the slot's aligned data-area capacity in `hdr[1]` on push.
//!   * `src/codegen/native_runtime/core.c::taida_str_alloc` reads it
//!     back on pop and falls through to arena alloc if the slot is
//!     too small for the requested length.
//!
//! Fixtures live under `examples/quality/c27b_028_async_str_rc/`.

mod common;

use common::{normalize, taida_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

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
        "c27b_028_{}_{}_{}.{}",
        std::process::id(),
        seq,
        stem,
        suffix
    ))
}

fn run_js(td_path: &Path) -> Option<String> {
    let js_path = tmp_artifact(td_path, "mjs");
    let build = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .ok()?;
    if !build.status.success() {
        let _ = std::fs::remove_file(&js_path);
        eprintln!(
            "js build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }
    let run = Command::new("node").arg(&js_path).output().ok()?;
    let _ = std::fs::remove_file(&js_path);
    if !run.status.success() {
        eprintln!(
            "node failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

fn run_native(td_path: &Path) -> Option<String> {
    let bin_path = tmp_artifact(td_path, "bin");
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .ok()?;
    if !build.status.success() {
        let _ = std::fs::remove_file(&bin_path);
        eprintln!(
            "native build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }
    let run = Command::new(&bin_path).output().ok()?;
    let _ = std::fs::remove_file(&bin_path);
    if !run.status.success() {
        eprintln!(
            "native binary failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

fn which_node() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixture_td(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "examples/quality/c27b_028_async_str_rc/{}.td",
        name
    ))
}

fn fixture_expected(name: &str) -> String {
    let path = PathBuf::from(format!(
        "examples/quality/c27b_028_async_str_rc/{}.expected",
        name
    ));
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    normalize(&raw)
}

fn check_three_way(name: &str) {
    let td = fixture_td(name);
    let expected = fixture_expected(name);

    let interp = run_interpreter(&td).expect("interpreter should succeed");
    assert_eq!(
        interp, expected,
        "interpreter drift on {} — .expected is the source of truth (interpreter is reference)",
        name
    );

    let native = run_native(&td).expect("native build/run should succeed");
    assert_eq!(
        native, expected,
        "native parity broken on {} — C27B-028/018 freelist capacity check regressed",
        name
    );

    if which_node() {
        let js = run_js(&td).expect("js build/run should succeed");
        assert_eq!(
            js, expected,
            "js parity broken on {} — Async/Str rendering drifted",
            name
        );
    } else {
        eprintln!("node not available; skipping js leg for {}", name);
    }
}

#[test]
fn case_01_reject_str_three_way_parity() {
    // Direct reproduction of the wB Round 1 corruption signature
    // ("something went wrong]" → "something went wRTSD"). Native
    // must now emit the bracket-closed "wrong]" instead.
    check_three_way("case_01_reject_str");
}

#[test]
fn case_02_stress_alternation_three_way_parity() {
    // Alternates 5-char / 13-char / 49-char rejected strings nine
    // times. Each iteration releases the previous slot and pops a
    // freshly-bucketed slot; pre-fix this corrupts whichever string
    // fits a slot smaller than its required data area.
    check_three_way("case_02_stress_alternation");
}

#[test]
fn case_03_str_repeat_freelist_three_way_parity() {
    // Exercises the freelist with `Repeat[ch, n]()` calls of mixed
    // lengths inside the same buckets (15 / 31 / 47). All three
    // backends must produce byte-identical output without truncation
    // or trailing garbage.
    check_three_way("case_03_str_repeat_freelist");
}

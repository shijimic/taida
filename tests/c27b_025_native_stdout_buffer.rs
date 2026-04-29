//! C27B-025 (carry-over from C26B-021): Native stdout setvbuf line-
//! buffering for 3-backend log flush parity. The implementation
//! (single `setvbuf(stdout, NULL, _IOLBF, 0)` + `setvbuf(stderr, ...,
//! _IOLBF, 0)` at the top of `main()` in
//! `src/codegen/native_runtime/net_h3_quic.c`) was landed in C26 and
//! is exercised by `tests/c26b_021_stdout_flush_parity.rs`.
//!
//! This file exists to satisfy the C27B-025 acceptance criterion that
//! a `tests/c27b_025_native_stdout_buffer.rs` regression test guard
//! the line-buffering invariant under the C27 ID. We re-assert the
//! same three properties without duplicating the slower pipe-timing
//! test (which is already covered by the C26B-021 file).

mod common;

use common::{normalize, taida_bin};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn tempdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("taida_c27b025_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn c27b_025_setvbuf_call_present_in_main() {
    // Read the C source for net_h3_quic.c and assert the setvbuf
    // calls sit inside main(). Catches accidental removal during
    // codegen refactors.
    let src =
        fs::read_to_string("src/codegen/native_runtime/net_h3_quic.c").expect("read net_h3_quic.c");
    let main_pos = src
        .find("int main(int argc, char **argv) {")
        .expect("main entry must exist in net_h3_quic.c");
    let after_main = &src[main_pos..];
    assert!(
        after_main.contains("setvbuf(stdout, NULL, _IOLBF, 0)"),
        "C27B-025 regression: setvbuf(stdout, _IOLBF) missing from main()"
    );
    assert!(
        after_main.contains("setvbuf(stderr, NULL, _IOLBF, 0)"),
        "C27B-025 regression: setvbuf(stderr, _IOLBF) missing from main()"
    );
}

#[test]
fn c27b_025_3backend_stdout_content_parity() {
    // Same byte sequence reaches the pipe on all three backends.
    if !cc_available() {
        eprintln!("cc unavailable; skipping native stdout content parity test");
        return;
    }

    let dir = tempdir("content_parity");
    let td_path = dir.join("main.td");
    fs::write(
        &td_path,
        "stdout(\"alpha\")\nstdout(\"beta\")\nstdout(\"gamma\")\n",
    )
    .expect("write fixture");

    let interp = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("interpreter run");
    assert!(
        interp.status.success(),
        "interpreter failed: {:?}",
        String::from_utf8_lossy(&interp.stderr)
    );
    let interp_out = normalize(&String::from_utf8_lossy(&interp.stdout));

    let bin_path: PathBuf = dir.join("main");
    let build = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(&td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("native build");
    assert!(
        build.status.success(),
        "native build failed: {:?}",
        String::from_utf8_lossy(&build.stderr)
    );
    let native = Command::new(&bin_path).output().expect("native run");
    assert!(
        native.status.success(),
        "native run failed: {:?}",
        String::from_utf8_lossy(&native.stderr)
    );
    let native_out = normalize(&String::from_utf8_lossy(&native.stdout));
    assert_eq!(
        native_out, interp_out,
        "C27B-025 regression: native vs interpreter stdout content mismatch"
    );

    // js leg only if node is available
    if Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        let js_path = dir.join("main.mjs");
        let jbuild = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&td_path)
            .arg("-o")
            .arg(&js_path)
            .output()
            .expect("js build");
        assert!(
            jbuild.status.success(),
            "js build failed: {:?}",
            String::from_utf8_lossy(&jbuild.stderr)
        );
        let js_run = Command::new("node")
            .arg(&js_path)
            .output()
            .expect("node run");
        assert!(
            js_run.status.success(),
            "node failed: {:?}",
            String::from_utf8_lossy(&js_run.stderr)
        );
        let js_out = normalize(&String::from_utf8_lossy(&js_run.stdout));
        assert_eq!(
            js_out, interp_out,
            "C27B-025 regression: js vs interpreter stdout content mismatch"
        );
    }
}

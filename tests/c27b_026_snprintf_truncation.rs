//! C27B-026 Step 3 Option B (carry-over from C26B-022): native HTTP
//! wire `snprintf` truncation must be a parser-level reject, not a
//! silent overflow.
//!
//! Pre-fix: `h2_extract_request_fields` and `h3_extract_request_fields`
//! used `snprintf(out->method, sizeof(...), "%s", headers[i].value)`
//! to copy decoded HPACK / QPACK values into fixed-size struct
//! fields. gcc's -Wformat-truncation warned that values up to 16383
//! bytes (HPACK / QPACK SETTINGS-bounded) could be silently
//! truncated to 16 / 256 / 2048 bytes, and any caller that bypassed
//! the H1 parser cap could lose bytes without diagnostic. Step 2
//! (C26B-022 / C27B-026 prior work) rejected over-cap values at the
//! H1 parser; Step 3 closes the loop on the H2 / H3 paths.
//!
//! Fix:
//!   * New error_reason `H{2,3}_REQ_ERR_PSEUDO_TOO_LONG` in
//!     net_h1_h2.c / net_h3_quic.c.
//!   * `h{2,3}_extract_request_fields` pre-checks
//!     `strlen(headers[i].value)` against the destination buffer
//!     capacity and returns the new error_reason on overflow.
//!   * The actual write switches from `snprintf("%s", value)` to a
//!     bounded `memcpy` so gcc can prove no truncation; this also
//!     drives the -Wformat-truncation warning baseline to 0 for the
//!     four pseudo-header copy sites.
//!
//! This test:
//!   1. Builds a native binary from a trivial fixture and asserts the
//!      build emits no `-Wformat-truncation` warnings for the four
//!      pseudo-header sites in net_h1_h2 / net_h3_quic.
//!   2. Verifies the new error_reason constants live in the C source
//!      (cheap regression guard against a future refactor reverting
//!      the parser-level reject).

mod common;

use common::taida_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tempdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("taida_c27b026_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

#[test]
fn c27b_026_pseudo_too_long_constants_present() {
    // Sanity: the new error_reason constants must exist in both
    // net_h1_h2.c and net_h3_quic.c. A future refactor that removes
    // them without replacement re-opens the silent truncation hole.
    let h12 =
        fs::read_to_string("src/codegen/native_runtime/net_h1_h2.c").expect("read net_h1_h2.c");
    assert!(
        h12.contains("H2_REQ_ERR_PSEUDO_TOO_LONG"),
        "C27B-026 regression: H2_REQ_ERR_PSEUDO_TOO_LONG missing from net_h1_h2.c"
    );

    let h3 =
        fs::read_to_string("src/codegen/native_runtime/net_h3_quic.c").expect("read net_h3_quic.c");
    assert!(
        h3.contains("H3_REQ_ERR_PSEUDO_TOO_LONG"),
        "C27B-026 regression: H3_REQ_ERR_PSEUDO_TOO_LONG missing from net_h3_quic.c"
    );
}

#[test]
fn c27b_026_pseudo_header_snprintf_sites_use_bounded_memcpy() {
    // The four pseudo-header copy sites in each fragment must NOT
    // use snprintf(... "%s", headers[i].value) any more — gcc cannot
    // prove that form safe and the -Wformat-truncation warnings re-
    // appear. The bounded memcpy form replaces them.
    let h12 =
        fs::read_to_string("src/codegen/native_runtime/net_h1_h2.c").expect("read net_h1_h2.c");
    assert!(
        !h12.contains(r#"snprintf(out->method, sizeof(out->method), "%s", headers[i].value)"#),
        "C27B-026 regression: net_h1_h2.c h2_extract_request_fields :method site reverted to snprintf"
    );
    assert!(
        !h12.contains(r#"snprintf(out->path, sizeof(out->path), "%s", headers[i].value)"#),
        "C27B-026 regression: net_h1_h2.c h2_extract_request_fields :path site reverted to snprintf"
    );
    assert!(
        !h12.contains(
            r#"snprintf(out->authority, sizeof(out->authority), "%s", headers[i].value)"#
        ),
        "C27B-026 regression: net_h1_h2.c h2_extract_request_fields :authority site reverted to snprintf"
    );
    assert!(
        !h12.contains(r#"snprintf(scheme, sizeof(scheme), "%s", headers[i].value)"#),
        "C27B-026 regression: net_h1_h2.c h2_extract_request_fields :scheme site reverted to snprintf"
    );

    let h3 =
        fs::read_to_string("src/codegen/native_runtime/net_h3_quic.c").expect("read net_h3_quic.c");
    assert!(
        !h3.contains(r#"snprintf(out->method, sizeof(out->method), "%s", headers[i].value)"#),
        "C27B-026 regression: net_h3_quic.c h3_extract_request_fields :method site reverted to snprintf"
    );
    assert!(
        !h3.contains(r#"snprintf(out->path, sizeof(out->path), "%s", headers[i].value)"#),
        "C27B-026 regression: net_h3_quic.c h3_extract_request_fields :path site reverted to snprintf"
    );
    assert!(
        !h3.contains(r#"snprintf(out->authority, sizeof(out->authority), "%s", headers[i].value)"#),
        "C27B-026 regression: net_h3_quic.c h3_extract_request_fields :authority site reverted to snprintf"
    );
}

#[test]
fn c27b_026_native_build_emits_no_format_truncation_warnings_for_pseudo_headers() {
    // Build a trivial native binary and assert the gcc warning
    // baseline for `-Wformat-truncation` against the four pseudo-
    // header memcpy sites is zero. We capture stderr from the cc
    // sub-invocation and grep.
    if !cc_available() {
        eprintln!("cc unavailable; skipping format-truncation warning baseline test");
        return;
    }

    let dir = tempdir("nowarn");
    let td_path = dir.join("main.td");
    fs::write(&td_path, r#"stdout("hello")"#).expect("write fixture");

    let bin_path = dir.join("main");
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
    let stderr = String::from_utf8_lossy(&build.stderr);

    // Count -Wformat-truncation warnings against the four sizes the
    // pseudo-header struct fields use (16 / 256 / 2048). A non-zero
    // count means our memcpy rewrite failed to convince gcc.
    let bad_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("-Wformat-truncation"))
        .filter(|l| {
            l.contains("region of size 16")
                || l.contains("region of size 256")
                || l.contains("region of size 2048")
        })
        .collect();
    assert!(
        bad_lines.is_empty(),
        "C27B-026 regression: -Wformat-truncation warnings re-appeared on pseudo-header sites:\n{}",
        bad_lines.join("\n")
    );
}

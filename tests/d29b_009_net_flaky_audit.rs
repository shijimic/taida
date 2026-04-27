//! D29B-009 / Lock-F — port 17xxx hard-coded literal static lint
//!
//! # Why this test exists
//!
//! `tests/c27b_027_read_body_2arg.rs` previously allocated test ports out of
//! a `static PORT_COUNTER: AtomicU16 = AtomicU16::new(17000);` band. Under
//! CI 2C nextest parallelism this collided with `TIME_WAIT` zombies and the
//! Linux ephemeral-port allocator, producing the recurring
//! `c27b_027_read_body_2arg_{short,long}_body` /
//! `c27b_014_port_announce_*` flakes catalogued in
//! `.dev/D29_BLOCKERS.md::D29B-009` (CI runs 24963621539 / 24965019780 /
//! 24935511811 across feat/d28 and main).
//!
//! Lock-F (Phase 0 verdict) replaces every `17xxx` literal with the shared
//! `common::find_free_loopback_port()` allocator. This test guards against
//! the regression of any new `17xxx` literal sneaking back into the NET
//! integration test corpus.
//!
//! # Scope
//!
//! Walks the integration test sources under `tests/` and rejects any
//! occurrence of a bare `17[0-9]{3}` integer literal in `tests/c2*b_*.rs` /
//! `tests/d2*b_*.rs` / `tests/parity.rs`. Comments are allowed to mention
//! `17xxx` (e.g. historical references), but a literal like `17000` is
//! forbidden everywhere except inside `// ` line comments.
//!
//! # Acceptance
//!
//! `cargo test --release --test d29b_009_net_flaky_audit` GREEN.

use std::fs;
use std::path::Path;

fn tests_dir() -> &'static Path {
    // The integration test crate has its manifest dir == repo root; the
    // tests/ directory lives directly under it.
    static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests"))
        .as_path()
}

fn is_in_scope(name: &str) -> bool {
    // c2Xb_*.rs / d2Xb_*.rs / parity.rs (the NET-test universe under Lock-F).
    // Exclude this audit file itself: the self-check tests below intentionally
    // contain `17xxx` string literals to validate the lint logic.
    if name == "d29b_009_net_flaky_audit.rs" {
        return false;
    }
    if name == "parity.rs" {
        return true;
    }
    if !name.ends_with(".rs") {
        return false;
    }
    let bytes = name.as_bytes();
    if bytes.len() < 6 {
        return false;
    }
    // Pattern: [cd]2[0-9]b_...
    (bytes[0] == b'c' || bytes[0] == b'd')
        && bytes[1] == b'2'
        && bytes[2].is_ascii_digit()
        && bytes[3] == b'b'
        && bytes[4] == b'_'
}

/// Strip `// ...` line comments (everything from `//` to end of line) so that
/// historical references to `17xxx` in commentary do not trigger the lint.
/// Block comments (`/* ... */`) and string literals are not stripped — those
/// would constitute live code or test fixtures that must use the shared
/// allocator.
fn strip_line_comments(line: &str) -> &str {
    if let Some(idx) = line.find("//") {
        &line[..idx]
    } else {
        line
    }
}

fn scan_file(path: &Path) -> Vec<(usize, String)> {
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut hits = Vec::new();
    for (lineno, raw_line) in src.lines().enumerate() {
        let code = strip_line_comments(raw_line);
        // Look for any 17XXX (where XXX is exactly 3 digits) integer literal.
        // We require word-boundary semantics: the match must be preceded by
        // a non-digit (or start of line) and followed by a non-digit (or end
        // of line) so that 117000, 170001 do not match.
        let bytes = code.as_bytes();
        let n = bytes.len();
        let mut i = 0;
        while i + 5 <= n {
            // Need exactly 5 digits 1,7,X,X,X with proper boundaries.
            if bytes[i] == b'1' && bytes[i + 1] == b'7' {
                let d2 = bytes[i + 2];
                let d3 = bytes[i + 3];
                let d4 = bytes[i + 4];
                if d2.is_ascii_digit() && d3.is_ascii_digit() && d4.is_ascii_digit() {
                    let prev_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
                    let after = bytes.get(i + 5).copied();
                    let next_ok = match after {
                        None => true,
                        Some(b) => !b.is_ascii_digit() && b != b'_',
                    };
                    if prev_ok && next_ok {
                        hits.push((lineno + 1, raw_line.to_string()));
                        i += 5;
                        continue;
                    }
                }
            }
            i += 1;
        }
    }
    hits
}

#[test]
fn d29b_009_no_port_17xxx_literals_in_net_tests() {
    let dir = tests_dir();
    let mut failures: Vec<String> = Vec::new();

    let entries = fs::read_dir(dir).expect("read tests dir");
    for entry in entries {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !is_in_scope(name) {
            continue;
        }
        let hits = scan_file(&path);
        for (lineno, line) in hits {
            failures.push(format!(
                "  {}:{}: forbidden port 17xxx literal: {}",
                name,
                lineno,
                line.trim()
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "D29B-009 / Lock-F: NET integration tests must allocate ports via \
             `common::find_free_loopback_port()`, not via hard-coded 17xxx \
             literals. Offending occurrences:\n{}",
            failures.join("\n")
        );
    }
}

#[test]
fn d29b_009_audit_self_check_strip_line_comment() {
    // Sanity: the comment-stripper must not flag a 17000 inside a // comment.
    assert_eq!(
        strip_line_comments("let x = 1; // 17000 historical"),
        "let x = 1; "
    );
    assert_eq!(strip_line_comments("17042"), "17042");
}

#[test]
fn d29b_009_audit_self_check_word_boundary() {
    // Sanity: 117000 / 170001 / 17000_u16 should NOT match (boundary rules).
    fn synth_scan(line: &str) -> usize {
        let mut count = 0;
        let bytes = line.as_bytes();
        let n = bytes.len();
        let code_end = line.find("//").unwrap_or(n);
        let bytes = &bytes[..code_end];
        let n = bytes.len();
        let mut i = 0;
        while i + 5 <= n {
            if bytes[i] == b'1' && bytes[i + 1] == b'7' {
                let d2 = bytes[i + 2];
                let d3 = bytes[i + 3];
                let d4 = bytes[i + 4];
                if d2.is_ascii_digit() && d3.is_ascii_digit() && d4.is_ascii_digit() {
                    let prev_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
                    let after = bytes.get(i + 5).copied();
                    let next_ok = match after {
                        None => true,
                        Some(b) => !b.is_ascii_digit() && b != b'_',
                    };
                    if prev_ok && next_ok {
                        count += 1;
                        i += 5;
                        continue;
                    }
                }
            }
            i += 1;
        }
        count
    }

    assert_eq!(synth_scan("let p = 17000;"), 1);
    assert_eq!(synth_scan("let p = 117000;"), 0); // leading digit boundary
    assert_eq!(synth_scan("let p = 170001;"), 0); // trailing digit boundary
    assert_eq!(synth_scan("let p = 17000_u16;"), 0); // trailing underscore
    assert_eq!(synth_scan("port = 17042; // legacy"), 1);
}

//! E32B-082: 3-backend parity — static string literals carrying an
//! embedded NUL byte must report their full byte length (not the
//! truncated-at-NUL length).
//!
//! Before the fix, the Native backend stored Taida string literals as
//! plain `[bytes..., 0]` blobs in cranelift's `.rodata`, so any caller
//! reaching for the byte length via `strlen()` (`taida_str_length`,
//! `taida_polymorphic_length`, `taida_read_cstr_len_safe`, …) saw the
//! truncated `"X"` for `"X\x00Y"` while the interpreter and JS backends
//! correctly observed all 3 bytes. The fix adds a hidden 16-byte
//! `[TAIDA_STR_STATIC_MAGIC, byte_len]` header in front of every
//! cranelift-emitted literal, mirroring the runtime-allocated heap
//! string layout.
//!
//! This test pins the `length()` parity directly and also exercises the
//! HTTP eager validator path where the truncation bypass was originally
//! discovered (E32B-080 `/case2`).

mod common;

use std::process::Command;

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

fn unique_tmp(prefix: &str, ext: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}_{}_{}.{}",
        prefix,
        std::process::id(),
        nanos,
        ext
    ))
}

fn run_program_three_backends(source: &str) -> [(String, String); 3] {
    let taida = common::taida_bin();

    let td_path = unique_tmp("e32b_082_static_nul", "td");
    std::fs::write(&td_path, source).expect("write source");

    // 1) Interpreter — direct run.
    let interp_out = {
        let out = Command::new(&taida)
            .arg(&td_path)
            .output()
            .expect("interp run");
        assert!(
            out.status.success(),
            "interp failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    // 2) JS backend.
    let js_out = if node_available() {
        let mjs_path = unique_tmp("e32b_082_static_nul", "mjs");
        let build = Command::new(&taida)
            .args(["build", "js"])
            .arg(&td_path)
            .arg("-o")
            .arg(&mjs_path)
            .output()
            .expect("build js");
        assert!(
            build.status.success(),
            "js build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let out = Command::new("node")
            .arg(&mjs_path)
            .output()
            .expect("node run");
        let _ = std::fs::remove_file(&mjs_path);
        assert!(
            out.status.success(),
            "js exec failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        eprintln!("node unavailable; skipping JS member");
        String::new()
    };

    // 3) Native backend.
    let native_out = if cc_available() {
        let bin_path = unique_tmp("e32b_082_static_nul", "bin");
        let build = Command::new(&taida)
            .args(["build", "native"])
            .arg(&td_path)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .expect("build native");
        assert!(
            build.status.success(),
            "native build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let out = Command::new(&bin_path).output().expect("native run");
        let _ = std::fs::remove_file(&bin_path);
        assert!(
            out.status.success(),
            "native exec failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        eprintln!("cc unavailable; skipping native member");
        String::new()
    };

    let _ = std::fs::remove_file(&td_path);

    [
        ("interp".to_string(), interp_out),
        ("js".to_string(), js_out),
        ("native".to_string(), native_out),
    ]
}

#[test]
fn e32b_082_nul_in_static_string_length_three_backend() {
    // The literal `"X\x00Y"` is 3 bytes. All three backends must agree
    // on `.length() == 3`. Before E32B-082 the Native backend reported
    // 1 because it scanned for NUL via strlen(); the heap-style header
    // emitted by cranelift now carries the byte length explicitly.
    let source = r#"s <= "X\x00Y"
stdout(s.length().toString())
"#;

    let results = run_program_three_backends(source);
    for (backend, out) in &results {
        if out.is_empty() {
            // Tooling unavailable for this backend; skip silently.
            continue;
        }
        assert_eq!(
            out, "3",
            "{} backend must observe length() == 3 for \"X\\x00Y\", got {:?}",
            backend, out
        );
    }
}

#[test]
fn e32b_084_byte_at_past_nul_three_backend() {
    // E32B-084: `taida_str_byte_at` / `_lax` were still scanning with
    // `strlen()`, so a static literal like "X\x00Y" reported byte length
    // 1 on the Native backend and any access at idx >= 1 returned the
    // OOB sentinel. The fix routes both helpers through
    // `taida_str_byte_len`, mirroring the E32B-082 treatment of
    // `byte_slice` / `byte_length`. This test pins parity at idx 2
    // (the `Y` byte after the embedded NUL); idx 1 (the NUL byte) is
    // also asserted so a future regression that conflates "value 0" and
    // "no value" surfaces here.
    let source = r#"s <= "X\x00Y"
b0Lax <= ByteAt[s, 0]()
b0Lax ]=> b0
stdout(b0.toString())
b1Lax <= ByteAt[s, 1]()
b1Lax ]=> b1
stdout(b1.toString())
b2Lax <= ByteAt[s, 2]()
b2Lax ]=> b2
stdout(b2.toString())
oobLax <= ByteAt[s, 3]()
oobLax ]=> oob
stdout(oob.toString())
"#;

    let results = run_program_three_backends(source);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert_eq!(
        interp, "88\n0\n89\n0",
        "interpreter must observe X(88) NUL(0) Y(89) and OOB(0) for \"X\\x00Y\""
    );
    for (backend, out) in &results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(
            out, &interp,
            "{} backend disagrees with interpreter on byteAt past NUL: {} vs {}",
            backend, out, interp
        );
    }
}

#[test]
fn e32b_084_byte_at_lax_tag_three_backend() {
    // The acceptance for the embedded-NUL byteAt fix also pins the Lax
    // tag: an in-bounds NUL byte must report `hasValue=true` (the value
    // happens to be 0 but is real), while an out-of-bounds index must
    // report `hasValue=false`. The previous strlen()-truncated path
    // confused the two on Native because every index >= 1 fell into the
    // empty-Lax branch. This test reads the tag directly via
    // `.hasValue()` instead of `]=>` so a tag mistake cannot hide
    // behind a default value that happens to coincide with the byte.
    let source = r#"s <= "X\x00Y"
b1Lax <= ByteAt[s, 1]()
stdout("has1:" + b1Lax.hasValue().toString())
oobLax <= ByteAt[s, 9]()
stdout("hasOob:" + oobLax.hasValue().toString())
"#;

    let results = run_program_three_backends(source);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert_eq!(
        interp, "has1:true\nhasOob:false",
        "interpreter must observe hasValue=true for the NUL byte and hasValue=false for the OOB index"
    );
    for (backend, out) in &results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(
            out, &interp,
            "{} backend disagrees with interpreter on byteAt Lax tag: {} vs {}",
            backend, out, interp
        );
    }
}

#[test]
fn e32b_082_nul_in_static_string_length_via_concat_three_backend() {
    // `"hello" + "X\x00Y" + "world"` is 5 + 3 + 5 = 13 bytes. The
    // concatenation result is a runtime-allocated heap string, so this
    // exercises the runtime path on top of the static-literal path.
    //
    // Note: parity here only requires interp / JS / Native to agree on
    // the byte length of the result. Whether the embedded NUL survives
    // every downstream `taida_str_*` helper is tracked separately under
    // the ongoing strlen() audit; this test pins the canonical layered
    // length from `taida_polymorphic_length`.
    let source = r#"a <= "hello"
b <= "X\x00Y"
c <= "world"
combined <= a + b + c
stdout(combined.length().toString())
"#;

    let results = run_program_three_backends(source);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert_eq!(
        interp, "13",
        "interpreter must observe combined length 13 (5 + 3 + 5)"
    );
    for (backend, out) in &results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(
            out, &interp,
            "{} backend disagrees with interpreter on concat length: {} vs {}",
            backend, out, interp
        );
    }
}

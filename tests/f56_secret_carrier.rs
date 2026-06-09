//! F56 sealed-carrier (`Moltenized[T]` / `Secret[T]`) diagnostics + fail-closed
//! runtime parity.
//!
//! Two layers are pinned here:
//!   1. The type checker rejects every leak sink at compile time — display
//!      (`[E1533]`), serialization (`[E1534]`), direct unmold (`[E1535]`), and
//!      binary ops / equality oracle (`[E1536]`). These run the interpreter
//!      entry point (`taida <FILE>`), the same flow as the other diagnostic
//!      suites.
//!   2. The runtime fails closed on all four backends even when type checking
//!      is skipped (`--no-check`): a sealed value never appears as plaintext on
//!      any path (display renders the policy label, `jsonEncode` emits `null`,
//!      direct unmold throws). The `compile_f56_secret_carrier.td` fixture pins
//!      the checked Redact → `"***"` output across backends via the parity gate;
//!      here we additionally pin the fail-closed behaviour under `--no-check`.

mod common;

use common::{node_available, taida_bin, unique_temp_dir, wasmtime_bin, write_file};
use std::fs;
use std::path::Path;
use std::process::Command;

/// A distinctive plaintext canary. It must never appear in any backend's output
/// for any sink — its presence anywhere is a leak.
const CANARY: &str = "S3CR3T_CANARY_must_never_print";

fn combined(output: &std::process::Output) -> String {
    let mut s = String::from_utf8_lossy(&output.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&output.stderr));
    s
}

/// Run `taida [--no-check] <FILE>` (interpreter) on a fixture in a fresh dir.
fn run_interp(label: &str, source: &str, no_check: bool) -> std::process::Output {
    let dir = unique_temp_dir(label);
    let src = dir.join("main.td");
    write_file(&src, source);
    let mut cmd = Command::new(taida_bin());
    if no_check {
        cmd.arg("--no-check");
    }
    let output = cmd.arg(&src).output().expect("run taida interpreter");
    let _ = fs::remove_dir_all(&dir);
    output
}

fn assert_code(source: &str, code: &str, label: &str) {
    let output = run_interp(label, source, false);
    assert!(
        !output.status.success(),
        "{label}: sealed-carrier sink must be rejected, but compiled cleanly\n{}",
        combined(&output)
    );
    let text = combined(&output);
    assert!(
        text.contains(code),
        "{label}: expected {code}, got:\n{text}"
    );
    assert!(
        !text.contains(CANARY),
        "{label}: canary leaked into a diagnostic message:\n{text}"
    );
}

// ── Layer 1: compile-time sink matrix ──────────────────────────────────────

#[test]
fn e1533_display_builtins_rejected() {
    for sink in ["stdout", "stderr", "debug"] {
        let src = format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\n{sink}(secret)\n");
        assert_code(&src, "[E1533]", &format!("e1533-{sink}"));
    }
}

#[test]
fn e1534_serialization_rejected() {
    for sink in ["jsonEncode", "jsonPretty"] {
        let src = format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout({sink}(secret))\n");
        assert_code(&src, "[E1534]", &format!("e1534-{sink}"));
    }
}

#[test]
fn e1535_direct_unmold_rejected() {
    // Both the forward (`>=>`) and backward (`<=<`) statement forms.
    assert_code(
        &format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nsecret >=> raw\n"),
        "[E1535]",
        "e1535-forward",
    );
    assert_code(
        &format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nraw <=< secret\n"),
        "[E1535]",
        "e1535-backward",
    );
}

#[test]
fn e1536_binary_ops_rejected() {
    // Concatenation would leak the value; equality is an oracle.
    assert_code(
        &format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(\"x\" + secret)\n"),
        "[E1536]",
        "e1536-concat",
    );
    assert_code(
        &format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(secret == secret)\n"),
        "[E1536]",
        "e1536-eq",
    );
}

#[test]
fn phase6_extended_display_sinks_rejected() {
    // Phase 6+: the display sinks the runtime already fails closed are now also
    // compile errors (lock L0-4): `.toString()`, `Str[]`, interpolation, and a
    // sealed carrier nested in a `@(...)` / `@[...]` literal reaching a sink.
    let p = format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\n");
    assert_code(
        &format!("{p}stdout(secret.toString())\n"),
        "[E1533]",
        "toString",
    );
    assert_code(
        &format!("{p}x <= Str[secret]()\nstdout(x)\n"),
        "[E1533]",
        "str-mold",
    );
    assert_code(
        &format!("{p}stdout(`a${{secret}}b`)\n"),
        "[E1533]",
        "interpolation",
    );
    assert_code(
        &format!("{p}stdout(jsonEncode(@(token <= secret, id <= 7)))\n"),
        "[E1534]",
        "nested-pack-json",
    );
    assert_code(
        &format!("{p}stdout(@(token <= secret))\n"),
        "[E1533]",
        "nested-pack-display",
    );
    assert_code(
        &format!("{p}stdout(jsonEncode(@[secret]))\n"),
        "[E1534]",
        "nested-list-json",
    );
}

#[test]
fn phase6_collection_membership_rejected() {
    // Phase 6+: `.contains(secret)` / `.indexOf(secret)` are equality oracles
    // (lock L0-4 collection sink) — now compile errors, not just runtime
    // fail-closed. Use ConstantTimeEq instead.
    let p = format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\n");
    assert_code(
        &format!("{p}stdout(@[secret].contains(secret))\n"),
        "[E1536]",
        "contains",
    );
    assert_code(
        &format!("{p}stdout(@[secret].indexOf(secret).toString())\n"),
        "[E1536]",
        "indexOf",
    );
}

#[test]
fn sealed_receiver_method_rejected_and_safe() {
    // Phase 6+ /so review: a sealed carrier exposes NO methods (the interpreter
    // rejects them all). A sealed *receiver* with a plain argument
    // (`secret.contains("x")`) slipped past the arg-only guard and made the
    // Native polymorphic dispatcher misread the carrier pack as a list (OOB
    // read). Now: a compile error on every backend, and the Native/WASM runtime
    // is fail-closed (no crash / OOB / leak) under --no-check.
    let p = format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\n");
    assert_code(
        &format!("{p}stdout(secret.contains(\"x\"))\n"),
        "[E1536]",
        "recv-contains",
    );
    assert_code(
        &format!("{p}stdout(secret.length().toString())\n"),
        "[E1533]",
        "recv-method",
    );

    // --no-check: the Native runtime must not crash / OOB / leak.
    let dir = unique_temp_dir("f56-recv");
    let src = format!("{p}stdout(secret.contains(\"x\").toString())\n");
    if let Some(out) = build_and_run(&dir, &src, "native") {
        assert!(
            !out.contains(CANARY),
            "native sealed-receiver method leaked:\n{out}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

// ── Layer 2: runtime fail-closed on the interpreter (reference) ─────────────

#[test]
fn interpreter_runtime_fail_closed() {
    // Display renders the policy label, never the sealed value.
    let disp = run_interp(
        "fc-display",
        &format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(secret)\n"),
        true,
    );
    let disp_t = combined(&disp);
    assert!(
        !disp_t.contains(CANARY),
        "display leaked under --no-check:\n{disp_t}"
    );
    assert!(
        disp_t.contains("<Secret>"),
        "display should render <Secret>:\n{disp_t}"
    );

    // jsonEncode emits `null`.
    let json = run_interp(
        "fc-json",
        &format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(jsonEncode(secret))\n"),
        true,
    );
    let json_t = combined(&json);
    assert!(
        !json_t.contains(CANARY),
        "jsonEncode leaked under --no-check:\n{json_t}"
    );
    assert!(
        json_t.contains("null"),
        "jsonEncode should emit null:\n{json_t}"
    );

    // Direct unmold throws (the inner value is never bound to `raw`).
    let unmold = run_interp(
        "fc-unmold",
        &format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nsecret >=> raw\nstdout(raw)\n"),
        true,
    );
    let unmold_t = combined(&unmold);
    assert!(
        !unmold_t.contains(CANARY),
        "unmold leaked under --no-check:\n{unmold_t}"
    );
}

#[test]
fn redact_returns_fixed_mask() {
    let out = run_interp(
        "redact",
        &format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(Redact[secret]())\n"),
        false,
    );
    let t = combined(&out);
    assert!(
        out.status.success(),
        "Redact program should compile + run:\n{t}"
    );
    assert_eq!(t.trim(), "***", "Redact must return the fixed mask");
    assert!(!t.contains(CANARY), "Redact leaked the sealed value:\n{t}");
}

#[test]
fn secret_aware_consumers_use_secret_without_leak() {
    // HmacSha256 / ConstantTimeEq consume a sealed secret directly. The MAC must
    // equal the HMAC computed over the plaintext key (proving the sealed bytes
    // reach the primitive), the comparison verdicts must be correct, and the
    // secret must never surface in the output.
    let src = format!(
        "secret <= MoltenizeSecret[\"{CANARY}\"]()\n\
         mac <= HmacSha256[secret, \"payload\"]()\n\
         stdout(mac)\n\
         same <= ConstantTimeEq[secret, \"{CANARY}\"]()\n\
         stdout(same.toString())\n\
         diff <= ConstantTimeEq[secret, \"wrong\"]()\n\
         stdout(diff.toString())\n"
    );
    let out = run_interp("consumers", &src, false);
    let t = combined(&out);
    assert!(out.status.success(), "consumer program should run:\n{t}");
    assert!(
        !t.contains(CANARY),
        "a consumer leaked the sealed secret:\n{t}"
    );

    let lines: Vec<&str> = t.lines().collect();
    // Cross-check the MAC against the public lowercase crypto over the plaintext.
    let xref = run_interp(
        "consumers-xref",
        &format!(
            ">>> taida-lang/crypto => @(hmacSha256)\nstdout(hmacSha256(\"{CANARY}\", \"payload\"))\n"
        ),
        false,
    );
    let xref_mac = combined(&xref).trim().to_string();
    assert_eq!(
        lines.first().copied(),
        Some(xref_mac.as_str()),
        "HmacSha256 over the sealed secret must equal the HMAC over its plaintext"
    );
    assert_eq!(
        lines.get(1).copied(),
        Some("true"),
        "ConstantTimeEq must match the equal candidate"
    );
    assert_eq!(
        lines.get(2).copied(),
        Some("false"),
        "ConstantTimeEq must reject the wrong candidate"
    );
}

#[test]
fn secret_aware_consumer_misuse_rejected_at_compile_time() {
    // review #4 (code-reviewer + /so Codex): the consumers' argument contract is
    // enforced by the checker ([E1506]), so a misuse is a *compile* error — the
    // same on all four backends — instead of a runtime split where interp/JS
    // reject but Native/WASM compute over raw bytes. Each case must be rejected
    // with [E1506] before any backend lowering runs.
    let cases = [
        // Non-sealed first argument (use the lowercase `hmacSha256` for plain inputs).
        "mac <= HmacSha256[\"plain-not-sealed\", \"msg\"]()\nstdout(mac)\n".to_string(),
        // Sealed value as the (non-secret) second argument.
        format!(
            "secret <= MoltenizeSecret[\"{CANARY}\"]()\n\
             b <= ConstantTimeEq[secret, secret]()\nstdout(b.toString())\n"
        ),
        // A Secret wrapping a non-byte payload (would diverge between Native/WASM).
        "secret <= MoltenizeSecret[Bytes[\"abc\"]()]()\n\
         mac <= HmacSha256[secret, \"m\"]()\nstdout(mac)\n"
            .to_string(),
    ];
    for (i, src) in cases.iter().enumerate() {
        let out = run_interp(&format!("consumer-misuse-{i}"), src, false);
        let t = combined(&out);
        assert!(
            !out.status.success(),
            "misuse #{i} should be rejected:\n{t}"
        );
        assert!(
            t.contains("[E1506]"),
            "misuse #{i} should emit [E1506]:\n{t}"
        );
        assert!(!t.contains(CANARY), "misuse #{i} leaked the canary:\n{t}");
    }
}

#[test]
fn consumer_non_sealed_first_arg_rejected_at_runtime_across_backends() {
    // F56-FB-002: under `--no-check` the checker [E1506] is bypassed, so the
    // *runtime* must reject a non-sealed first argument. The interpreter and JS
    // throw; Native/WASM previously passed the plain value straight to the crypto
    // primitive (MAC'd it) — a backend-parity break vs the reference interpreter.
    // Now every backend rejects.
    let src = "mac <= HmacSha256[\"plain-not-sealed\", \"msg\"]()\nstdout(mac)\n";

    // Reference: the interpreter rejects at runtime even with the checker off.
    let interp = run_interp("fb002-interp", src, true);
    let it = combined(&interp);
    assert!(
        !interp.status.success() && it.contains("sealed Secret"),
        "interp should reject the non-sealed first arg under --no-check:\n{it}"
    );

    // The compiled backends must match the reference: reject, never MAC the plain
    // value (a bare HMAC-SHA256 would surface as a 64-hex-char line).
    for profile in ["native", "wasm-wasi", "js"] {
        let dir = unique_temp_dir(&format!("fb002-{profile}"));
        if let Some(out) = build_and_run(&dir, src, profile) {
            let mac_leaked = out.lines().any(|l| {
                let t = l.trim();
                t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())
            });
            assert!(
                !mac_leaked,
                "{profile}: non-sealed first arg was MAC'd instead of rejected:\n{out}"
            );
            // native/interp/JS surface the message; WASM reports an uncaught throw
            // as a generic trap (the same as every other WASM `taida_throw`, e.g.
            // the unmold reject) — both are rejections, neither MACs the value.
            assert!(
                out.contains("sealed Secret")
                    || out.contains("TypeError")
                    || out.contains("Unhandled error")
                    || out.contains("trap"),
                "{profile}: non-sealed first arg should be rejected at runtime:\n{out}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn reveal_applies_consumer_to_plaintext() {
    // Reveal is the escape hatch: it hands the plaintext to a consumer and
    // returns the consumer's result. Here the consumer returns the secret's
    // length (not the secret), so the revealed value is genuinely used while
    // nothing leaks.
    let out = run_interp(
        "reveal",
        &format!(
            "secret <= MoltenizeSecret[\"{CANARY}\"]()\n\
             n <= Reveal[secret, _ s: Str = s.length()]()\n\
             stdout(n.toString())\n"
        ),
        false,
    );
    let t = combined(&out);
    assert!(out.status.success(), "Reveal program should run:\n{t}");
    assert_eq!(
        t.trim(),
        CANARY.chars().count().to_string(),
        "Reveal must apply the consumer to the revealed plaintext"
    );
    assert!(
        !t.contains(CANARY),
        "the consumer returned a length — nothing should leak:\n{t}"
    );
}

#[test]
fn reveal_rejects_non_secret() {
    let out = run_interp(
        "reveal-non-secret",
        "n <= Reveal[\"plain\", _ s: Str = s.length()]()\nstdout(n.toString())\n",
        false,
    );
    assert!(
        !out.status.success(),
        "Reveal must reject a non-secret first argument:\n{}",
        combined(&out)
    );
}

#[test]
fn secret_flow_audit_surfaces_reveal() {
    // Phase 5: `taida way verify --check secret-flow` surfaces every Reveal
    // de-seal point (the design's governance for the escape hatch), and a
    // consumer-only program (no Reveal) is clean.
    let dir = unique_temp_dir("secret-flow");

    let reveal_src = dir.join("reveal.td");
    write_file(
        &reveal_src,
        "secret <= MoltenizeSecret[\"k\"]()\n\
         n <= Reveal[secret, _ s: Str = s.length()]()\nstdout(n.toString())\n",
    );
    let out = Command::new(taida_bin())
        .args(["way", "verify", "--check", "secret-flow"])
        .arg(&reveal_src)
        .output()
        .expect("run way verify");
    let t = combined(&out);
    assert!(
        t.contains("secret-flow") && t.to_lowercase().contains("reveal"),
        "secret-flow must flag the Reveal de-seal point:\n{t}"
    );

    // A Reveal nested in a function body must also be surfaced (the walker
    // recurses into FuncDef / ClassLikeDef bodies, not just top-level).
    let nested_src = dir.join("nested.td");
    write_file(
        &nested_src,
        "useSecret s: Secret[Str] =\n  Reveal[s, _ p: Str = p.length()]()\n=> :Int\n\
         k <= MoltenizeSecret[\"x\"]()\nstdout(useSecret(k).toString())\n",
    );
    let out_nested = Command::new(taida_bin())
        .args(["way", "verify", "--check", "secret-flow"])
        .arg(&nested_src)
        .output()
        .expect("run way verify");
    assert!(
        combined(&out_nested).to_lowercase().contains("reveal"),
        "secret-flow must flag a Reveal nested in a function body:\n{}",
        combined(&out_nested)
    );

    let clean_src = dir.join("clean.td");
    write_file(
        &clean_src,
        "secret <= MoltenizeSecret[\"k\"]()\n\
         mac <= HmacSha256[secret, \"m\"]()\nstdout(mac)\n",
    );
    let out2 = Command::new(taida_bin())
        .args(["way", "verify", "--check", "secret-flow"])
        .arg(&clean_src)
        .output()
        .expect("run way verify");
    let t2 = combined(&out2);
    assert!(
        t2.contains("[PASS]") || t2.contains("0 warnings"),
        "a consumer-only program must pass secret-flow:\n{t2}"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── Layer 2 (cross-backend): no plaintext leak on any compiled backend ──────

/// Build `source` for `profile` into `out`, returning the captured run output
/// (stdout+stderr combined), or `None` if the toolchain for that profile is
/// unavailable so the caller can skip rather than fail.
fn build_and_run(dir: &Path, source: &str, profile: &str) -> Option<String> {
    let src = dir.join("main.td");
    write_file(&src, source);

    match profile {
        "native" => {
            let out = dir.join("prog");
            let build = Command::new(taida_bin())
                .args(["--no-check", "build", "native"])
                .arg(&src)
                .arg("-o")
                .arg(&out)
                .output()
                .ok()?;
            if !build.status.success() {
                return None; // no C toolchain → skip
            }
            let run = Command::new(&out).output().ok()?;
            Some(combined(&run))
        }
        "js" => {
            if !node_available() {
                return None;
            }
            let out = dir.join("prog.mjs");
            let build = Command::new(taida_bin())
                .args(["--no-check", "build", "js"])
                .arg(&src)
                .arg("-o")
                .arg(&out)
                .output()
                .ok()?;
            if !build.status.success() {
                return None;
            }
            let run = Command::new("node").arg(&out).output().ok()?;
            Some(combined(&run))
        }
        "wasm-wasi" => {
            let wasmtime = wasmtime_bin()?;
            let out = dir.join("prog.wasm");
            let build = Command::new(taida_bin())
                .args(["--no-check", "build", "wasm-wasi"])
                .arg(&src)
                .arg("-o")
                .arg(&out)
                .output()
                .ok()?;
            if !build.status.success() {
                return None;
            }
            let run = Command::new(wasmtime).arg(&out).output().ok()?;
            Some(combined(&run))
        }
        _ => None,
    }
}

#[test]
fn no_plaintext_leak_across_compiled_backends() {
    // Each sink, exercised with type checking skipped, must never surface the
    // canary on any backend whose toolchain is available.
    let sinks = [
        format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(secret)\n"),
        format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\ndebug(secret)\n"),
        format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(jsonEncode(secret))\n"),
        format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(jsonPretty(secret))\n"),
        format!(
            "secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(jsonEncode(@(token <= secret, id <= 7)))\n"
        ),
        format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nstdout(`x${{secret}}y`)\n"),
        format!("secret <= MoltenizeSecret[\"{CANARY}\"]()\nsecret >=> raw\nstdout(raw)\n"),
    ];

    let mut ran_any = false;
    for profile in ["native", "js", "wasm-wasi"] {
        for (i, sink) in sinks.iter().enumerate() {
            let dir = unique_temp_dir(&format!("f56-leak-{profile}-{i}"));
            if let Some(out) = build_and_run(&dir, sink, profile) {
                ran_any = true;
                assert!(
                    !out.contains(CANARY),
                    "{profile} leaked the sealed value for sink #{i}:\n{out}"
                );
            }
            let _ = fs::remove_dir_all(&dir);
        }
    }

    // At least the native backend should be exercisable in CI; if nothing ran,
    // surface that rather than passing vacuously.
    assert!(
        ran_any,
        "no compiled backend was available to exercise the fail-closed leak guards"
    );
}

#[test]
fn equality_is_fail_closed_across_backends() {
    // Same-value secrets must never reveal their match through equality, dedup,
    // hashing, or fingerprinting. The leak canary CANNOT catch this — no
    // plaintext prints; only the match bit (true/false, deduped-or-not) would
    // leak — so we pin each result against the interpreter reference (which is
    // fail-closed: a sealed carrier is non-equal and non-hashable).
    //
    // Two oracles slipped past every canary/parity gate and were caught only by
    // review: (1) JS `__taida_equals` structurally compared the plaintext
    // `__value` (top-level `==` → true); (2) native/wasm `taida_value_struct_eq`
    // / `_wasm_value_eq` + the hashable gate left `Unique`/`@[a]==@[b]` deduping
    // by content. This test exercises BOTH the direct and the collection paths.
    // Run under --no-check (every case is `[E1536]` in checked mode).
    // `a`/`b` are distinct same-value secrets; `c` is a different value.
    let setup = format!(
        "a <= MoltenizeSecret[\"{CANARY}\"]()\n\
         b <= MoltenizeSecret[\"{CANARY}\"]()\n\
         c <= MoltenizeSecret[\"different\"]()\n"
    );
    // (expression, expected interpreter-reference output). A sealed carrier is
    // never equal to ANYTHING — not another same-value carrier, not a
    // different-value one, not even itself — across every comparison entry
    // point (operator, dedup, membership, list/pack structural equality).
    let cases = [
        ("stdout(a == b)", "false"), // distinct, same value
        ("stdout(a == c)", "false"), // distinct, different value
        ("stdout(a == a)", "false"), // same object (identity must NOT leak through)
        ("stdout(a != b)", "true"),
        ("stdout(a != a)", "true"),
        ("stdout(Unique[@[a, b]]().length())", "2"), // dedup must not collapse
        ("stdout(Unique[@[a, a]]().length())", "2"), // even the same object
        ("stdout(@[a].contains(b))", "false"),
        ("stdout(@[a].contains(a))", "false"), // same object — identity must NOT match
        ("stdout(@[a, b].indexOf(b))", "-1"),  // membership by identity must not find it
        ("stdout(@[a, b].indexOf(a))", "-1"),  // same object too
        ("stdout(@[a] == @[b])", "false"),
        ("stdout(@(x <= a) == @(x <= b))", "false"),
    ];

    let mut ran_any = false;
    for (expr, expected) in cases {
        let src = format!("{setup}{expr}\n");

        let interp = combined(&run_interp("eq-ref", &src, true));
        assert_eq!(
            interp.trim(),
            expected,
            "interpreter reference for `{expr}` must be `{expected}`:\n{interp}"
        );
        assert!(
            !interp.contains(CANARY),
            "interpreter leaked on `{expr}`:\n{interp}"
        );

        for profile in ["native", "js", "wasm-wasi"] {
            let dir = unique_temp_dir(&format!("f56-eq-{profile}"));
            if let Some(out) = build_and_run(&dir, &src, profile) {
                ran_any = true;
                assert_eq!(
                    out.trim(),
                    expected,
                    "{profile}: `{expr}` must be `{expected}` — equality oracle open:\n{out}"
                );
                assert!(
                    !out.contains(CANARY),
                    "{profile} leaked on `{expr}`:\n{out}"
                );
            }
            let _ = fs::remove_dir_all(&dir);
        }
    }
    assert!(
        ran_any,
        "no compiled backend was available to exercise the equality-oracle guards"
    );
}

// ── F56 Phase 2: source-side secret producers ──────────────────────────────

#[test]
fn secret_source_producers_seal_at_boundary() {
    // MoltenizeSecretFromEnv reads an env var straight into a Secret[Str]: the
    // value is sealed at the boundary, so Redact masks it and the plaintext
    // never appears. The unwrapped value is a Secret and is display-rejected at
    // compile time.
    let dir = unique_temp_dir("f56-fromenv");
    let src = dir.join("main.td");
    write_file(
        &src,
        "MoltenizeSecretFromEnv[\"F56_TEST_SECRET\"]() >=> s\nstdout(Redact[s]())\n",
    );
    let masked = Command::new(taida_bin())
        .env("F56_TEST_SECRET", CANARY)
        .arg(&src)
        .output()
        .expect("run FromEnv");
    let masked_t = combined(&masked);
    assert_eq!(
        masked_t.trim(),
        "***",
        "FromEnv -> Redact must be ***:\n{masked_t}"
    );
    assert!(
        !masked_t.contains(CANARY),
        "FromEnv leaked the env secret:\n{masked_t}"
    );

    write_file(
        &src,
        "MoltenizeSecretFromEnv[\"F56_TEST_SECRET\"]() >=> s\nstdout(s)\n",
    );
    let rejected = Command::new(taida_bin())
        .env("F56_TEST_SECRET", CANARY)
        .arg(&src)
        .output()
        .expect("run FromEnv display");
    assert!(
        combined(&rejected).contains("[E1533]"),
        "the unwrapped FromEnv value is a Secret and must be display-rejected"
    );
    let _ = fs::remove_dir_all(&dir);

    // MoltenizeSecretFromFile reads a file's bytes into a Secret[Bytes]
    // (Async[Lax[Secret[Bytes]]], so two unmolds reach the carrier).
    let fdir = unique_temp_dir("f56-fromfile");
    let keyfile = fdir.join("key.bin");
    write_file(&keyfile, CANARY);
    let fromfile_src = format!(
        "MoltenizeSecretFromFile[\"{}\"]() >=> lx\nlx >=> sec\nstdout(Redact[sec]())\n",
        keyfile.display()
    );
    let fsrc = fdir.join("main.td");
    write_file(&fsrc, &fromfile_src);
    let fout = Command::new(taida_bin())
        .arg(&fsrc)
        .output()
        .expect("run FromFile");
    let ft = combined(&fout);
    assert_eq!(ft.trim(), "***", "FromFile -> Redact must be ***:\n{ft}");
    assert!(
        !ft.contains(CANARY),
        "FromFile leaked the file secret:\n{ft}"
    );

    // FromFile is also implemented on Native (Phase 6+): the same masked output,
    // no leak. (WASM / JS gate it with a capability error — design L0-5.)
    let ndir = unique_temp_dir("f56-fromfile-native");
    if let Some(nout) = build_and_run(&ndir, &fromfile_src, "native") {
        assert_eq!(
            nout.trim(),
            "***",
            "native FromFile -> Redact must be ***:\n{nout}"
        );
        assert!(!nout.contains(CANARY), "native FromFile leaked:\n{nout}");
    }
    let _ = fs::remove_dir_all(&ndir);
    let _ = fs::remove_dir_all(&fdir);
}

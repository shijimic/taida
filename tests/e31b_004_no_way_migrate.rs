mod common;

use common::taida_bin;
use std::process::Command;

#[test]
fn e31_way_help_does_not_list_migrate() {
    let output = Command::new(taida_bin())
        .args(["way", "--help"])
        .output()
        .expect("run taida way --help");

    assert!(
        output.status.success(),
        "way help should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("migrate"),
        "taida way help must not expose a migration hub: {}",
        stdout
    );
}

#[test]
fn e31_way_migrate_is_rejected_without_file_fallback() {
    for args in [
        &["way", "migrate"][..],
        &["way", "migrate", "--e30", "src"][..],
    ] {
        let output = Command::new(taida_bin())
            .args(args)
            .output()
            .unwrap_or_else(|_| panic!("run taida {}", args.join(" ")));

        assert_eq!(
            output.status.code(),
            Some(2),
            "taida {} should reject migration command: stdout={}, stderr={}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("[E1700]") && stderr.contains("taida way migrate"),
            "migration reject should name taida way migrate with E1700: {}",
            stderr
        );
        assert!(
            stderr.contains("does not provide AST migration tooling"),
            "migration reject should state no migration tooling exists: {}",
            stderr
        );
    }
}

#[test]
fn e31_upgrade_help_is_self_upgrade_only() {
    let output = Command::new(taida_bin())
        .args(["upgrade", "--help"])
        .output()
        .expect("run taida upgrade --help");

    assert!(
        output.status.success(),
        "upgrade help should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("taida upgrade [--check]"),
        "self-upgrade usage should remain: {}",
        stdout
    );
    for removed_usage in [
        "taida upgrade --d28",
        "taida upgrade --d29",
        "taida upgrade --e30",
    ] {
        assert!(
            !stdout.contains(removed_usage),
            "upgrade help must not list removed AST migration usage {}: {}",
            removed_usage,
            stdout
        );
    }
}

#[test]
fn e31_upgrade_ast_migration_flags_are_rejected() {
    for flag in ["--d28", "--d29", "--e30"] {
        let output = Command::new(taida_bin())
            .args(["upgrade", flag, "--help"])
            .output()
            .unwrap_or_else(|_| panic!("run taida upgrade {} --help", flag));

        assert_eq!(
            output.status.code(),
            Some(2),
            "taida upgrade {} should reject: stdout={}, stderr={}",
            flag,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("[E1700]") && stderr.contains(&format!("taida upgrade {}", flag)),
            "old migration flag should emit E1700 and name the flag: {}",
            stderr
        );
    }
}

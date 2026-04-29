mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::process::Command;

#[test]
fn e31_removed_top_level_commands_emit_e1700() {
    let cases = [
        ("check", "taida way check"),
        ("verify", "taida way verify"),
        ("lint", "taida way lint"),
        ("todo", "taida way todo"),
        ("inspect", "taida graph summary"),
        ("transpile", "taida build js"),
        ("compile", "taida build native"),
        ("deps", "taida ingot deps"),
        ("install", "taida ingot install"),
        ("update", "taida ingot update"),
        ("publish", "taida ingot publish"),
        ("cache", "taida ingot cache"),
        ("c", "taida community"),
    ];

    for (old, replacement) in cases {
        let output = Command::new(taida_bin())
            .arg(old)
            .output()
            .unwrap_or_else(|_| panic!("run taida {}", old));

        assert_eq!(
            output.status.code(),
            Some(2),
            "{} should exit 2, stdout={}, stderr={}",
            old,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("[E1700]"),
            "{} should emit E1700, got: {}",
            old,
            stderr
        );
        assert!(
            stderr.contains(&format!("Command '{}' was removed", old)),
            "{} should name removed command, got: {}",
            old,
            stderr
        );
        assert!(
            stderr.contains(replacement),
            "{} should suggest {}, got: {}",
            old,
            replacement,
            stderr
        );
    }
}

#[test]
fn e31_removed_command_reject_happens_before_file_fallback() {
    let dir = unique_temp_dir("e31_removed_command_fallback");
    write_file(&dir.join("check"), "stdout(\"should not run\")\n");

    let output = Command::new(taida_bin())
        .current_dir(&dir)
        .arg("check")
        .output()
        .expect("run taida check");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("[E1700]"),
        "stderr should contain E1700: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).is_empty(),
        "file fallback should not execute: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn e31_top_help_lists_new_surface_without_removed_commands() {
    let output = Command::new(taida_bin())
        .arg("--help")
        .output()
        .expect("run taida --help");

    assert!(
        output.status.success(),
        "--help should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "build",
        "way",
        "graph",
        "doc",
        "ingot",
        "init",
        "lsp",
        "auth",
        "community",
        "upgrade",
    ] {
        assert!(
            stdout.contains(expected),
            "top help should list {}, got: {}",
            expected,
            stdout
        );
    }

    for removed in [
        "\n  check",
        "\n  verify",
        "\n  lint",
        "\n  todo",
        "\n  inspect",
        "\n  transpile",
        "\n  compile",
        "\n  deps",
        "\n  install",
        "\n  update",
        "\n  publish",
        "\n  cache",
    ] {
        assert!(
            !stdout.contains(removed),
            "top help should not list removed command marker {:?}: {}",
            removed,
            stdout
        );
    }
}

#[test]
fn e31_new_hub_roots_print_help() {
    for (args, expected) in [
        (&["way", "--help"][..], "taida way check <PATH>"),
        (&["way"][..], "taida way check <PATH>"),
        (&["ingot", "--help"][..], "taida ingot install"),
        (&["ingot"][..], "taida ingot install"),
    ] {
        let output = Command::new(taida_bin())
            .args(args)
            .output()
            .unwrap_or_else(|_| panic!("run taida {}", args.join(" ")));

        assert!(
            output.status.success(),
            "taida {} should succeed: stderr={}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(expected),
            "taida {} should print hub help containing {}, got: {}",
            args.join(" "),
            expected,
            stdout
        );
    }
}

#[test]
fn e31_ingot_does_not_accept_package_install_form() {
    let output = Command::new(taida_bin())
        .args(["ingot", "taida-lang/net"])
        .output()
        .expect("run taida ingot taida-lang/net");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown subcommand for `taida ingot`: taida-lang/net"),
        "package-form ingot should be rejected as an unknown subcommand: {}",
        stderr
    );
    assert!(
        stderr.contains("taida ingot --help"),
        "stderr should point to ingot help: {}",
        stderr
    );
}

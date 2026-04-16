//! CLI help text, usage, and removed/deprecated command tests.
//!
//! RCB-29: Split from `todo_cli.rs` (1764 lines) into responsibility-based test files.

mod common;

use common::taida_bin;
use std::process::Command;

#[test]
fn test_compile_command_removed() {
    let output = Command::new(taida_bin())
        .arg("compile")
        .arg("dummy.td")
        .output()
        .expect("failed to run taida compile");

    assert!(
        !output.status.success(),
        "compile should fail with non-zero exit code"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`taida compile` has been removed"),
        "expected removal error, got: {}",
        stderr
    );
    assert!(
        stderr.contains("taida build --target native"),
        "expected migration hint, got: {}",
        stderr
    );
}

#[test]
fn test_transpile_command_help() {
    let output = Command::new(taida_bin())
        .arg("transpile")
        .arg("--help")
        .output()
        .expect("failed to run taida transpile --help");

    assert!(output.status.success(), "transpile --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("taida transpile"),
        "expected usage text, got: {}",
        stdout
    );
}

#[test]
fn test_top_level_help_prints_usage_and_commands() {
    let output = Command::new(taida_bin())
        .arg("--help")
        .output()
        .expect("failed to run taida --help");

    assert!(
        output.status.success(),
        "--help should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage:\n  taida [--no-check] <FILE>")
            && stdout.contains("Commands:")
            && stdout.contains("graph")
            && stdout.contains("upgrade")
            && stdout.contains("Global options:"),
        "unexpected help output: {}",
        stdout
    );
}

#[test]
fn test_subcommand_help_prints_usage_and_exits_zero() {
    let workdir = common::unique_temp_dir("taida_subcommand_help");
    let cases = [
        (&["check", "--help"][..], "taida check [--json] <PATH>"),
        (
            &["build", "--help"][..],
            "taida build [--target native|js|wasm-min|wasm-wasi|wasm-edge|wasm-full]",
        ),
        (
            &["todo", "--help"][..],
            "taida todo [--format text|json] [PATH]",
        ),
        (
            &["graph", "--help"][..],
            "taida graph [-o OUTPUT] [--recursive] <PATH>",
        ),
        (
            &["verify", "--help"][..],
            "taida verify [--check CHECK] [--format FORMAT] <PATH>",
        ),
        (
            &["inspect", "--help"][..],
            "taida inspect [--format text|json|sarif] <PATH>",
        ),
        (
            &["init", "--help"][..],
            "taida init [--target rust-addon] [DIR]",
        ),
        (&["deps", "--help"][..], "taida deps"),
        (&["install", "--help"][..], "taida install"),
        (&["update", "--help"][..], "taida update"),
        (
            &["publish", "--help"][..],
            "taida publish [--label LABEL] [--force-version VERSION] [--retag] [--dry-run]",
        ),
        (
            &["doc", "--help"][..],
            "taida doc generate [-o OUTPUT] <PATH>",
        ),
        (
            &["doc", "generate", "--help"][..],
            "taida doc generate [-o OUTPUT] <PATH>",
        ),
        (&["lsp", "--help"][..], "taida lsp"),
        (&["auth", "--help"][..], "taida auth <login|logout|status>"),
        (&["auth", "login", "--help"][..], "taida auth login"),
        (&["auth", "logout", "--help"][..], "taida auth logout"),
        (&["auth", "status", "--help"][..], "taida auth status"),
        (
            &["community", "--help"][..],
            "taida community <posts|post|messages|message|author>",
        ),
        (
            &["community", "posts", "--help"][..],
            "taida community posts [--tag <tag>] [--by <author>]",
        ),
        (
            &["community", "post", "--help"][..],
            "taida community post \"content\" [--tag <tag>...]",
        ),
        (
            &["community", "post", "hello", "--help"][..],
            "taida community post \"content\" [--tag <tag>...]",
        ),
        (
            &["community", "messages", "--help"][..],
            "taida community messages",
        ),
        (
            &["community", "message", "--help"][..],
            "taida community message --to <user> \"content\"",
        ),
        (
            &["community", "message", "--to", "alice", "hi", "--help"][..],
            "taida community message --to <user> \"content\"",
        ),
        (
            &["community", "author", "--help"][..],
            "taida community author [NAME]",
        ),
        (
            &["community", "author", "alice", "--help"][..],
            "taida community author [NAME]",
        ),
        (
            &["upgrade", "--help"][..],
            "taida upgrade [--check] [--gen GEN] [--label LABEL] [--version VERSION]",
        ),
    ];

    for (args, expected) in cases {
        let output = Command::new(taida_bin())
            .current_dir(&workdir)
            .args(args)
            .output()
            .unwrap_or_else(|_| panic!("failed to run {}", args.join(" ")));

        assert!(
            output.status.success(),
            "{} should succeed: stderr={}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(expected),
            "unexpected help output for {}: {}",
            args.join(" "),
            stdout
        );
    }

    assert!(
        !workdir.join("--help").exists(),
        "init --help must not create a directory named --help"
    );

    let _ = std::fs::remove_dir_all(&workdir);
}

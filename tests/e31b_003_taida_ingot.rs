mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::process::Command;

#[test]
fn e31_ingot_root_is_help_only_and_does_not_install() {
    let dir = unique_temp_dir("e31_ingot_root_help_only");
    write_file(&dir.join("packages.tdm"), "name <= \"demo-pkg\"\n");

    let output = Command::new(taida_bin())
        .current_dir(&dir)
        .arg("ingot")
        .output()
        .expect("run taida ingot");

    assert!(
        output.status.success(),
        "taida ingot should print help successfully: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("taida ingot install"),
        "root ingot help should list subcommands: {}",
        stdout
    );
    assert!(
        !dir.join(".taida").join("taida.lock").exists(),
        "bare taida ingot must not perform install side effects"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e31_ingot_subcommand_help_uses_new_surface() {
    for (args, expected) in [
        (&["ingot", "deps", "--help"][..], "taida ingot deps"),
        (&["ingot", "install", "--help"][..], "taida ingot install"),
        (&["ingot", "update", "--help"][..], "taida ingot update"),
        (&["ingot", "cache", "--help"][..], "taida ingot cache"),
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
            "taida {} help should contain {}, got: {}",
            args.join(" "),
            expected,
            stdout
        );
    }
}

#[test]
fn e31_ingot_publish_help_is_rehomed_or_feature_gated() {
    let output = Command::new(taida_bin())
        .args(["ingot", "publish", "--help"])
        .output()
        .expect("run taida ingot publish --help");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("taida ingot publish")
            || combined.contains("requires the 'community' feature"),
        "publish help should either show rehomed usage or the existing feature gate: {}",
        combined
    );
}

#[test]
fn e31_ingot_rejects_package_shorthand_without_manifest_write() {
    let dir = unique_temp_dir("e31_ingot_reject_package_form");
    write_file(&dir.join("packages.tdm"), "name <= \"demo-pkg\"\n");

    let output = Command::new(taida_bin())
        .current_dir(&dir)
        .args(["ingot", "taida-lang/net"])
        .output()
        .expect("run taida ingot taida-lang/net");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown subcommand for `taida ingot`: taida-lang/net"),
        "package shorthand should be rejected as an unknown subcommand: {}",
        stderr
    );
    assert!(
        !dir.join(".taida").join("taida.lock").exists(),
        "rejected package shorthand must not write lockfile"
    );

    let _ = fs::remove_dir_all(&dir);
}

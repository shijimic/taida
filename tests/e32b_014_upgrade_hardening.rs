//! E32B-014: self-upgrade supply-chain hardening.

use std::fs;

#[test]
fn e32b_014_upgrade_api_url_is_pinned() {
    unsafe {
        std::env::set_var("TAIDA_GITHUB_API_URL", "http://127.0.0.1:9998");
    }
    assert_eq!(taida::upgrade::api_url(), "https://api.github.com");
    unsafe {
        std::env::remove_var("TAIDA_GITHUB_API_URL");
    }
}

#[test]
fn e32b_014_missing_sha256sums_entry_rejects() {
    let err = taida::upgrade::expected_sha256_for_archive(
        "abc123  other.tar.gz\n",
        "taida-@e.1-x86_64-unknown-linux-gnu.tar.gz",
    )
    .expect_err("missing SHA256SUMS row must reject upgrade");
    assert!(
        err.contains("[E32K1_UPGRADE_NO_SHA256SUMS]")
            && err.contains("taida-@e.1-x86_64-unknown-linux-gnu.tar.gz"),
        "unexpected error: {err}"
    );
}

#[test]
fn e32b_014_upgrade_identity_is_taida_lang_taida_workflow() {
    assert_eq!(
        taida::upgrade::UPGRADE_COSIGN_IDENTITY_REGEXP,
        r"^https://github.com/taida-lang/taida/\.github/workflows/.+@refs/tags/.+$"
    );
}

#[test]
fn e32b_014_install_sh_identity_not_derived_from_taida_repo() {
    let install = fs::read_to_string("install.sh").expect("read install.sh");
    assert!(
        install.contains(
            "TAIDA_COSIGN_IDENTITY_REGEXP='^https://github.com/taida-lang/taida/\\.github/workflows/.+@refs/tags/.+$'"
        ),
        "installer must define a hard-coded taida-lang/taida workflow identity regex"
    );
    assert!(
        !install.contains("--certificate-identity-regexp \"^https://github.com/${TAIDA_REPO}/\""),
        "installer must not derive cosign identity from TAIDA_REPO"
    );
}

#[test]
fn e32b_014_upgrade_code_no_longer_reads_api_override_env() {
    let source = fs::read_to_string("src/upgrade.rs").expect("read src/upgrade.rs");
    let production_source = source
        .split("#[cfg(test)]")
        .next()
        .expect("upgrade source should have production section");
    assert!(
        !production_source.contains("std::env::var(\"TAIDA_GITHUB_API_URL\")"),
        "self-upgrade path must not read TAIDA_GITHUB_API_URL"
    );
}

#[test]
fn e32b_062_download_bytes_err_carries_code_prefix() {
    let err = taida::upgrade::download_bytes("file:///nonexistent/path/that/should/not/exist")
        .expect_err("missing file must fail");
    assert!(
        err.contains("[E32K1_UPGRADE_DOWNLOAD_FAILED]"),
        "download_bytes error must carry [E32K1_UPGRADE_DOWNLOAD_FAILED]: {err}"
    );
}

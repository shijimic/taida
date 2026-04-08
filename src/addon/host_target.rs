//! Host target detection for prebuild addon distribution.
//!
//! Uses `target_lexicon::HOST` to detect the current platform and maps
//! it to a canonical `HostTarget` enum.
//!
//! ## RC1.5 v1 (5 targets)
//!
//! - `x86_64-unknown-linux-gnu`
//! - `aarch64-unknown-linux-gnu`
//! - `x86_64-apple-darwin`
//! - `aarch64-apple-darwin`
//! - `x86_64-pc-windows-msvc`
//!
//! ## RC1.5 Nice-to-Have extensions (RC15B-003)
//!
//! - `x86_64-unknown-linux-musl` (Alpine Linux)
//! - `aarch64-unknown-linux-musl`
//! - `i686-unknown-linux-gnu` (32-bit Linux)
//! - `riscv64gc-unknown-linux-gnu`
//! - `x86_64-unknown-freebsd`
//!
//! Unknown targets are rejected with a deterministic error so that addon
//! binaries are never loaded against a platform the author did not sign
//! for. `HostTarget` is `#[non_exhaustive]` so new variants can be added
//! without breaking consumers that already `match` on the enum with a
//! catch-all arm.

use std::fmt;

/// Canonical host targets supported for prebuild addon distribution.
///
/// The enum is `#[non_exhaustive]`: adding a new variant is not a
/// breaking change, but every branch below (`as_triple`, `from_triple`,
/// `cdylib_ext`, `os_name`, `supported_targets`) must be updated in
/// lock-step. Tests pin the full set to catch accidental omissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostTarget {
    // RC1.5 v1 baseline (5 targets)
    X86_64LinuxGnu,
    Aarch64LinuxGnu,
    X86_64MacOs,
    Aarch64MacOs,
    X86_64Windows,

    // RC1.5 Nice-to-Have extensions (RC15B-003)
    X86_64LinuxMusl,
    Aarch64LinuxMusl,
    I686LinuxGnu,
    Riscv64LinuxGnu,
    X86_64FreeBsd,
}

impl HostTarget {
    /// Returns the canonical target triple string.
    pub fn as_triple(&self) -> &'static str {
        match self {
            Self::X86_64LinuxGnu => "x86_64-unknown-linux-gnu",
            Self::Aarch64LinuxGnu => "aarch64-unknown-linux-gnu",
            Self::X86_64MacOs => "x86_64-apple-darwin",
            Self::Aarch64MacOs => "aarch64-apple-darwin",
            Self::X86_64Windows => "x86_64-pc-windows-msvc",
            Self::X86_64LinuxMusl => "x86_64-unknown-linux-musl",
            Self::Aarch64LinuxMusl => "aarch64-unknown-linux-musl",
            Self::I686LinuxGnu => "i686-unknown-linux-gnu",
            Self::Riscv64LinuxGnu => "riscv64gc-unknown-linux-gnu",
            Self::X86_64FreeBsd => "x86_64-unknown-freebsd",
        }
    }

    /// Parse a triple string into a HostTarget. Only exact canonical forms are accepted.
    pub fn from_triple(triple: &str) -> Option<Self> {
        match triple {
            "x86_64-unknown-linux-gnu" => Some(Self::X86_64LinuxGnu),
            "aarch64-unknown-linux-gnu" => Some(Self::Aarch64LinuxGnu),
            "x86_64-apple-darwin" => Some(Self::X86_64MacOs),
            "aarch64-apple-darwin" => Some(Self::Aarch64MacOs),
            "x86_64-pc-windows-msvc" => Some(Self::X86_64Windows),
            "x86_64-unknown-linux-musl" => Some(Self::X86_64LinuxMusl),
            "aarch64-unknown-linux-musl" => Some(Self::Aarch64LinuxMusl),
            "i686-unknown-linux-gnu" => Some(Self::I686LinuxGnu),
            "riscv64gc-unknown-linux-gnu" => Some(Self::Riscv64LinuxGnu),
            "x86_64-unknown-freebsd" => Some(Self::X86_64FreeBsd),
            _ => None,
        }
    }

    /// Returns the cdylib extension for this target's OS.
    pub fn cdylib_ext(&self) -> &'static str {
        match self {
            Self::X86_64LinuxGnu
            | Self::Aarch64LinuxGnu
            | Self::X86_64LinuxMusl
            | Self::Aarch64LinuxMusl
            | Self::I686LinuxGnu
            | Self::Riscv64LinuxGnu
            | Self::X86_64FreeBsd => "so",
            Self::X86_64MacOs | Self::Aarch64MacOs => "dylib",
            Self::X86_64Windows => "dll",
        }
    }

    /// Returns the OS name for error messages.
    pub fn os_name(&self) -> &'static str {
        match self {
            Self::X86_64LinuxGnu
            | Self::Aarch64LinuxGnu
            | Self::X86_64LinuxMusl
            | Self::Aarch64LinuxMusl
            | Self::I686LinuxGnu
            | Self::Riscv64LinuxGnu => "linux",
            Self::X86_64MacOs | Self::Aarch64MacOs => "macos",
            Self::X86_64Windows => "windows",
            Self::X86_64FreeBsd => "freebsd",
        }
    }
}

/// Error when the host target is not supported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedHost {
    pub host_triple: String,
}

impl fmt::Display for UnsupportedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported host target '{}' (not available for prebuild addons)",
            self.host_triple
        )
    }
}

/// Returns all supported target triples in canonical order.
///
/// The first 5 entries are the RC1.5 v1 baseline targets; the remaining
/// entries are RC1.5 Nice-to-Have extensions (RC15B-003). The order is
/// preserved so help messages and error diagnostics are deterministic.
pub fn supported_targets() -> &'static [&'static str] {
    &[
        // v1 baseline
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        // RC15B-003 Nice-to-Have extensions
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-musl",
        "i686-unknown-linux-gnu",
        "riscv64gc-unknown-linux-gnu",
        "x86_64-unknown-freebsd",
    ]
}

/// Detects the current host target.
#[cfg(feature = "native")]
pub fn detect_host_target() -> Result<HostTarget, UnsupportedHost> {
    use std::sync::OnceLock;

    static HOST: OnceLock<Result<HostTarget, UnsupportedHost>> = OnceLock::new();

    HOST.get_or_init(|| {
        let triple = target_lexicon::HOST.to_string();
        HostTarget::from_triple(&triple).ok_or(UnsupportedHost {
            host_triple: triple,
        })
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_target_as_triple() {
        assert_eq!(
            HostTarget::X86_64LinuxGnu.as_triple(),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            HostTarget::Aarch64LinuxGnu.as_triple(),
            "aarch64-unknown-linux-gnu"
        );
        assert_eq!(HostTarget::X86_64MacOs.as_triple(), "x86_64-apple-darwin");
        assert_eq!(HostTarget::Aarch64MacOs.as_triple(), "aarch64-apple-darwin");
        assert_eq!(
            HostTarget::X86_64Windows.as_triple(),
            "x86_64-pc-windows-msvc"
        );
        // RC15B-003 extensions
        assert_eq!(
            HostTarget::X86_64LinuxMusl.as_triple(),
            "x86_64-unknown-linux-musl"
        );
        assert_eq!(
            HostTarget::Aarch64LinuxMusl.as_triple(),
            "aarch64-unknown-linux-musl"
        );
        assert_eq!(
            HostTarget::I686LinuxGnu.as_triple(),
            "i686-unknown-linux-gnu"
        );
        assert_eq!(
            HostTarget::Riscv64LinuxGnu.as_triple(),
            "riscv64gc-unknown-linux-gnu"
        );
        assert_eq!(
            HostTarget::X86_64FreeBsd.as_triple(),
            "x86_64-unknown-freebsd"
        );
    }

    #[test]
    fn host_target_from_triple() {
        assert_eq!(
            HostTarget::from_triple("x86_64-unknown-linux-gnu"),
            Some(HostTarget::X86_64LinuxGnu)
        );
        assert_eq!(
            HostTarget::from_triple("aarch64-unknown-linux-gnu"),
            Some(HostTarget::Aarch64LinuxGnu)
        );
        assert_eq!(
            HostTarget::from_triple("x86_64-apple-darwin"),
            Some(HostTarget::X86_64MacOs)
        );
        assert_eq!(
            HostTarget::from_triple("aarch64-apple-darwin"),
            Some(HostTarget::Aarch64MacOs)
        );
        assert_eq!(
            HostTarget::from_triple("x86_64-pc-windows-msvc"),
            Some(HostTarget::X86_64Windows)
        );
        // RC15B-003 extensions
        assert_eq!(
            HostTarget::from_triple("x86_64-unknown-linux-musl"),
            Some(HostTarget::X86_64LinuxMusl)
        );
        assert_eq!(
            HostTarget::from_triple("aarch64-unknown-linux-musl"),
            Some(HostTarget::Aarch64LinuxMusl)
        );
        assert_eq!(
            HostTarget::from_triple("i686-unknown-linux-gnu"),
            Some(HostTarget::I686LinuxGnu)
        );
        assert_eq!(
            HostTarget::from_triple("riscv64gc-unknown-linux-gnu"),
            Some(HostTarget::Riscv64LinuxGnu)
        );
        assert_eq!(
            HostTarget::from_triple("x86_64-unknown-freebsd"),
            Some(HostTarget::X86_64FreeBsd)
        );
    }

    #[test]
    fn host_target_from_triple_rejects_non_canonical() {
        assert_eq!(HostTarget::from_triple("arm64-apple-darwin"), None);
        assert_eq!(HostTarget::from_triple("x86_64-linux-gnu"), None);
        // riscv64 without the "gc" suffix is not the canonical triple.
        assert_eq!(HostTarget::from_triple("riscv64-unknown-linux-gnu"), None);
        // Unknown OS variants remain rejected.
        assert_eq!(HostTarget::from_triple("x86_64-unknown-openbsd"), None);
        assert_eq!(HostTarget::from_triple("aarch64-unknown-freebsd"), None);
    }

    #[test]
    fn host_target_cdylib_ext() {
        assert_eq!(HostTarget::X86_64LinuxGnu.cdylib_ext(), "so");
        assert_eq!(HostTarget::Aarch64LinuxGnu.cdylib_ext(), "so");
        assert_eq!(HostTarget::X86_64MacOs.cdylib_ext(), "dylib");
        assert_eq!(HostTarget::Aarch64MacOs.cdylib_ext(), "dylib");
        assert_eq!(HostTarget::X86_64Windows.cdylib_ext(), "dll");
        // RC15B-003 extensions: all non-darwin/non-windows targets use `so`.
        assert_eq!(HostTarget::X86_64LinuxMusl.cdylib_ext(), "so");
        assert_eq!(HostTarget::Aarch64LinuxMusl.cdylib_ext(), "so");
        assert_eq!(HostTarget::I686LinuxGnu.cdylib_ext(), "so");
        assert_eq!(HostTarget::Riscv64LinuxGnu.cdylib_ext(), "so");
        assert_eq!(HostTarget::X86_64FreeBsd.cdylib_ext(), "so");
    }

    #[test]
    fn host_target_os_name() {
        assert_eq!(HostTarget::X86_64LinuxGnu.os_name(), "linux");
        assert_eq!(HostTarget::X86_64LinuxMusl.os_name(), "linux");
        assert_eq!(HostTarget::Riscv64LinuxGnu.os_name(), "linux");
        assert_eq!(HostTarget::X86_64MacOs.os_name(), "macos");
        assert_eq!(HostTarget::X86_64Windows.os_name(), "windows");
        assert_eq!(HostTarget::X86_64FreeBsd.os_name(), "freebsd");
    }

    #[test]
    fn supported_targets_count() {
        // v1 baseline (5) + RC15B-003 extensions (5) = 10
        assert_eq!(supported_targets().len(), 10);
    }

    #[test]
    fn supported_targets_first_five_are_baseline() {
        // Pin the ordering: the first 5 entries must remain the v1 baseline.
        let all = supported_targets();
        assert_eq!(all[0], "x86_64-unknown-linux-gnu");
        assert_eq!(all[1], "aarch64-unknown-linux-gnu");
        assert_eq!(all[2], "x86_64-apple-darwin");
        assert_eq!(all[3], "aarch64-apple-darwin");
        assert_eq!(all[4], "x86_64-pc-windows-msvc");
    }

    #[cfg(feature = "native")]
    #[test]
    fn detect_host_target_returns_ok() {
        let result = detect_host_target();
        assert!(
            result.is_ok(),
            "detect_host_target should succeed on supported hosts, got: {:?}",
            result
        );
    }
}

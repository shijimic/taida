//! `native/addon.toml` parser and validator (RC1 Phase 4 -- `RC1-4a`).
//!
//! `.dev/RC1_DESIGN.md` Phase 4 Lock pins the boundary between the
//! Taida-language facade (`packages.tdm`) and the Native-only addon
//! manifest (`native/addon.toml`). This module owns the second half:
//! parsing and validating `addon.toml` files. It is intentionally a
//! **minimal hand-written TOML subset parser** so we don't pull a TOML
//! crate into the dependency tree (RC1 dep minimisation policy).
//!
//! # Accepted syntax
//!
//! ```toml
//! # Top-level required keys.
//! abi = 1
//! entry = "taida_addon_get_v1"
//! package = "taida-lang/addon-rs-sample"
//! library = "taida_addon_sample"
//!
//! # Optional. When omitted, the loader explicitly injects ["native"].
//! # The omitted form and an explicit `targets = ["native"]` parse to a
//! # bit-identical `AddonManifest`. Adding a non-`native` entry is a
//! # parse error today (currently the only valid entry is `"native"`);
//! # future generations may widen the allowlist, but never narrow it
//! # silently — see `docs/reference/addon_manifest.md` for the
//! # compatibility contract.
//! targets = ["native"]
//!
//! # Required function table. Maps declared function names -> arities.
//! [functions]
//! noop = 0
//! echo = 1
//! ```
//!
//! Anything outside this subset is rejected with a structured error so
//! authors get a clear failure mode rather than silent acceptance.
//!
//! # Validation contract (`RC1_DESIGN.md` Phase 4 Lock §Manifest boundary)
//!
//! 1. `abi` MUST equal [`taida_addon::TAIDA_ADDON_ABI_VERSION`] (currently `1`).
//! 2. `entry` MUST equal [`taida_addon::TAIDA_ADDON_ENTRY_SYMBOL`]
//!    (`"taida_addon_get_v1"`).
//! 3. `package` MUST be a non-empty string.
//! 4. `library` MUST be a non-empty string (the cdylib stem, no
//!    platform suffix).
//! 5. `[functions]` table MUST exist and contain at least one entry.
//! 6. Each function arity MUST be a non-negative integer.
//! 7. `targets`, when present, MUST be a non-empty array of strings
//!    drawn from the supported allowlist (currently `{"native"}`).
//!    When absent, the parser explicitly injects `vec!["native"]` so
//!    the omitted form and `targets = ["native"]` produce a
//!    bit-identical [`AddonManifest`].
//!
//! Any violation -> `AddonManifestError::*` with a deterministic
//! single-line `Display` for diagnostic routing.
//!
//! # Why hand-roll TOML?
//!
//! `addon.toml` is a **frozen v1 manifest**. The accepted shape is
//! described in five lines above. Pulling in a 30k-line TOML crate (and
//! its `serde` derive surface) for a five-line schema would invert the
//! cost/benefit ratio. The hand parser is ~150 lines, has no
//! dependencies, and rejects every shape outside the v1 schema with a
//! pinned error variant — exactly the property RC1 needs.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use taida_addon::{TAIDA_ADDON_ABI_VERSION, TAIDA_ADDON_ENTRY_SYMBOL};

/// D28B-021: closed allowlist for the top-level `targets` array.
///
/// The list is intentionally tiny so additions are visible review
/// events. `"native"` is the only currently shipping addon dispatch
/// target; `"wasm"` is reserved for the post-stable wasm dispatcher
/// (POST-STABLE-001) and must be added here in lock-step with the
/// dispatcher landing — silently accepting unknown entries would
/// erase the stable compatibility guarantee documented in
/// `docs/reference/addon_manifest.md`.
pub const SUPPORTED_ADDON_TARGETS: &[&str] = &["native"];

/// D28B-021: default value injected when `targets` is omitted.
///
/// Returning the same `Vec<String>` for both the absent and the
/// `targets = ["native"]` cases is the structural half of the
/// "bit-identical" contract — combined with sorted-key TOML emission
/// it lets `tests/d28b_021_*.rs` md5 the parsed manifest and assert
/// equality regardless of how the author wrote the source.
pub fn default_addon_targets() -> Vec<String> {
    vec!["native".to_string()]
}

// ── RC1.5: Prebuild distribution config ───────────────────────

/// Parsed `[library.prebuild]` section from `native/addon.toml`.
///
/// This is **optional** — addons without a prebuild section simply fall
/// back to the RC1 "developer-side `.so` manual placement" mode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrebuildConfig {
    /// URL template with `{version}`, `{target}`, `{ext}`, `{name}` variables.
    pub url_template: Option<String>,
    /// Target triple -> `sha256:<64-hex-string>` mapping.
    pub targets: HashMap<String, String>,
    /// RC15B-005: Optional GPG / detached signatures keyed by target triple.
    ///
    /// Reserved for future supply-chain hardening. In RC1.5 the signatures
    /// are **parsed and stored** but **not verified**: the field is
    /// preserved so that manifest authors can start including signatures
    /// today without tripping `unknown key` errors, and so that the
    /// forthcoming verifier has a stable place to read them from.
    ///
    /// Accepted value format is `"gpg:<opaque-identifier>"` — the opaque
    /// identifier is treated as an opaque token (URL, fingerprint, base64
    /// signature, ...). Non-`gpg:` prefixes are rejected at parse time
    /// so that the reserved namespace stays disjoint from future schemes
    /// such as `sigstore:`.
    pub signatures: HashMap<String, String>,
}

impl PrebuildConfig {
    /// Returns true if a prebuild URL template is configured.
    pub fn has_prebuild(&self) -> bool {
        self.url_template.is_some()
    }

    /// RC15B-005: Returns true if any target has a signature recorded.
    ///
    /// The signature field is reserved in RC1.5 and the verifier is not
    /// wired yet; callers can use this to gate future verification code
    /// paths without having to plumb through a separate flag.
    pub fn has_signatures(&self) -> bool {
        !self.signatures.is_empty()
    }
}

/// A parsed and validated `native/addon.toml` manifest.
///
/// Constructed via [`parse_addon_manifest`]. The struct is immutable
/// after construction so the import resolver can hand it around freely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonManifest {
    /// Source path the manifest was read from. Kept for diagnostics.
    pub manifest_path: PathBuf,
    /// `abi = 1` -- always [`TAIDA_ADDON_ABI_VERSION`] after validation.
    pub abi: u32,
    /// `entry = "taida_addon_get_v1"` -- always
    /// [`TAIDA_ADDON_ENTRY_SYMBOL`] after validation.
    pub entry: String,
    /// `package = "<org/name>"` canonical id. Must match the package
    /// the import resolver was looking up.
    pub package: String,
    /// `library = "<stem>"` cdylib filename stem (no platform suffix).
    pub library: String,
    /// `[functions]` table: function name -> declared arity.
    pub functions: BTreeMap<String, u32>,
    /// D28B-021: top-level `targets` array. Always populated after
    /// parsing — when the source omits `targets`, the parser injects
    /// [`default_addon_targets`] so the omitted form and an explicit
    /// `targets = ["native"]` produce a bit-identical struct.
    ///
    /// Entries are validated against [`SUPPORTED_ADDON_TARGETS`] at
    /// parse time; unknown entries are rejected with
    /// [`AddonManifestError::UnknownAddonTarget`] (`[E2001]`).
    pub targets: Vec<String>,
    /// RC1.5: `[library.prebuild]` section. `None` if absent (RC1 addon).
    pub prebuild: PrebuildConfig,
}

/// Errors produced when parsing or validating `native/addon.toml`.
///
/// Every variant carries the manifest path so diagnostics can route
/// back to the offending file. The `Display` impl uses a deterministic
/// `addon manifest error: ...` prefix that the import resolver pins on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddonManifestError {
    /// `addon.toml` could not be read from disk.
    ReadFailed { path: PathBuf, message: String },
    /// Lexer / parser failed (syntax outside the accepted subset).
    Syntax {
        path: PathBuf,
        line: usize,
        message: String,
    },
    /// Required top-level key missing.
    MissingKey { path: PathBuf, key: &'static str },
    /// `abi` value did not match [`TAIDA_ADDON_ABI_VERSION`].
    AbiUnsupported {
        path: PathBuf,
        expected: u32,
        actual: u32,
    },
    /// `entry` did not match [`TAIDA_ADDON_ENTRY_SYMBOL`].
    EntryMismatch {
        path: PathBuf,
        expected: &'static str,
        actual: String,
    },
    /// `package` was empty.
    MissingPackageId { path: PathBuf },
    /// `library` was empty.
    MissingLibrary { path: PathBuf },
    /// `[functions]` table was missing or empty.
    NoFunctions { path: PathBuf },
    /// A function entry's arity was not a non-negative integer.
    InvalidArity {
        path: PathBuf,
        function: String,
        raw: String,
    },
    /// A required key carried the wrong type.
    TypeMismatch {
        path: PathBuf,
        key: String,
        expected: &'static str,
    },
    /// RC1.5: `[library.prebuild]` url field is missing or empty.
    PrebuildMissingUrl { path: PathBuf },
    /// RC1.5: `sha256:` prefix validation failed (must be `sha256:` + 64 hex chars).
    PrebuildInvalidSha256 {
        path: PathBuf,
        target: String,
        value: String,
    },
    /// RC1.5: unknown URL template variable in `[library.prebuild].url`.
    PrebuildUnknownUrlVariable { path: PathBuf, variable: String },
    /// RC1.5: unbalanced brace in `[library.prebuild].url` template.
    PrebuildUnbalancedBrace { path: PathBuf, detail: String },
    /// RC1.5: duplicate `[library.prebuild.targets.<target>]` for the same target.
    PrebuildDuplicateTarget { path: PathBuf, target: String },
    /// RC15B-103: target triple in `[library.prebuild.targets]` is not a known valid triple.
    PrebuildUnknownTarget { path: PathBuf, target: String },
    /// RC15B-005: `[library.prebuild.signatures]` entry is not in the
    /// reserved `gpg:<opaque>` format. Future schemes (e.g. `sigstore:`)
    /// should be added as a new variant rather than loosened here.
    PrebuildInvalidSignatureFormat {
        path: PathBuf,
        target: String,
        value: String,
    },
    /// RC15B-005: signature target triple is not a known valid triple.
    PrebuildSignatureUnknownTarget { path: PathBuf, target: String },
    /// RC15B-005: duplicate `[library.prebuild.signatures.<target>]` for the same target.
    PrebuildDuplicateSignatureTarget { path: PathBuf, target: String },
    /// D28B-021 (`[E2001]`): top-level `targets` contains an entry
    /// that is not in [`SUPPORTED_ADDON_TARGETS`].
    ///
    /// The error is raised early — before any dispatcher is consulted —
    /// so unknown targets cannot silently fall back to a default. The
    /// `target` field carries the offending string verbatim (case
    /// preserved) for the diagnostic.
    UnknownAddonTarget { path: PathBuf, target: String },
    /// D28B-021 (`[E2002]`): top-level `targets` was present but the
    /// array was empty (`targets = []`). An empty array is rejected
    /// rather than silently treated as the default — silent treatment
    /// would let an author opt out of the contract by writing `[]`.
    EmptyAddonTargets { path: PathBuf },
    /// D28B-021: top-level `targets` was the wrong type (e.g. a string
    /// or integer rather than an array of strings).
    AddonTargetsTypeMismatch { path: PathBuf, detail: String },
}

impl fmt::Display for AddonManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFailed { path, message } => write!(
                f,
                "addon manifest error: cannot read '{}': {}",
                path.display(),
                message
            ),
            Self::Syntax {
                path,
                line,
                message,
            } => write!(
                f,
                "addon manifest error: syntax error in '{}' at line {}: {}",
                path.display(),
                line,
                message
            ),
            Self::MissingKey { path, key } => write!(
                f,
                "addon manifest error: required key '{}' missing in '{}'",
                key,
                path.display()
            ),
            Self::AbiUnsupported {
                path,
                expected,
                actual,
            } => write!(
                f,
                "addon manifest error: unsupported abi {} in '{}' (expected {})",
                actual,
                path.display(),
                expected
            ),
            Self::EntryMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "addon manifest error: entry symbol '{}' in '{}' does not match expected '{}'",
                actual,
                path.display(),
                expected
            ),
            Self::MissingPackageId { path } => write!(
                f,
                "addon manifest error: 'package' must be a non-empty string in '{}'",
                path.display()
            ),
            Self::MissingLibrary { path } => write!(
                f,
                "addon manifest error: 'library' must be a non-empty string in '{}'",
                path.display()
            ),
            Self::NoFunctions { path } => write!(
                f,
                "addon manifest error: '[functions]' table must declare at least one function in '{}'",
                path.display()
            ),
            Self::InvalidArity {
                path,
                function,
                raw,
            } => write!(
                f,
                "addon manifest error: function '{}' has invalid arity '{}' in '{}'",
                function,
                raw,
                path.display()
            ),
            Self::TypeMismatch {
                path,
                key,
                expected,
            } => write!(
                f,
                "addon manifest error: key '{}' in '{}' must be {}",
                key,
                path.display(),
                expected
            ),
            Self::PrebuildMissingUrl { path } => write!(
                f,
                "addon manifest error: '[library.prebuild].url' is required when [library.prebuild] is present in '{}'",
                path.display()
            ),
            Self::PrebuildInvalidSha256 {
                path,
                target,
                value,
            } => write!(
                f,
                "addon manifest error: invalid sha256 for target '{}' in '{}': expected 'sha256:' prefix + 64 lowercase hex chars, got '{}'",
                target,
                path.display(),
                value
            ),
            Self::PrebuildUnknownUrlVariable { path, variable } => write!(
                f,
                "addon manifest error: unknown url template variable '{{{}}}' in '[library.prebuild].url' of '{}'",
                variable,
                path.display()
            ),
            Self::PrebuildUnbalancedBrace { path, detail } => write!(
                f,
                "addon manifest error: unbalanced brace in '[library.prebuild].url' of '{}': {}",
                path.display(),
                detail
            ),
            Self::PrebuildDuplicateTarget { path, target } => write!(
                f,
                "addon manifest error: duplicate prebuild target '{}' in '{}'",
                target,
                path.display()
            ),
            Self::PrebuildUnknownTarget { path, target } => write!(
                f,
                "addon manifest error: unknown target triple '{}' in '{}' (must be a canonical triple like 'x86_64-unknown-linux-gnu')",
                target,
                path.display()
            ),
            Self::PrebuildInvalidSignatureFormat {
                path,
                target,
                value,
            } => write!(
                f,
                "addon manifest error: invalid signature for target '{}' in '{}': expected 'gpg:' prefix + opaque identifier, got '{}'",
                target,
                path.display(),
                value
            ),
            Self::PrebuildSignatureUnknownTarget { path, target } => write!(
                f,
                "addon manifest error: unknown target triple '{}' in '[library.prebuild.signatures]' of '{}' (must be a canonical triple)",
                target,
                path.display()
            ),
            Self::PrebuildDuplicateSignatureTarget { path, target } => write!(
                f,
                "addon manifest error: duplicate prebuild signature target '{}' in '{}'",
                target,
                path.display()
            ),
            Self::UnknownAddonTarget { path, target } => write!(
                f,
                "addon manifest error: [E2001] unknown addon target '{}' in '{}' (supported: {})",
                target,
                path.display(),
                SUPPORTED_ADDON_TARGETS.join(", ")
            ),
            Self::EmptyAddonTargets { path } => write!(
                f,
                "addon manifest error: [E2002] 'targets' must be a non-empty array in '{}' (omit the key to accept the default {:?})",
                path.display(),
                SUPPORTED_ADDON_TARGETS
            ),
            Self::AddonTargetsTypeMismatch { path, detail } => write!(
                f,
                "addon manifest error: 'targets' in '{}' must be an array of strings ({})",
                path.display(),
                detail
            ),
        }
    }
}

impl std::error::Error for AddonManifestError {}

/// Parse and validate `path` as an `addon.toml` v1 manifest.
///
/// Returns a fully-validated [`AddonManifest`] or an
/// [`AddonManifestError`] tagged with the source path. The function is
/// pure / read-only: it does not touch the filesystem beyond reading
/// the manifest file.
pub fn parse_addon_manifest(path: &Path) -> Result<AddonManifest, AddonManifestError> {
    let source = std::fs::read_to_string(path).map_err(|e| AddonManifestError::ReadFailed {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    parse_addon_manifest_str(path, &source)
}

/// Same as [`parse_addon_manifest`] but operates on an already-loaded
/// source string. Used by unit tests to avoid the filesystem.
pub fn parse_addon_manifest_str(
    path: &Path,
    source: &str,
) -> Result<AddonManifest, AddonManifestError> {
    let raw = parse_minimal_toml(path, source)?;

    // Validate top-level required keys.
    let abi = require_int(&raw.top_level, "abi", path)?;
    if (abi as u32) != TAIDA_ADDON_ABI_VERSION {
        return Err(AddonManifestError::AbiUnsupported {
            path: path.to_path_buf(),
            expected: TAIDA_ADDON_ABI_VERSION,
            actual: abi as u32,
        });
    }
    if abi < 0 {
        return Err(AddonManifestError::AbiUnsupported {
            path: path.to_path_buf(),
            expected: TAIDA_ADDON_ABI_VERSION,
            actual: 0,
        });
    }

    let entry = require_str(&raw.top_level, "entry", path)?;
    if entry != TAIDA_ADDON_ENTRY_SYMBOL {
        return Err(AddonManifestError::EntryMismatch {
            path: path.to_path_buf(),
            expected: TAIDA_ADDON_ENTRY_SYMBOL,
            actual: entry,
        });
    }

    let package = require_str(&raw.top_level, "package", path)?;
    if package.trim().is_empty() {
        return Err(AddonManifestError::MissingPackageId {
            path: path.to_path_buf(),
        });
    }

    let library = require_str(&raw.top_level, "library", path)?;
    if library.trim().is_empty() {
        return Err(AddonManifestError::MissingLibrary {
            path: path.to_path_buf(),
        });
    }

    // D28B-021: validate top-level `targets`. The parser explicitly
    // injects [`default_addon_targets`] when the key is absent so the
    // omitted form and an explicit `targets = ["native"]` produce a
    // bit-identical [`AddonManifest`].
    let targets = match raw.top_level.get("targets") {
        None => default_addon_targets(),
        Some(RawValue::StrArray(items)) => {
            if items.is_empty() {
                return Err(AddonManifestError::EmptyAddonTargets {
                    path: path.to_path_buf(),
                });
            }
            for entry in items {
                if !SUPPORTED_ADDON_TARGETS.contains(&entry.as_str()) {
                    return Err(AddonManifestError::UnknownAddonTarget {
                        path: path.to_path_buf(),
                        target: entry.clone(),
                    });
                }
            }
            // Deterministic order: keep author-given order, but
            // collapse duplicate entries so the bit-identical contract
            // covers `targets = ["native", "native"]` too.
            let mut seen = std::collections::BTreeSet::new();
            let mut deduped = Vec::with_capacity(items.len());
            for entry in items {
                if seen.insert(entry.clone()) {
                    deduped.push(entry.clone());
                }
            }
            deduped
        }
        Some(other) => {
            return Err(AddonManifestError::AddonTargetsTypeMismatch {
                path: path.to_path_buf(),
                detail: format!("got {}", other.kind_label()),
            });
        }
    };

    // Validate [functions] table.
    let functions_raw = raw
        .functions
        .ok_or_else(|| AddonManifestError::NoFunctions {
            path: path.to_path_buf(),
        })?;
    if functions_raw.is_empty() {
        return Err(AddonManifestError::NoFunctions {
            path: path.to_path_buf(),
        });
    }
    let mut functions: BTreeMap<String, u32> = BTreeMap::new();
    for (fn_name, fn_value) in functions_raw {
        match fn_value {
            RawValue::Int(n) => {
                if n < 0 {
                    return Err(AddonManifestError::InvalidArity {
                        path: path.to_path_buf(),
                        function: fn_name,
                        raw: n.to_string(),
                    });
                }
                functions.insert(fn_name, n as u32);
            }
            other => {
                return Err(AddonManifestError::InvalidArity {
                    path: path.to_path_buf(),
                    function: fn_name,
                    raw: other.kind_label().to_string(),
                });
            }
        }
    }

    // Validate and build prebuild config (RC1.5).
    let prebuild_section_present = !raw.prebuild.is_empty()
        || !raw.prebuild_targets.is_empty()
        || !raw.prebuild_signatures.is_empty();
    let prebuild = if !prebuild_section_present {
        PrebuildConfig::default()
    } else {
        // If either prebuild section exists, validate the URL template.
        let url_template = match raw.prebuild.get("url") {
            Some(RawValue::Str(s)) => {
                if s.trim().is_empty() {
                    return Err(AddonManifestError::PrebuildMissingUrl {
                        path: path.to_path_buf(),
                    });
                }
                // Validate URL template variables at parse time.
                match crate::addon::url_template::validate_template(s) {
                    Ok(()) => Some(s.clone()),
                    Err(crate::addon::url_template::UrlTemplateError::UnknownVariable {
                        variable,
                    }) => {
                        return Err(AddonManifestError::PrebuildUnknownUrlVariable {
                            path: path.to_path_buf(),
                            variable,
                        });
                    }
                    Err(crate::addon::url_template::UrlTemplateError::UnbalancedBrace {
                        detail,
                    }) => {
                        return Err(AddonManifestError::PrebuildUnbalancedBrace {
                            path: path.to_path_buf(),
                            detail,
                        });
                    }
                }
            }
            Some(_) => {
                return Err(AddonManifestError::TypeMismatch {
                    path: path.to_path_buf(),
                    key: "url".to_string(),
                    expected: "string",
                });
            }
            None => {
                return Err(AddonManifestError::PrebuildMissingUrl {
                    path: path.to_path_buf(),
                });
            }
        };

        // Validate [library.prebuild.targets] entries.
        let mut targets: HashMap<String, String> = HashMap::new();
        for (target_triple, sha_value) in raw.prebuild_targets {
            // RC15B-103: Validate target triple against known supported targets.
            if crate::addon::host_target::HostTarget::from_triple(&target_triple).is_none() {
                return Err(AddonManifestError::PrebuildUnknownTarget {
                    path: path.to_path_buf(),
                    target: target_triple,
                });
            }
            match sha_value {
                RawValue::Str(s) => {
                    if !is_valid_sha256(&s) {
                        return Err(AddonManifestError::PrebuildInvalidSha256 {
                            path: path.to_path_buf(),
                            target: target_triple,
                            value: s.clone(),
                        });
                    }
                    if targets.contains_key(&target_triple) {
                        return Err(AddonManifestError::PrebuildDuplicateTarget {
                            path: path.to_path_buf(),
                            target: target_triple,
                        });
                    }
                    targets.insert(target_triple, s);
                }
                _ => {
                    return Err(AddonManifestError::TypeMismatch {
                        path: path.to_path_buf(),
                        key: format!("targets.{}", target_triple),
                        expected: "string",
                    });
                }
            }
        }

        // RC15B-005: Validate [library.prebuild.signatures] entries.
        //
        // Signatures are **reserved**: parsed and stored but not verified
        // in RC1.5. Each entry must be keyed by a canonical target triple
        // and carry a `"gpg:<opaque>"` value. Other schemes (e.g.
        // `sigstore:`) are rejected so the `gpg:` namespace stays clean
        // for a future verifier without accidentally colliding with
        // formats added ad-hoc by addon authors.
        let mut signatures: HashMap<String, String> = HashMap::new();
        for (sig_target, sig_value) in raw.prebuild_signatures {
            if crate::addon::host_target::HostTarget::from_triple(&sig_target).is_none() {
                return Err(AddonManifestError::PrebuildSignatureUnknownTarget {
                    path: path.to_path_buf(),
                    target: sig_target,
                });
            }
            match sig_value {
                RawValue::Str(s) => {
                    if !is_valid_signature(&s) {
                        return Err(AddonManifestError::PrebuildInvalidSignatureFormat {
                            path: path.to_path_buf(),
                            target: sig_target,
                            value: s.clone(),
                        });
                    }
                    if signatures.contains_key(&sig_target) {
                        return Err(AddonManifestError::PrebuildDuplicateSignatureTarget {
                            path: path.to_path_buf(),
                            target: sig_target,
                        });
                    }
                    signatures.insert(sig_target, s);
                }
                _ => {
                    return Err(AddonManifestError::TypeMismatch {
                        path: path.to_path_buf(),
                        key: format!("signatures.{}", sig_target),
                        expected: "string",
                    });
                }
            }
        }

        PrebuildConfig {
            url_template,
            targets,
            signatures,
        }
    };

    Ok(AddonManifest {
        manifest_path: path.to_path_buf(),
        abi: abi as u32,
        entry,
        package,
        library,
        functions,
        targets,
        prebuild,
    })
}

// ── Minimal TOML subset parser ────────────────────────────────

/// Internal representation of a parsed `addon.toml`. Holds top-level
/// keys, the `[functions]` table, and optional `[library.prebuild]`
/// with its child `[library.prebuild.targets]` table (RC1.5).
/// Anything else triggers a syntax error so the schema stays pinned.
///
/// ## RC15B-107: Unknown-key forward-compatibility policy
///
/// The parser is **strict**: any section or top-level key not listed in
/// [`parse_minimal_toml`]'s section table is rejected with a
/// `Syntax` error. This is intentional — it prevents ABI drift by
/// making schema changes visible at install time rather than silently
/// tolerating stray keys that would become load-bearing in a future RC.
///
/// Forward-compat for addon authors:
///   1. Adding a new section is an **ABI bump** and should coincide with
///      a new manifest schema version (future work).
///   2. Adding a new optional key inside an existing reserved section
///      (e.g. `[library.prebuild.signatures]` added in RC15B-005) must
///      only be done after updating this parser and bumping the host's
///      minimum supported tool version.
///   3. Manifests written by a *newer* taida will fail to load on older
///      taidas that do not know the new key — this is by design. Authors
///      should document the minimum `taida` version in their README.
#[derive(Debug, Default)]
struct ParsedToml {
    top_level: BTreeMap<String, RawValue>,
    functions: Option<BTreeMap<String, RawValue>>,
    // RC1.5: [library.prebuild] key-value pairs
    prebuild: BTreeMap<String, RawValue>,
    // RC1.5: [library.prebuild.targets] key-value pairs
    prebuild_targets: BTreeMap<String, RawValue>,
    // RC15B-005: [library.prebuild.signatures] key-value pairs (reserved)
    prebuild_signatures: BTreeMap<String, RawValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawValue {
    Int(i64),
    Str(String),
    /// D28B-021: inline array of strings, used only for the top-level
    /// `targets = ["native", ...]` key. Other keys still reject arrays
    /// via [`require_str`] / [`require_int`] type checks.
    StrArray(Vec<String>),
}

impl RawValue {
    fn kind_label(&self) -> &'static str {
        match self {
            RawValue::Int(_) => "integer",
            RawValue::Str(_) => "string",
            RawValue::StrArray(_) => "array of strings",
        }
    }
}

fn require_str(
    map: &BTreeMap<String, RawValue>,
    key: &'static str,
    path: &Path,
) -> Result<String, AddonManifestError> {
    match map.get(key) {
        Some(RawValue::Str(s)) => Ok(s.clone()),
        Some(_) => Err(AddonManifestError::TypeMismatch {
            path: path.to_path_buf(),
            key: key.to_string(),
            expected: "string",
        }),
        None => Err(AddonManifestError::MissingKey {
            path: path.to_path_buf(),
            key,
        }),
    }
}

fn require_int(
    map: &BTreeMap<String, RawValue>,
    key: &'static str,
    path: &Path,
) -> Result<i64, AddonManifestError> {
    match map.get(key) {
        Some(RawValue::Int(n)) => Ok(*n),
        Some(_) => Err(AddonManifestError::TypeMismatch {
            path: path.to_path_buf(),
            key: key.to_string(),
            expected: "integer",
        }),
        None => Err(AddonManifestError::MissingKey {
            path: path.to_path_buf(),
            key,
        }),
    }
}

fn parse_minimal_toml(path: &Path, source: &str) -> Result<ParsedToml, AddonManifestError> {
    let mut parsed = ParsedToml::default();
    let mut current_section: Option<String> = None;

    for (line_idx, raw_line) in source.lines().enumerate() {
        let line_no = line_idx + 1;
        let line = raw_line.trim();

        // Skip blank lines and comments.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Section header.
        if let Some(stripped) = line.strip_prefix('[') {
            let header = stripped
                .strip_suffix(']')
                .ok_or_else(|| AddonManifestError::Syntax {
                    path: path.to_path_buf(),
                    line: line_no,
                    message: "section header missing ']'".to_string(),
                })?;
            let header = header.trim();
            match header {
                "functions" => {
                    if parsed.functions.is_some() {
                        return Err(AddonManifestError::Syntax {
                            path: path.to_path_buf(),
                            line: line_no,
                            message: "[functions] section declared more than once".to_string(),
                        });
                    }
                    parsed.functions = Some(BTreeMap::new());
                    current_section = Some("functions".to_string());
                }
                "library.prebuild" => {
                    // RC1.5: optional prebuild section
                    current_section = Some("library.prebuild".to_string());
                }
                "library.prebuild.targets" => {
                    // RC1.5: target -> sha256 mapping
                    current_section = Some("library.prebuild.targets".to_string());
                }
                "library.prebuild.signatures" => {
                    // RC15B-005: target -> gpg:<opaque> reserved section
                    current_section = Some("library.prebuild.signatures".to_string());
                }
                other => {
                    return Err(AddonManifestError::Syntax {
                        path: path.to_path_buf(),
                        line: line_no,
                        message: format!(
                            "unknown section '[{}]' (only [functions], [library.prebuild], [library.prebuild.targets], [library.prebuild.signatures] are allowed)",
                            other
                        ),
                    });
                }
            }
            continue;
        }

        // Key = value (strip inline comments after value).
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: "expected 'key = value' or '[section]'".to_string(),
            })?;
        let key = key.trim();
        let value = strip_inline_comment(value).trim();

        // Support simple quoted keys: "key name" -> key name
        let key = if key.starts_with('"') && key.ends_with('"') && key.len() >= 2 {
            &key[1..key.len() - 1]
        } else {
            key
        };

        if key.is_empty() {
            return Err(AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: "empty key before '='".to_string(),
            });
        }

        // For [library.prebuild.targets] and [library.prebuild.signatures],
        // allow '/' in addition to standard chars so path-traversal attempts
        // flow through to the per-section validator (which rejects them
        // via `HostTarget::from_triple`).
        let section_is_target_keyed = matches!(
            current_section.as_deref(),
            Some("library.prebuild.targets") | Some("library.prebuild.signatures")
        );
        let key_is_valid = if section_is_target_keyed {
            !key.is_empty()
                && key.chars().all(|c| {
                    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/'
                })
        } else {
            is_valid_key(key)
        };
        if !key_is_valid {
            return Err(AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: format!(
                    "invalid key '{}': only ASCII letters/digits/_/-/. allowed",
                    key
                ),
            });
        }

        let raw_value = parse_value(path, line_no, value)?;
        let target = match &current_section {
            None => &mut parsed.top_level,
            Some(name) if name == "functions" => parsed
                .functions
                .as_mut()
                .expect("functions section must be initialised"),
            Some(name) if name == "library.prebuild" => &mut parsed.prebuild,
            Some(name) if name == "library.prebuild.targets" => &mut parsed.prebuild_targets,
            Some(name) if name == "library.prebuild.signatures" => &mut parsed.prebuild_signatures,
            Some(other) => unreachable!("unexpected section state: {}", other),
        };
        if target.contains_key(key) {
            return Err(AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: format!("duplicate key '{}'", key),
            });
        }
        target.insert(key.to_string(), raw_value);
    }

    Ok(parsed)
}

fn strip_inline_comment(value: &str) -> &str {
    // RC1 v1 schema: strip `# comment` tails. Only quoted `"..."` strings
    // are expected (no embedded special characters). RC1.5+ URL-based
    // schema does not use this parser.
    if let Some(idx) = value.find('#') {
        // Be conservative: if `#` is inside `"..."` keep the entire
        // value. The string parser will surface a syntax error if the
        // string is malformed.
        let before = &value[..idx];
        let quotes = before.matches('"').count();
        if quotes.is_multiple_of(2) {
            return before;
        }
    }
    value
}

fn parse_value(path: &Path, line_no: usize, raw: &str) -> Result<RawValue, AddonManifestError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AddonManifestError::Syntax {
            path: path.to_path_buf(),
            line: line_no,
            message: "empty value after '='".to_string(),
        });
    }

    // String literal: `"..."`.
    if let Some(stripped) = trimmed.strip_prefix('"') {
        let inner = stripped
            .strip_suffix('"')
            .ok_or_else(|| AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: "unterminated string literal".to_string(),
            })?;
        if inner.contains('"') || inner.contains('\\') {
            return Err(AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: "string literals must be simple \"...\" (no escapes, no embedded quotes)"
                    .to_string(),
            });
        }
        return Ok(RawValue::Str(inner.to_string()));
    }

    // D28B-021: inline array of strings — `["a", "b"]`.
    // The parser is intentionally minimal: it only handles a single
    // line of `["a", "b", ...]` with simple `"..."` string literals.
    // Multi-line arrays are not part of the v1 schema and are rejected
    // as a syntax error so authors get a clear failure rather than a
    // partially-consumed continuation line.
    if let Some(rest) = trimmed.strip_prefix('[') {
        let inner = rest
            .strip_suffix(']')
            .ok_or_else(|| AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: "array literal must close with ']' on the same line".to_string(),
            })?;
        let items = parse_str_array_items(path, line_no, inner)?;
        return Ok(RawValue::StrArray(items));
    }

    // Integer literal.
    if let Ok(n) = trimmed.parse::<i64>() {
        return Ok(RawValue::Int(n));
    }

    Err(AddonManifestError::Syntax {
        path: path.to_path_buf(),
        line: line_no,
        message: format!(
            "expected string \"...\", integer, or array [\"...\", ...], got '{}'",
            trimmed
        ),
    })
}

/// D28B-021: parse the inside of a single-line `["a", "b"]` array.
/// Returns the extracted strings in order; rejects empty entries,
/// embedded escapes, and stray characters between elements.
fn parse_str_array_items(
    path: &Path,
    line_no: usize,
    inner: &str,
) -> Result<Vec<String>, AddonManifestError> {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    let mut rest = trimmed;
    loop {
        rest = rest.trim_start();
        if !rest.starts_with('"') {
            return Err(AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: format!(
                    "array element must start with '\"', got '{}'",
                    rest.chars().next().unwrap_or(' ')
                ),
            });
        }
        // Find the closing quote without supporting escapes.
        let after_open = &rest[1..];
        let close_idx = after_open
            .find('"')
            .ok_or_else(|| AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: "unterminated string literal in array".to_string(),
            })?;
        let item = &after_open[..close_idx];
        if item.contains('\\') {
            return Err(AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: "array string literals must be simple \"...\" (no escapes)".to_string(),
            });
        }
        items.push(item.to_string());
        rest = after_open[close_idx + 1..].trim_start();
        if rest.is_empty() {
            return Ok(items);
        }
        let after_comma = rest
            .strip_prefix(',')
            .ok_or_else(|| AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: format!("expected ',' or end of array, got '{}'", rest),
            })?;
        rest = after_comma.trim_start();
        // Permit trailing comma.
        if rest.is_empty() {
            return Ok(items);
        }
    }
}

fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Validates `sha256:<64-lowercase-hex>` format.
///
/// The prefix must be exactly `sha256:` (lowercase) followed by exactly
/// 64 lowercase hex characters (`0-9`, `a-f`). Uppercase hex is rejected
/// to ensure canonical form for cross-platform consistency.
fn is_valid_sha256(value: &str) -> bool {
    let prefix = "sha256:";
    if !value.starts_with(prefix) {
        return false;
    }
    let hex_part = &value[prefix.len()..];
    if hex_part.len() != 64 {
        return false;
    }
    hex_part.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// RC15B-005: validates the reserved `gpg:<opaque>` signature format.
///
/// The opaque identifier must be non-empty and contain only printable
/// ASCII (no whitespace, no control characters) so it can be embedded
/// in a URL template or a fingerprint without escaping. Actual signature
/// verification is deferred to a future RC; the format check here is
/// only to keep the reserved namespace clean.
fn is_valid_signature(value: &str) -> bool {
    let prefix = "gpg:";
    let Some(payload) = value.strip_prefix(prefix) else {
        return false;
    };
    if payload.is_empty() {
        return false;
    }
    // Printable ASCII, no whitespace.
    payload.chars().all(|c| c.is_ascii_graphic() && c != ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Result<AddonManifest, AddonManifestError> {
        parse_addon_manifest_str(Path::new("test://addon.toml"), source)
    }

    #[test]
    fn happy_path_parses_all_required_keys() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/addon-rs-sample"
library = "taida_addon_sample"

[functions]
noop = 0
echo = 1
"#;
        let manifest = parse(src).expect("happy path must parse");
        assert_eq!(manifest.abi, 1);
        assert_eq!(manifest.entry, "taida_addon_get_v1");
        assert_eq!(manifest.package, "taida-lang/addon-rs-sample");
        assert_eq!(manifest.library, "taida_addon_sample");
        assert_eq!(manifest.functions.len(), 2);
        assert_eq!(manifest.functions.get("noop"), Some(&0));
        assert_eq!(manifest.functions.get("echo"), Some(&1));
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let src = r#"
# Top-level required keys
abi = 1   # ABI v1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

# Functions table.
[functions]
# noop has no args
noop = 0
"#;
        let manifest = parse(src).expect("must parse with comments");
        assert_eq!(manifest.functions.get("noop"), Some(&0));
    }

    #[test]
    fn rejects_unsupported_abi() {
        let src = r#"
abi = 99
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"
[functions]
noop = 0
"#;
        let err = parse(src).expect_err("abi=99 must be rejected");
        assert!(matches!(
            err,
            AddonManifestError::AbiUnsupported {
                expected: 1,
                actual: 99,
                ..
            }
        ));
    }

    #[test]
    fn rejects_entry_symbol_drift() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v2"
package = "x/y"
library = "z"
[functions]
f = 1
"#;
        let err = parse(src).expect_err("entry mismatch must be rejected");
        match err {
            AddonManifestError::EntryMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, "taida_addon_get_v1");
                assert_eq!(actual, "taida_addon_get_v2");
            }
            other => panic!("expected EntryMismatch, got {other:?}"),
        }
    }

    #[test]
    fn missing_package_key_is_reported() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
library = "z"
[functions]
f = 0
"#;
        let err = parse(src).expect_err("missing package must error");
        match err {
            AddonManifestError::MissingKey { key, .. } => assert_eq!(key, "package"),
            other => panic!("expected MissingKey, got {other:?}"),
        }
    }

    #[test]
    fn empty_package_key_is_reported() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = ""
library = "z"
[functions]
f = 0
"#;
        let err = parse(src).expect_err("empty package must error");
        assert!(matches!(err, AddonManifestError::MissingPackageId { .. }));
    }

    #[test]
    fn missing_library_key_is_reported() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
[functions]
f = 0
"#;
        let err = parse(src).expect_err("missing library must error");
        match err {
            AddonManifestError::MissingKey { key, .. } => assert_eq!(key, "library"),
            other => panic!("expected MissingKey, got {other:?}"),
        }
    }

    #[test]
    fn missing_functions_section_is_reported() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"
"#;
        let err = parse(src).expect_err("missing [functions] must error");
        assert!(matches!(err, AddonManifestError::NoFunctions { .. }));
    }

    #[test]
    fn empty_functions_section_is_reported() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
"#;
        let err = parse(src).expect_err("empty [functions] must error");
        assert!(matches!(err, AddonManifestError::NoFunctions { .. }));
    }

    #[test]
    fn negative_arity_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"
[functions]
f = -1
"#;
        let err = parse(src).expect_err("negative arity must error");
        assert!(matches!(err, AddonManifestError::InvalidArity { .. }));
    }

    #[test]
    fn string_arity_is_rejected_as_invalid_arity() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"
[functions]
f = "one"
"#;
        let err = parse(src).expect_err("string arity must error");
        assert!(matches!(err, AddonManifestError::InvalidArity { .. }));
    }

    #[test]
    fn unknown_section_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"
[functions]
f = 0

[other]
nope = "yes"
"#;
        let err = parse(src).expect_err("unknown section must error");
        assert!(matches!(err, AddonManifestError::Syntax { .. }));
    }

    #[test]
    fn duplicate_top_level_key_is_rejected() {
        let src = r#"
abi = 1
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"
[functions]
f = 0
"#;
        let err = parse(src).expect_err("duplicate top-level key must error");
        match err {
            AddonManifestError::Syntax { message, .. } => {
                assert!(message.contains("duplicate"))
            }
            other => panic!("expected Syntax, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_function_key_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"
[functions]
f = 0
f = 1
"#;
        let err = parse(src).expect_err("duplicate function key must error");
        assert!(matches!(err, AddonManifestError::Syntax { .. }));
    }

    #[test]
    fn type_mismatch_for_abi_string() {
        let src = r#"
abi = "1"
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"
[functions]
f = 0
"#;
        let err = parse(src).expect_err("string abi must be rejected");
        match err {
            AddonManifestError::TypeMismatch { key, expected, .. } => {
                assert_eq!(key, "abi");
                assert_eq!(expected, "integer");
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_string_literal_is_syntax_error() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1
package = "x/y"
library = "z"
[functions]
f = 0
"#;
        let err = parse(src).expect_err("unterminated string must error");
        assert!(matches!(err, AddonManifestError::Syntax { .. }));
    }

    #[test]
    fn key_with_invalid_characters_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"
[functions]
bad name = 0
"#;
        let err = parse(src).expect_err("space in key must error");
        match err {
            AddonManifestError::Syntax { message, .. } => assert!(message.contains("invalid key")),
            other => panic!("expected Syntax, got {other:?}"),
        }
    }

    #[test]
    fn display_format_is_deterministic_for_abi_mismatch() {
        let err = AddonManifestError::AbiUnsupported {
            path: PathBuf::from("/tmp/addon.toml"),
            expected: 1,
            actual: 7,
        };
        let msg = err.to_string();
        assert!(msg.starts_with("addon manifest error:"));
        assert!(msg.contains("unsupported abi 7"));
        assert!(msg.contains("expected 1"));
        assert!(msg.contains("/tmp/addon.toml"));
    }

    #[test]
    fn display_format_is_deterministic_for_missing_key() {
        let err = AddonManifestError::MissingKey {
            path: PathBuf::from("/tmp/addon.toml"),
            key: "library",
        };
        let msg = err.to_string();
        assert!(msg.starts_with("addon manifest error:"));
        assert!(msg.contains("required key 'library'"));
    }

    // ── RC1.5: Prebuild manifest tests ────────────────────────

    #[test]
    fn prebuild_happy_path_with_url_and_targets() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/terminal"
library = "terminal"

[functions]
termPrint = 1

[library.prebuild]
url = "https://example.com/v{version}/{name}-{target}.{ext}"

[library.prebuild.targets]
x86_64-unknown-linux-gnu = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
aarch64-apple-darwin = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#;
        let manifest = parse(src).expect("prebuild happy path must parse");
        assert!(manifest.prebuild.has_prebuild());
        assert_eq!(
            manifest.prebuild.url_template.as_ref().unwrap(),
            "https://example.com/v{version}/{name}-{target}.{ext}"
        );
        assert_eq!(manifest.prebuild.targets.len(), 2);
        assert_eq!(
            manifest
                .prebuild
                .targets
                .get("x86_64-unknown-linux-gnu")
                .unwrap(),
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn prebuild_without_targets_is_valid() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"
"#;
        let manifest = parse(src).expect("prebuild without targets must parse");
        assert!(manifest.prebuild.has_prebuild());
        assert!(manifest.prebuild.targets.is_empty());
    }

    #[test]
    fn prebuild_missing_url_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]

[library.prebuild.targets]
x86_64-unknown-linux-gnu = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#;
        let err = parse(src).expect_err("prebuild without url must error");
        assert!(matches!(err, AddonManifestError::PrebuildMissingUrl { .. }));
    }

    #[test]
    fn prebuild_empty_url_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = ""
"#;
        let err = parse(src).expect_err("prebuild with empty url must error");
        assert!(matches!(err, AddonManifestError::PrebuildMissingUrl { .. }));
    }

    #[test]
    fn prebuild_url_type_mismatch_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = 123
"#;
        let err = parse(src).expect_err("prebuild url as int must error");
        assert!(matches!(err, AddonManifestError::TypeMismatch { key, .. } if key == "url"));
    }

    #[test]
    fn prebuild_invalid_sha256_prefix_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
x86_64-unknown-linux-gnu = "md5:aaaa"
"#;
        let err = parse(src).expect_err("invalid sha256 must error");
        assert!(
            matches!(err, AddonManifestError::PrebuildInvalidSha256 { target, .. } if target == "x86_64-unknown-linux-gnu")
        );
    }

    #[test]
    fn prebuild_sha256_wrong_length_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
x86_64-unknown-linux-gnu = "sha256:aaaa"
"#;
        let err = parse(src).expect_err("short sha256 must error");
        assert!(matches!(
            err,
            AddonManifestError::PrebuildInvalidSha256 { .. }
        ));
    }

    #[test]
    fn prebuild_sha256_uppercase_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
x86_64-unknown-linux-gnu = "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
"#;
        let err = parse(src).expect_err("uppercase sha256 must error");
        assert!(matches!(
            err,
            AddonManifestError::PrebuildInvalidSha256 { .. }
        ));
    }

    #[test]
    fn prebuild_duplicate_target_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
x86_64-unknown-linux-gnu = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
x86_64-unknown-linux-gnu = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#;
        let err = parse(src).expect_err("duplicate target must error");
        assert!(
            matches!(err, AddonManifestError::Syntax { message, .. } if message.contains("duplicate"))
        );
    }

    #[test]
    fn prebuild_url_unknown_variable_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{foo}"
"#;
        let err = parse(src).expect_err("unknown url variable must error");
        assert!(
            matches!(err, AddonManifestError::PrebuildUnknownUrlVariable { variable, .. } if variable == "foo")
        );
    }

    #[test]
    fn prebuild_url_unbalanced_brace_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{version"
"#;
        let err = parse(src).expect_err("unbalanced brace must error");
        assert!(matches!(
            err,
            AddonManifestError::PrebuildUnbalancedBrace { .. }
        ));
    }

    #[test]
    fn addon_without_prebuild_section() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0
"#;
        let manifest = parse(src).expect("addon without prebuild must parse");
        assert!(!manifest.prebuild.has_prebuild());
        assert!(manifest.prebuild.url_template.is_none());
        assert!(manifest.prebuild.targets.is_empty());
    }

    // ── RC15B-103: Unknown target triple is rejected ──

    #[test]
    fn prebuild_unknown_target_triple_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
"armv7-unknown-linux-gnueabihf" = "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
"#;
        let err = parse(src).expect_err("unknown target must error");
        assert!(
            matches!(err, AddonManifestError::PrebuildUnknownTarget { target, .. } if target == "armv7-unknown-linux-gnueabihf")
        );
    }

    #[test]
    fn prebuild_path_traversal_target_is_rejected() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
"x86_64/../../../etc/passwd" = "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
"#;
        let err = parse(src).expect_err("path traversal target must error");
        assert!(matches!(
            err,
            AddonManifestError::PrebuildUnknownTarget { .. }
        ));
    }

    #[test]
    fn prebuild_valid_target_triples_parse() {
        let base = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
"#;
        // All canonical targets (baseline + RC15B-003 extensions) should parse.
        for triple in crate::addon::host_target::supported_targets() {
            let src = format!(
                "{base}{} = \"sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789\"\n",
                triple
            );
            let manifest = parse(&src).unwrap_or_else(|e| panic!("triple={}, err={:?}", triple, e));
            assert!(manifest.prebuild.targets.contains_key(*triple));
            // Cleanup is not needed since we use a virtual path.
        }
    }

    // ── RC15B-005: signatures (reserved) ────────────────────────

    #[test]
    fn prebuild_signatures_happy_path() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu" = "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"

[library.prebuild.signatures]
"x86_64-unknown-linux-gnu" = "gpg:ABC123DEADBEEF"
"#;
        let manifest = parse(src).expect("signature section should parse");
        assert!(manifest.prebuild.has_prebuild());
        assert!(manifest.prebuild.has_signatures());
        assert_eq!(
            manifest.prebuild.signatures.get("x86_64-unknown-linux-gnu"),
            Some(&"gpg:ABC123DEADBEEF".to_string())
        );
    }

    #[test]
    fn prebuild_signatures_reject_non_gpg_prefix() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu" = "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"

[library.prebuild.signatures]
"x86_64-unknown-linux-gnu" = "sigstore:xxx"
"#;
        let err = parse(src).unwrap_err();
        assert!(
            matches!(
                err,
                AddonManifestError::PrebuildInvalidSignatureFormat { .. }
            ),
            "got: {:?}",
            err
        );
    }

    #[test]
    fn prebuild_signatures_reject_empty_gpg_payload() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu" = "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"

[library.prebuild.signatures]
"x86_64-unknown-linux-gnu" = "gpg:"
"#;
        let err = parse(src).unwrap_err();
        assert!(
            matches!(
                err,
                AddonManifestError::PrebuildInvalidSignatureFormat { .. }
            ),
            "got: {:?}",
            err
        );
    }

    #[test]
    fn prebuild_signatures_reject_unknown_target() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu" = "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"

[library.prebuild.signatures]
"x86_64-unknown-hurd" = "gpg:ABC"
"#;
        let err = parse(src).unwrap_err();
        assert!(
            matches!(
                err,
                AddonManifestError::PrebuildSignatureUnknownTarget { .. }
            ),
            "got: {:?}",
            err
        );
    }

    #[test]
    fn prebuild_signatures_absent_by_default() {
        let src = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "x/y"
library = "z"

[functions]
f = 0

[library.prebuild]
url = "https://example.com/{name}.{ext}"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu" = "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
"#;
        let manifest = parse(src).expect("manifest without signatures must parse");
        assert!(manifest.prebuild.has_prebuild());
        assert!(!manifest.prebuild.has_signatures());
        assert!(manifest.prebuild.signatures.is_empty());
    }

    #[test]
    fn is_valid_signature_accepts_gpg_prefix() {
        assert!(is_valid_signature("gpg:ABC123"));
        assert!(is_valid_signature("gpg:deadbeef"));
        assert!(is_valid_signature(
            "gpg:https://keys.openpgp.org/vks/v1/by-fingerprint/ABC"
        ));
    }

    #[test]
    fn is_valid_signature_rejects_bad_formats() {
        assert!(!is_valid_signature("sigstore:xxx"));
        assert!(!is_valid_signature(""));
        assert!(!is_valid_signature("gpg:"));
        assert!(!is_valid_signature("gpg:has space"));
        assert!(!is_valid_signature("gpg:has\ttab"));
    }
}

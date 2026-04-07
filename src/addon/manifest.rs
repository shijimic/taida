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

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use taida_addon::{TAIDA_ADDON_ABI_VERSION, TAIDA_ADDON_ENTRY_SYMBOL};

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

    Ok(AddonManifest {
        manifest_path: path.to_path_buf(),
        abi: abi as u32,
        entry,
        package,
        library,
        functions,
    })
}

// ── Minimal TOML subset parser ────────────────────────────────

/// Internal representation of a parsed `addon.toml`. Holds top-level
/// keys and the optional `[functions]` table. Anything else triggers
/// a syntax error so the schema stays pinned.
#[derive(Debug, Default)]
struct ParsedToml {
    top_level: BTreeMap<String, RawValue>,
    functions: Option<BTreeMap<String, RawValue>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawValue {
    Int(i64),
    Str(String),
}

impl RawValue {
    fn kind_label(&self) -> &'static str {
        match self {
            RawValue::Int(_) => "integer",
            RawValue::Str(_) => "string",
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
            if header != "functions" {
                return Err(AddonManifestError::Syntax {
                    path: path.to_path_buf(),
                    line: line_no,
                    message: format!(
                        "unknown section '[{}]' (only [functions] is allowed)",
                        header
                    ),
                });
            }
            if parsed.functions.is_some() {
                return Err(AddonManifestError::Syntax {
                    path: path.to_path_buf(),
                    line: line_no,
                    message: "[functions] section declared more than once".to_string(),
                });
            }
            parsed.functions = Some(BTreeMap::new());
            current_section = Some("functions".to_string());
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

        if key.is_empty() {
            return Err(AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: "empty key before '='".to_string(),
            });
        }
        if !is_valid_key(key) {
            return Err(AddonManifestError::Syntax {
                path: path.to_path_buf(),
                line: line_no,
                message: format!(
                    "invalid key '{}': only ASCII letters/digits/_/- allowed",
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
    // Strip a `# comment` tail. We do not interpret `#` inside strings
    // because the v1 schema only accepts simple `"..."` strings without
    // embedded special characters.
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

    // Integer literal.
    if let Ok(n) = trimmed.parse::<i64>() {
        return Ok(RawValue::Int(n));
    }

    Err(AddonManifestError::Syntax {
        path: path.to_path_buf(),
        line: line_no,
        message: format!("expected string \"...\" or integer, got '{}'", trimmed),
    })
}

fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
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
}

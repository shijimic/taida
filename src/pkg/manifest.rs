/// packages.tdm manifest parser and representation.
///
/// Supports two formats:
///
/// **New format** (packages.tdm as executable entry point):
/// ```taida
/// >>> taida-lang/os@1.0.0
/// >>> taida-community/http@2.1.0
/// >>> ./main.td => @(func)
/// <<<@1.0.0 @(capitalize, truncate)
/// ```
///
/// **Legacy format** (declarative assignments only):
/// ```taida
/// name <= "my-project"
/// version <= "0.1.0"
/// deps <= @(
///   utils <= @(path <= "../shared/utils")
/// )
/// ```
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::parser::{Expr, Statement};

/// A parsed package manifest.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Package name.
    pub name: String,
    /// Package version (semver string).
    pub version: String,
    /// Package description.
    pub description: String,
    /// Entry point file (default: "main.td").
    pub entry: String,
    /// Dependencies: name -> Dependency.
    pub deps: BTreeMap<String, Dependency>,
    /// Directory containing the manifest file.
    pub root_dir: PathBuf,
}

/// A single dependency specification.
#[derive(Debug, Clone, PartialEq)]
pub enum Dependency {
    /// Local path dependency.
    Path { path: String },
    /// Registry dependency: `>>> org/name@version`
    Registry {
        org: String,
        name: String,
        version: String,
    },
}

impl Manifest {
    /// Parse a `packages.tdm` file from the given directory.
    /// Returns None if the file doesn't exist.
    pub fn from_dir(dir: &Path) -> Result<Option<Self>, String> {
        let manifest_path = dir.join("packages.tdm");
        if !manifest_path.exists() {
            return Ok(None);
        }
        let source = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("Cannot read '{}': {}", manifest_path.display(), e))?;
        let manifest = Self::parse(&source, dir)?;
        Ok(Some(manifest))
    }

    /// Parse manifest from source string.
    ///
    /// Detects format automatically:
    /// - If the AST contains versioned imports (`>>> pkg@ver`) or versioned exports
    ///   (`<<<@ver`), use new format extraction.
    /// - Otherwise, fall back to legacy format (assignments only, reject side-effects).
    pub fn parse(source: &str, root_dir: &Path) -> Result<Self, String> {
        let (program, parse_errors) = crate::parser::parse(source);
        if !parse_errors.is_empty() {
            let msgs: Vec<String> = parse_errors.iter().map(|e| e.to_string()).collect();
            return Err(format!("packages.tdm parse errors:\n{}", msgs.join("\n")));
        }

        // Detect new format: any versioned import or versioned export
        let has_new_format = program.statements.iter().any(|stmt| {
            matches!(
                stmt,
                Statement::Import(imp) if imp.version.is_some()
            ) || matches!(
                stmt,
                Statement::Export(exp) if exp.version.is_some()
            )
        });

        if has_new_format {
            Self::extract_from_ast(&program, root_dir)
        } else {
            Self::parse_legacy(&program, root_dir)
        }
    }

    /// Extract manifest from new-format AST (versioned imports/exports).
    ///
    /// Enforces packages.tdm constraints:
    /// - P-2: Only `>>>` and `<<<` statements allowed (no expressions, assignments, function defs)
    /// - P-4: Only one `<<<` line allowed
    fn extract_from_ast(program: &crate::parser::Program, root_dir: &Path) -> Result<Self, String> {
        let mut deps = BTreeMap::new();
        let mut version = "0.1.0".to_string();
        let mut entry = "main.td".to_string();
        let mut export_count = 0;
        // B11-1b: package identity from `<<<@version owner/name`
        let mut package_id: Option<String> = None;

        for stmt in &program.statements {
            match stmt {
                Statement::Import(imp) if imp.version.is_some() => {
                    // >>> taida-lang/string-utils@1.0.0
                    if let Some((org, name)) = parse_org_name(&imp.path) {
                        let ver = match &imp.version {
                            Some(v) => v.clone(),
                            None => {
                                return Err(format!(
                                    "packages.tdm: import '{}' has no version. This is a parser bug.",
                                    imp.path
                                ));
                            }
                        };
                        let canonical_id = format!("{}/{}", org, name);
                        deps.insert(
                            canonical_id,
                            Dependency::Registry {
                                org,
                                name,
                                version: ver,
                            },
                        );
                    }
                }
                Statement::Import(imp)
                    if imp.version.is_none()
                        && (imp.path.starts_with("./") || imp.path.starts_with("../")) =>
                {
                    // >>> ./main.td => @(hello) — local import determines entry point
                    entry = imp.path.clone();
                }
                Statement::Export(exp) => {
                    export_count += 1;
                    // P-4: only one <<< line allowed in packages.tdm
                    if export_count > 1 {
                        return Err(
                            "packages.tdm: only one <<< (export) line is allowed.".to_string()
                        );
                    }
                    if let Some(v) = &exp.version {
                        version = v.clone();
                    }
                    // B11-1b: extract package identity from `<<<@version owner/name`
                    if let Some(pkg_id) = &exp.path {
                        package_id = Some(pkg_id.clone());
                    }
                }
                // P-2: reject non-import/export statements
                Statement::Import(imp) => {
                    // Non-local, non-versioned import (e.g. bare package name without version)
                    return Err(format!(
                        "packages.tdm: import '{}' must have a version (@version) or be a local path (./...).",
                        imp.path
                    ));
                }
                Statement::FuncDef(fd) => {
                    return Err(format!(
                        "packages.tdm: function definitions ('{}') are not allowed. \
                         Only >>> and <<< are permitted.",
                        fd.name
                    ));
                }
                Statement::Assignment(a) => {
                    return Err(format!(
                        "packages.tdm: assignments ('{}') are not allowed. \
                         Only >>> and <<< are permitted.",
                        a.target
                    ));
                }
                other => {
                    return Err(format!(
                        "packages.tdm: only >>> and <<< statements are allowed. Found: {:?}",
                        other
                    ));
                }
            }
        }

        // B11-1b: name from package identity (<<<@version owner/name),
        // falling back to directory name for backward compatibility.
        let name = package_id.unwrap_or_else(|| {
            root_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        Ok(Manifest {
            name,
            version,
            description: String::new(),
            entry,
            deps,
            root_dir: root_dir.to_path_buf(),
        })
    }

    /// Parse legacy format manifest (assignments only).
    fn parse_legacy(program: &crate::parser::Program, root_dir: &Path) -> Result<Self, String> {
        // Validate: only Assignment statements are allowed (reject side-effect statements)
        for stmt in &program.statements {
            match stmt {
                Statement::Assignment(_) => { /* allowed */ }
                Statement::Import(_) => {
                    return Err(
                        "packages.tdm: import statements (>>>) are not allowed in manifest files. \
                         Only assignments (name <= \"value\") are permitted."
                            .to_string(),
                    );
                }
                Statement::Export(_) => {
                    return Err(
                        "packages.tdm: export statements (<<<) are not allowed in manifest files."
                            .to_string(),
                    );
                }
                Statement::FuncDef(fd) => {
                    return Err(format!(
                        "packages.tdm: function definitions ('{}') are not allowed in manifest files.",
                        fd.name
                    ));
                }
                Statement::Expr(expr) => {
                    return Err(format!(
                        "packages.tdm: expression statements are not allowed in manifest files. \
                         Found: {:?}",
                        expr
                    ));
                }
                other => {
                    return Err(format!(
                        "packages.tdm: unsupported statement type in manifest file: {:?}",
                        other
                    ));
                }
            }
        }

        // Extract fields from AST assignments
        let mut fields: BTreeMap<String, AstValue> = BTreeMap::new();
        for stmt in &program.statements {
            if let Statement::Assignment(assign) = stmt {
                let value = Self::extract_ast_value(&assign.value)?;
                fields.insert(assign.target.clone(), value);
            }
        }

        let name = match fields.get("name") {
            Some(AstValue::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let version = match fields.get("version") {
            Some(AstValue::Str(s)) => s.clone(),
            _ => "0.1.0".to_string(),
        };
        let description = match fields.get("description") {
            Some(AstValue::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let entry = match fields.get("entry") {
            Some(AstValue::Str(s)) => s.clone(),
            _ => "main.td".to_string(),
        };

        // Extract dependencies from the `deps` field
        let mut deps = BTreeMap::new();
        if let Some(AstValue::BuchiPack(dep_entries)) = fields.get("deps") {
            for (dep_name, dep_val) in dep_entries {
                if let AstValue::BuchiPack(dep_fields) = dep_val
                    && let Some(AstValue::Str(path_str)) = dep_fields.get("path")
                {
                    deps.insert(
                        dep_name.clone(),
                        Dependency::Path {
                            path: path_str.clone(),
                        },
                    );
                }
            }
        }

        Ok(Manifest {
            name,
            version,
            description,
            entry,
            deps,
            root_dir: root_dir.to_path_buf(),
        })
    }

    /// Extract a value from an AST expression (literals and buchi packs only).
    fn extract_ast_value(expr: &Expr) -> Result<AstValue, String> {
        match expr {
            Expr::StringLit(s, _) => Ok(AstValue::Str(s.clone())),
            Expr::IntLit(n, _) => Ok(AstValue::Int(*n)),
            Expr::FloatLit(n, _) => Ok(AstValue::Float(*n)),
            Expr::BoolLit(b, _) => Ok(AstValue::Bool(*b)),
            Expr::BuchiPack(fields, _) => {
                let mut map = BTreeMap::new();
                for field in fields {
                    let val = Self::extract_ast_value(&field.value)?;
                    map.insert(field.name.clone(), val);
                }
                Ok(AstValue::BuchiPack(map))
            }
            Expr::ListLit(items, _) => {
                let mut list = Vec::new();
                for item in items {
                    list.push(Self::extract_ast_value(item)?);
                }
                Ok(AstValue::List(list))
            }
            _ => Err(format!(
                "packages.tdm: unsupported expression in manifest. \
                 Only literals and @(...) are allowed. Found: {:?}",
                expr
            )),
        }
    }

    /// Generate the default `packages.tdm` content for `taida init`.
    pub fn default_template(name: &str) -> String {
        format!(
            r#"// packages.tdm -- {name}
// Dependencies:
// >>> taida-community/example@a.1
"#
        )
    }

    /// Generate the default `main.td` content for `taida init`.
    pub fn default_main() -> &'static str {
        r#"// main.td -- Entry point

stdout("Hello from Taida!")
"#
    }
}

/// Check if a version string is a valid Taida version (`gen.num.label`, `gen.num`, or `gen`).
///
/// Valid: "a.3", "b.12", "aa.1", "a", "b", "aa", "a.1.alpha", "x.34.gen-2-stable"
/// Invalid: "1.0.0", "ABC.3", "", "a.1.", "a.1.Alpha", "a.1.-bad"
pub fn is_valid_taida_version(v: &str) -> bool {
    if v.is_empty() {
        return false;
    }
    let parts: Vec<&str> = v.splitn(3, '.').collect();
    match parts.len() {
        1 => {
            // gen-only: all lowercase letters
            parts[0].chars().all(|c| c.is_ascii_lowercase()) && !parts[0].is_empty()
        }
        2 => {
            // gen.num
            let generation = parts[0];
            let num = parts[1];
            generation.chars().all(|c| c.is_ascii_lowercase())
                && !generation.is_empty()
                && num.parse::<u64>().is_ok()
        }
        3 => {
            // gen.num.label
            let generation = parts[0];
            let num = parts[1];
            let label = parts[2];
            generation.chars().all(|c| c.is_ascii_lowercase())
                && !generation.is_empty()
                && num.parse::<u64>().is_ok()
                && is_valid_version_label(label)
        }
        _ => false,
    }
}

/// Check if a string is a valid version label: [a-z0-9][a-z0-9-]* (no trailing hyphen).
fn is_valid_version_label(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let first = s.chars().next().unwrap();
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    if s.ends_with('-') {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Parse "org/name" from an import path.
/// "taida-lang/string-utils" -> Some(("taida-lang", "string-utils"))
/// "string-utils"            -> Some(("", "string-utils"))
/// "./main.td"               -> None (local path)
fn parse_org_name(path: &str) -> Option<(String, String)> {
    if path.starts_with('.') || path.starts_with('/') {
        return None;
    }
    if let Some(slash_pos) = path.find('/') {
        let org = path[..slash_pos].to_string();
        let name = path[slash_pos + 1..].to_string();
        Some((org, name))
    } else {
        Some((String::new(), path.to_string()))
    }
}

/// Internal AST value representation for manifest extraction.
/// Some variants (Int, Float, Bool, List) are not yet consumed by manifest
/// field extraction but are included for completeness and future extensibility.
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum AstValue {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    BuchiPack(BTreeMap<String, AstValue>),
    List(Vec<AstValue>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let source = r#"
name <= "test-pkg"
version <= "1.0.0"
"#;
        let manifest = Manifest::parse(source, Path::new("/tmp")).unwrap();
        assert_eq!(manifest.name, "test-pkg");
        assert_eq!(manifest.version, "1.0.0");
        assert!(manifest.deps.is_empty());
    }

    #[test]
    fn test_parse_manifest_with_deps() {
        let source = r#"
name <= "my-app"
version <= "0.2.0"
description <= "My application"

deps <= @(
  utils <= @(path <= "../shared/utils"),
  math <= @(path <= "./libs/math")
)
"#;
        let manifest = Manifest::parse(source, Path::new("/project")).unwrap();
        assert_eq!(manifest.name, "my-app");
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.description, "My application");
        assert_eq!(manifest.deps.len(), 2);

        assert_eq!(
            manifest.deps.get("utils").unwrap(),
            &Dependency::Path {
                path: "../shared/utils".to_string()
            }
        );
        assert_eq!(
            manifest.deps.get("math").unwrap(),
            &Dependency::Path {
                path: "./libs/math".to_string()
            }
        );
    }

    #[test]
    fn test_parse_manifest_defaults() {
        let source = r#"
name <= "minimal"
"#;
        let manifest = Manifest::parse(source, Path::new("/tmp")).unwrap();
        assert_eq!(manifest.name, "minimal");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.entry, "main.td");
    }

    #[test]
    fn test_default_template() {
        let template = Manifest::default_template("hello-world");
        assert!(template.contains("hello-world"));
        assert!(template.contains("taida-community/example@a.1"));
    }

    // ── Security tests: reject non-declarative statements ──

    #[test]
    fn test_reject_import_in_manifest() {
        let source = r#"
name <= "evil-pkg"
>>> std/io => @(writeFile)
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(result.is_err(), "Import should be rejected in manifest");
        assert!(result.unwrap_err().contains("import statements"));
    }

    #[test]
    fn test_reject_function_def_in_manifest() {
        let source = r#"
name <= "evil-pkg"
evil =
  42
=> :Int
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(
            result.is_err(),
            "Function def should be rejected in manifest"
        );
        assert!(result.unwrap_err().contains("function definitions"));
    }

    #[test]
    fn test_reject_export_in_manifest() {
        let source = r#"
name <= "evil-pkg"
<<< @(name)
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(result.is_err(), "Export should be rejected in manifest");
        assert!(result.unwrap_err().contains("export statements"));
    }

    #[test]
    fn test_reject_function_call_in_manifest() {
        let source = r#"
name <= "evil-pkg"
writeFile("/tmp/evil.txt", "gotcha")
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(
            result.is_err(),
            "Function call expression should be rejected in manifest"
        );
        assert!(result.unwrap_err().contains("expression statements"));
    }

    // ── New format tests ──

    #[test]
    fn test_extract_deps_from_new_format() {
        let source = r#"
>>> taida-lang/os@1.0.0
>>> taida-community/http@2.1.0
>>> ./main.td => @(func)
<<<@1.0.0 @(func)
"#;
        let manifest = Manifest::parse(source, Path::new("/tmp")).unwrap();
        assert_eq!(manifest.deps.len(), 2); // 2 registry deps (not ./main.td)
        assert_eq!(manifest.version, "1.0.0"); // from <<<@1.0.0

        assert_eq!(
            manifest.deps.get("taida-lang/os").unwrap(),
            &Dependency::Registry {
                org: "taida-lang".to_string(),
                name: "os".to_string(),
                version: "1.0.0".to_string(),
            }
        );
        assert_eq!(
            manifest.deps.get("taida-community/http").unwrap(),
            &Dependency::Registry {
                org: "taida-community".to_string(),
                name: "http".to_string(),
                version: "2.1.0".to_string(),
            }
        );
    }

    #[test]
    fn test_new_format_version_from_export() {
        let source = r#"
<<<@2.5.3 @(myFunc)
"#;
        let manifest = Manifest::parse(source, Path::new("/my-pkg")).unwrap();
        assert_eq!(manifest.version, "2.5.3");
        assert_eq!(manifest.name, "my-pkg"); // derived from directory
    }

    #[test]
    fn test_new_format_no_version_uses_default() {
        let source = r#"
>>> taida-lang/os@1.0.0
"#;
        let manifest = Manifest::parse(source, Path::new("/tmp")).unwrap();
        assert_eq!(manifest.version, "0.1.0"); // no <<<@version found
    }

    #[test]
    fn test_legacy_format_still_works() {
        let source = r#"
name <= "legacy-app"
version <= "3.0.0"
description <= "A legacy app"
"#;
        let manifest = Manifest::parse(source, Path::new("/tmp")).unwrap();
        assert_eq!(manifest.name, "legacy-app");
        assert_eq!(manifest.version, "3.0.0");
        assert_eq!(manifest.description, "A legacy app");
    }

    #[test]
    fn test_is_valid_taida_version() {
        assert!(is_valid_taida_version("a.3"));
        assert!(is_valid_taida_version("b.12"));
        assert!(is_valid_taida_version("aa.1"));
        assert!(is_valid_taida_version("a"));
        assert!(is_valid_taida_version("bb"));
        // Label support
        assert!(is_valid_taida_version("a.1.alpha"));
        assert!(is_valid_taida_version("a.5.beta"));
        assert!(is_valid_taida_version("x.34.gen-2-stable"));
        assert!(is_valid_taida_version("a.12.rc"));
        assert!(is_valid_taida_version("a.1.0rc1")); // starts with digit
        // Invalid
        assert!(!is_valid_taida_version("1.0.0")); // SemVer, not gen.num
        assert!(!is_valid_taida_version("ABC.3")); // uppercase
        assert!(!is_valid_taida_version(""));
        assert!(!is_valid_taida_version("a.")); // missing num
        assert!(!is_valid_taida_version("a.abc")); // non-numeric num
        assert!(!is_valid_taida_version("a.1.Alpha")); // uppercase label
        assert!(!is_valid_taida_version("a.1.-bad")); // label starts with hyphen
        assert!(!is_valid_taida_version("a.1.bad-")); // label ends with hyphen
    }

    #[test]
    fn test_extract_deps_gen_num_format() {
        let source = r#"
>>> alice/webframework@b.12
>>> bob/jsonutil@a
<<<@a.3 @(MyApp)
"#;
        let manifest = Manifest::parse(source, Path::new("/my-app")).unwrap();
        assert_eq!(manifest.deps.len(), 2);
        assert_eq!(manifest.version, "a.3");

        assert_eq!(
            manifest.deps.get("alice/webframework").unwrap(),
            &Dependency::Registry {
                org: "alice".to_string(),
                name: "webframework".to_string(),
                version: "b.12".to_string(),
            }
        );
        assert_eq!(
            manifest.deps.get("bob/jsonutil").unwrap(),
            &Dependency::Registry {
                org: "bob".to_string(),
                name: "jsonutil".to_string(),
                version: "a".to_string(),
            }
        );
    }

    #[test]
    fn test_entry_from_local_import() {
        let source = r#"
>>> taida-lang/os@a.1
>>> ./lib.td => @(hello, greet)
<<<@a.3 @(hello, greet)
"#;
        let manifest = Manifest::parse(source, Path::new("/my-pkg")).unwrap();
        assert_eq!(manifest.entry, "./lib.td");
        assert_eq!(manifest.version, "a.3");
        assert_eq!(manifest.deps.len(), 1);
    }

    #[test]
    fn test_entry_defaults_to_main_td_without_local_import() {
        let source = r#"
>>> taida-lang/os@a.1
<<<@a.1 @(hello)
"#;
        let manifest = Manifest::parse(source, Path::new("/my-pkg")).unwrap();
        assert_eq!(manifest.entry, "main.td");
    }

    #[test]
    fn test_reject_unversioned_import_in_legacy_format() {
        // Legacy format rejects >>> without version (since it's not new format)
        let source = r#"
name <= "evil-pkg"
>>> ./main.td => @(func)
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(
            result.is_err(),
            "Unversioned import should be rejected in legacy format"
        );
    }

    // ── P-2: Side-effect prohibition in new format ──

    #[test]
    fn test_p2_reject_assignment_in_new_format() {
        let source = r#"
>>> taida-lang/os@a.1
hw <= "hello"
<<<@a.1 @(hw)
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("assignments"));
    }

    #[test]
    fn test_p2_reject_funcdef_in_new_format() {
        let source = r#"
>>> taida-lang/os@a.1
evil x = x => :Int
<<<@a.1 @(evil)
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("function definitions"));
    }

    #[test]
    fn test_p2_reject_expression_in_new_format() {
        let source = r#"
>>> taida-lang/os@a.1
stdout("hello")
<<<@a.1 @(hello)
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(result.is_err(), "Expression should be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("only >>> and <<<"),
            "Unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_p2_reject_bare_package_import_without_version() {
        let source = r#"
>>> taida-lang/os@a.1
>>> alice/utils
<<<@a.1 @(hello)
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must have a version"));
    }

    // ── P-4: Single <<< line constraint ──

    #[test]
    fn test_p4_reject_multiple_exports() {
        let source = r#"
>>> taida-lang/os@a.1
>>> ./main.td => @(hello, greet)
<<<@a.1 @(hello)
<<< @(greet)
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("only one <<<"));
    }

    #[test]
    fn test_p4_single_export_ok() {
        let source = r#"
>>> taida-lang/os@a.1
>>> ./main.td => @(hello, greet)
<<<@a.1 @(hello, greet)
"#;
        let result = Manifest::parse(source, Path::new("/tmp"));
        assert!(result.is_ok());
    }

    // ── FL-15 regression: version unwrap safety ──

    #[test]
    fn test_export_without_version_does_not_panic() {
        // An export (<<<) without a version should use the default, not panic
        let source = r#"
>>> taida-lang/os@a.1
<<< @(hello)
"#;
        let result = Manifest::parse(source, Path::new("/my-pkg"));
        assert!(result.is_ok());
        let manifest = result.unwrap();
        assert_eq!(manifest.version, "0.1.0"); // default version
    }

    // ── B11-1e: Package identity from <<< line ──

    #[test]
    fn test_package_identity_from_export() {
        // B11B-012: `owner/name + @(symbols)` is now forbidden.
        // Canonical form is `<<<@version owner/name` only.
        let source = r#"
>>> taida-lang/os@a.1
<<<@b.11.rc3 taida-lang/terminal
"#;
        let manifest = Manifest::parse(source, Path::new("/my-pkg")).unwrap();
        assert_eq!(manifest.name, "taida-lang/terminal");
        assert_eq!(manifest.version, "b.11.rc3");
    }

    #[test]
    fn test_package_identity_without_symbols() {
        let source = r#"
<<<@a.3 shijimic/my-pkg
"#;
        let manifest = Manifest::parse(source, Path::new("/tmp")).unwrap();
        assert_eq!(manifest.name, "shijimic/my-pkg");
        assert_eq!(manifest.version, "a.3");
    }

    #[test]
    fn test_package_identity_absent_falls_back_to_dir_name() {
        // B11-1f backward compat: <<<@version without package identity
        // should still derive name from directory.
        let source = r#"
<<<@a.3 @(MyApp)
"#;
        let manifest = Manifest::parse(source, Path::new("/my-pkg")).unwrap();
        assert_eq!(manifest.name, "my-pkg");
        assert_eq!(manifest.version, "a.3");
    }

    #[test]
    fn test_package_identity_version_only_no_symbols() {
        // <<<@a.3 with no symbols and no package identity
        let source = "<<<@a.3\n";
        let manifest = Manifest::parse(source, Path::new("/my-project")).unwrap();
        assert_eq!(manifest.name, "my-project");
        assert_eq!(manifest.version, "a.3");
    }
}

//! `taida upgrade --e30 <path>` — E30 migration tool (Phase 2 Sub-step 2.3 skeleton).
//!
//! E30 で確定した型構文 surface 統一 (`Name[?type-args] [=> Parent] = @(...)`) への
//! 旧構文 migrate を AST-aware に行う。
//!
//! ## Lock-E verdict (2026-04-28) 整合
//!
//! - 統合先: E31 `taida way migrate --<ver>` ハブ (E31B-004 subcommand 統合候補)
//! - E30 段階の skeleton は `taida upgrade --e30 <PATH>` として稼働
//!   (D28 前例 `taida upgrade --d28` 継承)
//! - E31 ハブ統合時に `taida way migrate --e30` を `taida upgrade --e30` の
//!   alias / wrapper として追加予定
//! - deprecation policy: E gen は **deprecation なし、即破壊的変更**
//!   → 本 tool は旧構文を新構文に直接書き換える (warning フェーズなし)
//! - stable gate 必須条件: migration tool が動作することは `@e.30` stable
//!   宣言の必須条件 (Phase 7 で完成)
//!
//! ## Scope (Phase 2 Sub-step 2.3 skeleton)
//!
//! 本 skeleton は以下のみ実装:
//! - CLI argument 解析 (`--check` / `--dry-run` / `<PATH>`)
//! - 旧 `Mold[T] => Foo[T] = @(...)` 構文の検出
//!   (`ClassLikeDef::is_legacy_e30_syntax()` 経由)
//! - dry-run mode で旧構文 → 新構文の **header** 出力
//!   (1〜2 パターン smoke level、fields 部の完全 textual 再構成は Phase 7)
//! - `--check` mode (旧構文を検出したら 1 件以上で error 戻り値)
//! - **ファイル書き換え (in-place rewrite) は Phase 7 で実装**
//!
//! ## Phase 7 で完成させる要素
//!
//! - 完全な AST 書き換えロジック (全旧構文 patterns / fields 部完全 reconstruction)
//! - 23 sentinel 関数の `RustAddon[...]` migration (E30B-007 連携)
//! - in-place ファイル書き換え (D28 同様 char-offset based rewrite)
//! - idempotent test (二度実行しても変化なし)
//! - 4-backend で migration 後の .td が同一動作する parity test

use crate::parser::{ClassLikeDef, ClassLikeKind, MoldHeaderArg, Statement, TypeExpr, parse};

/// Configuration for the `taida upgrade --e30` migration run.
#[derive(Debug, Clone)]
pub struct UpgradeE30Config {
    /// Target path. Either a single `.td` file or a directory tree.
    pub path: std::path::PathBuf,
    /// `--check`: read-only mode, exits with error if any legacy syntax is found.
    pub check_only: bool,
    /// `--dry-run`: scan and print proposed migrations without modifying files.
    pub dry_run: bool,
}

/// One proposed migration of a single legacy `ClassLikeDef`.
#[derive(Debug, Clone)]
pub struct MigrationProposal {
    pub file: std::path::PathBuf,
    /// 1-based source line of the legacy class-like definition.
    pub line: usize,
    /// `"mold"` (Phase 2 Sub-step 2.3) — extensible in Phase 7.
    pub legacy_kind: &'static str,
    /// Header snippet of the legacy form, e.g. `Mold[T] => Box[T]`.
    pub legacy_header: String,
    /// Proposed new header, e.g. `Box[T]`.
    pub proposed_header: String,
}

/// Result of running the migration tool.
#[derive(Debug, Default)]
pub struct UpgradeE30Report {
    pub files_scanned: usize,
    /// Total legacy ClassLikeDef nodes detected across all files.
    pub legacy_count: usize,
    pub proposals: Vec<MigrationProposal>,
}

/// Errors surfaced from the migration tool entry point.
#[derive(Debug)]
pub enum UpgradeE30Error {
    Io(std::io::Error),
    /// `--check` mode: returned when any legacy syntax was detected.
    CheckFailed {
        legacy_count: usize,
    },
}

impl std::fmt::Display for UpgradeE30Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpgradeE30Error::Io(e) => write!(f, "I/O error: {}", e),
            UpgradeE30Error::CheckFailed { legacy_count } => write!(
                f,
                "{} legacy E30 class-like definition(s) need migration",
                legacy_count
            ),
        }
    }
}

impl std::error::Error for UpgradeE30Error {}

impl From<std::io::Error> for UpgradeE30Error {
    fn from(e: std::io::Error) -> Self {
        UpgradeE30Error::Io(e)
    }
}

/// Format a `MoldHeaderArg` list as the textual `[...]` arg list for the
/// migration header preview. Skeleton level: produces `[T]`, `[T, U]`,
/// `[:Int]`, `[T <= :Int]` etc. matches the parser surface.
fn format_mold_header_args(args: &[MoldHeaderArg]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(args.len());
    for a in args {
        match a {
            MoldHeaderArg::TypeParam(tp) => match &tp.constraint {
                None => parts.push(tp.name.clone()),
                Some(constraint) => {
                    parts.push(format!("{} <= {}", tp.name, format_type_expr(constraint)));
                }
            },
            MoldHeaderArg::Concrete(ty) => parts.push(format_type_expr(ty)),
        }
    }
    format!("[{}]", parts.join(", "))
}

/// Skeleton-level type expression formatter (only covers the surface forms
/// reachable from `MoldHeaderArg`). Phase 7 will replace this with a full
/// AST → source pretty-printer.
fn format_type_expr(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(name) => format!(":{}", name),
        TypeExpr::Generic(name, params) => {
            let inner: Vec<String> = params.iter().map(format_type_expr).collect();
            format!(":{}[{}]", name, inner.join(", "))
        }
        TypeExpr::List(inner) => format!(":@[{}]", format_type_expr(inner)),
        TypeExpr::BuchiPack(_) => ":@(...)".to_string(), // Phase 7 で完全化
        TypeExpr::Function(_, _) => ":(... => :...)".to_string(), // Phase 7
    }
}

/// Compute the legacy header snippet for a `ClassLikeKind::Mold` definition.
/// E.g. `Mold[T] => Box[T]`.
fn legacy_mold_header(def: &ClassLikeDef, mold_args: &[MoldHeaderArg]) -> String {
    let mold_part = format_mold_header_args(mold_args);
    let name_part = match &def.name_args {
        Some(args) => format!("{}{}", def.name, format_mold_header_args(args)),
        None => def.name.clone(),
    };
    format!("Mold{} => {}", mold_part, name_part)
}

/// Compute the proposed new header (Lock-B Sub-B1 + Sub-B2 verdict):
/// drop the `Mold[...] =>` prefix, keep the child's `Name[...]` arg list
/// (zero-or-more arity, accepted by parser since Sub-step 2.2).
fn proposed_new_header(def: &ClassLikeDef) -> String {
    match &def.name_args {
        Some(args) => format!("{}{}", def.name, format_mold_header_args(args)),
        None => def.name.clone(),
    }
}

/// Walk a parsed `Program` and collect a `MigrationProposal` for every
/// legacy class-like definition encountered. Skeleton level: only the
/// `Mold[T] => Foo[T] = @(...)` legacy form is detected
/// (`ClassLikeDef::is_legacy_e30_syntax()`).
fn collect_proposals_from_program(
    program: &crate::parser::Program,
    file: &std::path::Path,
) -> Vec<MigrationProposal> {
    let mut out = Vec::new();
    for stmt in &program.statements {
        if let Statement::ClassLikeDef(def) = stmt
            && def.is_legacy_e30_syntax()
            && let ClassLikeKind::Mold { mold_args } = &def.kind
        {
            out.push(MigrationProposal {
                file: file.to_path_buf(),
                line: def.span.line,
                legacy_kind: def.legacy_e30_kind().unwrap_or("mold"),
                legacy_header: legacy_mold_header(def, mold_args),
                proposed_header: proposed_new_header(def),
            });
        }
    }
    out
}

/// Public entry: scan a single Taida source string for legacy E30 syntax
/// and return proposed migrations. No file I/O, suitable for unit tests.
///
/// Phase 7 will extend this to return a full rewritten source string;
/// the Sub-step 2.3 skeleton only emits proposal metadata.
pub fn scan_source(source: &str, file: &std::path::Path) -> Vec<MigrationProposal> {
    let (program, errors) = parse(source);
    if !errors.is_empty() {
        // Parse errors → conservative: no proposals (caller decides).
        return Vec::new();
    }
    collect_proposals_from_program(&program, file)
}

/// Recursively walk a directory and collect all `.td` files.
/// Skips dotted directories (`.git`, `.dev`) and build artifacts.
fn collect_td_files(
    path: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    if path.is_file() {
        if path.extension().and_then(|s| s.to_str()) == Some("td") {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with('.') || s == "target" || s == "node_modules")
                .unwrap_or(false)
            {
                continue;
            }
            collect_td_files(&p, out)?;
        }
    }
    Ok(())
}

/// Public entry from the CLI: run the migration scan according to
/// `config`. Returns an `UpgradeE30Report` summarising the scan.
///
/// `--check` mode propagates `UpgradeE30Error::CheckFailed` if any legacy
/// syntax was detected. `--dry-run` prints proposals to stdout. Default
/// mode (neither flag) currently behaves like `--dry-run` (skeleton);
/// Phase 7 will switch the default to in-place rewrite.
pub fn run(config: UpgradeE30Config) -> Result<UpgradeE30Report, UpgradeE30Error> {
    let mut files = Vec::new();
    collect_td_files(&config.path, &mut files)?;

    let mut report = UpgradeE30Report {
        files_scanned: 0,
        legacy_count: 0,
        proposals: Vec::new(),
    };

    if files.is_empty() {
        eprintln!("No .td files found under {}", config.path.display());
        return Ok(report);
    }

    for f in &files {
        report.files_scanned += 1;
        let source = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading {}: {}", f.display(), e);
                continue;
            }
        };
        let proposals = scan_source(&source, f);
        report.legacy_count += proposals.len();
        for p in &proposals {
            if config.check_only {
                println!(
                    "[check] {}:{} legacy {} syntax: `{}` -> `{}`",
                    p.file.display(),
                    p.line,
                    p.legacy_kind,
                    p.legacy_header,
                    p.proposed_header
                );
            } else if config.dry_run {
                println!(
                    "[dry-run] {}:{} {} -> {}",
                    p.file.display(),
                    p.line,
                    p.legacy_header,
                    p.proposed_header
                );
            } else {
                // Skeleton: default mode behaves like dry-run for now.
                // Phase 7 will perform in-place rewrite here.
                println!(
                    "[skeleton] {}:{} {} -> {} (Phase 7 で in-place rewrite 予定)",
                    p.file.display(),
                    p.line,
                    p.legacy_header,
                    p.proposed_header
                );
            }
        }
        report.proposals.extend(proposals);
    }

    if config.check_only && report.legacy_count > 0 {
        return Err(UpgradeE30Error::CheckFailed {
            legacy_count: report.legacy_count,
        });
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_detects_legacy_mold_syntax() {
        let src = "Mold[T] => Box[T] = @(filling: T)\n";
        let proposals = scan_source(src, std::path::Path::new("test.td"));
        assert_eq!(proposals.len(), 1, "expected 1 legacy mold detection");
        let p = &proposals[0];
        assert_eq!(p.legacy_kind, "mold");
        assert_eq!(p.legacy_header, "Mold[T] => Box[T]");
        assert_eq!(p.proposed_header, "Box[T]");
        assert_eq!(p.line, 1);
    }

    #[test]
    fn scan_ignores_new_e30_class_like_forms() {
        // Sub-step 2.2 で受理される新構文は migration 対象外
        let src = "Pilot[] = @(name: Str)\nBox[T] = @(filling: T)\n";
        let proposals = scan_source(src, std::path::Path::new("test.td"));
        assert!(
            proposals.is_empty(),
            "new-form class-likes must not be flagged: {:?}",
            proposals
        );
    }

    #[test]
    fn scan_ignores_error_inheritance() {
        // Lock-B Sub-B2 verdict: `Error =>` prefix 撤廃 = 必須でなくなる、
        // 撤廃 ≠ 禁止。Error 継承構文は migration 対象外。
        let src = "Error => NotFound = @(msg: Str)\n";
        let proposals = scan_source(src, std::path::Path::new("test.td"));
        assert!(
            proposals.is_empty(),
            "Error inheritance must not be flagged: {:?}",
            proposals
        );
    }

    #[test]
    fn scan_ignores_legacy_buchi_pack_zero_arity() {
        // Lock-B Sub-B1 verdict: `Pilot = @(...)` ≡ `Pilot[] = @(...)`、
        // どちらも合法。migration 対象外。
        let src = "Pilot = @(name: Str)\n";
        let proposals = scan_source(src, std::path::Path::new("test.td"));
        assert!(
            proposals.is_empty(),
            "zero-arity sugar buchi pack must not be flagged: {:?}",
            proposals
        );
    }

    #[test]
    fn scan_handles_concrete_mold_args() {
        // 旧 Mold 構文で concrete 引数を含むケース
        let src = "Mold[:Int] => IntBox = @(value: Int)\n";
        let proposals = scan_source(src, std::path::Path::new("test.td"));
        assert_eq!(proposals.len(), 1);
        let p = &proposals[0];
        assert_eq!(p.legacy_header, "Mold[:Int] => IntBox");
        assert_eq!(p.proposed_header, "IntBox");
    }

    #[test]
    fn scan_handles_constrained_type_param() {
        // 旧 Mold 構文で型変数制約を含むケース
        let src = "Mold[T <= :Int] => IntBox[T] = @(value: T)\n";
        let proposals = scan_source(src, std::path::Path::new("test.td"));
        assert_eq!(proposals.len(), 1);
        let p = &proposals[0];
        assert_eq!(p.legacy_header, "Mold[T <= :Int] => IntBox[T]");
        assert_eq!(p.proposed_header, "IntBox[T]");
    }

    #[test]
    fn ast_helper_legacy_e30_kind_returns_mold() {
        let src = "Mold[T] => Box[T] = @(filling: T)\n";
        let (program, errors) = crate::parser::parse(src);
        assert!(errors.is_empty(), "parse errors: {:?}", errors);
        let stmt = program.statements.first().expect("expected one statement");
        match stmt {
            Statement::ClassLikeDef(def) => {
                assert!(def.is_legacy_e30_syntax());
                assert_eq!(def.legacy_e30_kind(), Some("mold"));
            }
            other => panic!("expected ClassLikeDef, got {:?}", other),
        }
    }

    #[test]
    fn ast_helper_returns_none_for_new_forms() {
        let src = "Box[T] = @(filling: T)\n";
        let (program, errors) = crate::parser::parse(src);
        assert!(errors.is_empty(), "parse errors: {:?}", errors);
        let stmt = program.statements.first().expect("expected one statement");
        match stmt {
            Statement::ClassLikeDef(def) => {
                assert!(!def.is_legacy_e30_syntax());
                assert_eq!(def.legacy_e30_kind(), None);
            }
            other => panic!("expected ClassLikeDef, got {:?}", other),
        }
    }
}

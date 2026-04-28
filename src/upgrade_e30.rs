//! `taida upgrade --e30 <path>` — E30 migration tool.
//!
//! E30 で確定した型構文 surface 統一 (`Name[?type-args] [=> Parent] = @(...)`) への
//! 旧構文 migrate を AST-aware に行う。
//!
//! ## Lock-E verdict (2026-04-28) 整合
//!
//! - 統合先: E31 `taida way migrate --<ver>` ハブ (E31B-004 subcommand 統合候補)
//! - E30 段階では `taida upgrade --e30 <PATH>` として稼働
//!   (D28 前例 `taida upgrade --d28` 継承)
//! - E31 ハブ統合時に `taida way migrate --e30` を `taida upgrade --e30` の
//!   alias / wrapper として追加予定
//! - deprecation policy: E gen は **deprecation なし、即破壊的変更**
//!   → 本 tool は旧構文を新構文に直接書き換える (warning フェーズなし)
//! - stable gate 必須条件: migration tool が動作することは `@e.30` stable
//!   宣言の必須条件 (Phase 7 sub-track A 完成済)
//!
//! ## CLI surface
//!
//! - 引数なし (default): in-place rewrite mode (旧構文を直接 file に書き戻す)
//! - `--check`: 旧構文があれば exit 非ゼロ、ファイルは変更しない
//! - `--dry-run`: proposal を stdout に印字、ファイルは変更しない
//! - `<PATH>`: 単一 `.td` または `.td` を再帰収集するディレクトリ
//!
//! ## Phase 7 (sub-track A) で land した要素
//!
//! - **AST-driven char-offset rewrite** of legacy `Mold[T] => Foo[T] = @(...)`
//!   class-like definitions. Strategy: locate the legacy `ClassLikeDef::span.start`
//!   (which the parser sets to the `Mold` token), scan forward to the FatArrow
//!   `=>`, skip whitespace, and replace `[span.start, child_name_start)` with the
//!   empty string. This drops the `Mold[...] => ` prefix verbatim while preserving
//!   exact whitespace, comments, and field bodies after the child header.
//! - In-place file rewrite (default mode). `--check` exits non-zero when legacy
//!   syntax is found; `--dry-run` prints proposals without touching files.
//! - Idempotency: a second run is a no-op because the rewrite drops the legacy
//!   prefix entirely. The new form (`Foo[T] = @(...)`) is recognised as
//!   `ClassLikeKind::BuchiPack` and `is_legacy_e30_syntax()` returns false.
//! - Multi-pattern coverage: single param (`Mold[T]`), multi param (`Mold[T, U]`),
//!   concrete arg (`Mold[:Int]`), constrained param (`Mold[T <= :Int]`), zero-arity
//!   child (`Mold[T] => Box`), and consecutive defs in one file.
//! - 4-backend parity is established by running migration-output `.td` files
//!   through `tests/parity.rs::e30b_001_*` style fixtures (added in
//!   `tests/e30b_001_unified_class_like.rs`); migration must be a behaviour-
//!   preserving textual transform on already-equivalent surface.
//!
//! ## Phase 7 sub-track B sub-step B-5 (2026-04-28、Lock-G Sub-G4 verdict) で land した要素
//!
//! - **Addon facade migration**: `<pkg>/native/addon.toml::[functions]` の各 entry に
//!   対し、対応する Taida-side facade (`<pkg>/taida/<stem>.td`) で
//!   `name <= RustAddon["name"](arity <= N)` 明示 binding が存在するか確認する。
//!   存在しない関数があれば、facade の `<<<` export 直前 (or 末尾) に missing
//!   binding を一括 append して書き戻す。`taida-lang/terminal` の 23 sentinel が
//!   主要対象 (TMB-029 解消)。
//! - **CLI surface拡張**: `taida upgrade --e30 <pkg-or-facade-path>` が package root
//!   または facade `.td` を直接指定された場合に addon facade 検出を行う。
//! - **Idempotent**: 既に明示 binding がある関数は touch しない、二度目の実行は no-op。

use crate::parser::{
    BuchiField, ClassLikeDef, ClassLikeKind, Expr, MoldHeaderArg, Statement, TypeExpr, parse,
};

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
    /// Total missing explicit RustAddon bindings detected across addon facades.
    pub addon_binding_count: usize,
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
            UpgradeE30Error::CheckFailed { legacy_count } => {
                write!(f, "{} E30 migration issue(s) need migration", legacy_count)
            }
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
/// Returns proposal metadata only; use [`upgrade_source`] (Phase 7) to obtain
/// a fully rewritten source string.
pub fn scan_source(source: &str, file: &std::path::Path) -> Vec<MigrationProposal> {
    let (program, errors) = parse(source);
    if !errors.is_empty() {
        // Parse errors → conservative: no proposals (caller decides).
        return Vec::new();
    }
    collect_proposals_from_program(&program, file)
}

// ── Phase 7 sub-track A: AST-driven char-offset rewrite ──────────────

/// A single source rewrite to apply. Char offsets, exclusive end. Mirrors
/// the D28 `upgrade_d28::Rewrite` shape so the apply algorithm can be kept
/// simple.
#[derive(Debug, Clone)]
struct Rewrite {
    /// Start char offset (0-based, into source).
    start: usize,
    /// End char offset (exclusive).
    end: usize,
    /// Replacement text. Phase 7 sub-track A always uses an empty string
    /// (legacy `Mold[...] => ` prefix is dropped).
    replacement: String,
}

/// Locate the char-offset range of the legacy `Mold[...] => ` prefix in
/// `source` for a given `ClassLikeDef` whose `kind` is `Mold`.
///
/// The parser sets `def.span.start` to the `M` of `Mold`. We scan forward
/// looking for the FatArrow `=>` token, then skip the trailing whitespace
/// so the rewrite preserves exactly the user's spacing before the child
/// name (`Mold[T] => Box[T]` → `Box[T]`, with no leading whitespace gap).
///
/// Returns `None` if the source between `span.start` and the file end does
/// not actually contain a `=>` (defensive: should not happen for a well-
/// formed legacy mold parsed by `parse`).
fn legacy_mold_prefix_range(source: &str, def: &ClassLikeDef) -> Option<(usize, usize)> {
    // Convert def.span.start (char offset) to a byte offset for slicing.
    let prefix_byte: usize = source
        .chars()
        .take(def.span.start)
        .map(char::len_utf8)
        .sum();
    // Search the remaining source for the first `=>` occurrence. Identifiers
    // and brackets in `Mold[T <= :Int]` cannot contain `=>` literally, so
    // first-match is unambiguous for the legacy header.
    let tail = source.get(prefix_byte..)?;
    let arrow_rel_byte = tail.find("=>")?;
    let after_arrow_byte = prefix_byte + arrow_rel_byte + "=>".len();

    // Skip ASCII whitespace after the `=>` so we land exactly on the child
    // name's first char (e.g. the `B` in `Box[T]`).
    let mut after_ws_byte = after_arrow_byte;
    while after_ws_byte < source.len() {
        let b = source.as_bytes()[after_ws_byte];
        if b == b' ' || b == b'\t' {
            after_ws_byte += 1;
        } else {
            break;
        }
    }

    // Convert the end byte offset back to a char offset by counting chars
    // up to that position. Identifiers / type params are ASCII per the
    // lexer's identifier rule, so this is exact.
    let end_char = source[..after_ws_byte].chars().count();
    Some((def.span.start, end_char))
}

/// Walk the parsed program once and collect a `Rewrite` for every legacy
/// `Mold[...] => Foo[...] = @(...)` class-like definition.
fn collect_legacy_rewrites(program: &crate::parser::Program, source: &str) -> Vec<Rewrite> {
    let mut out = Vec::new();
    for stmt in &program.statements {
        if let Statement::ClassLikeDef(def) = stmt
            && def.is_legacy_e30_syntax()
            && let ClassLikeKind::Mold { .. } = &def.kind
            && let Some((start, end)) = legacy_mold_prefix_range(source, def)
        {
            out.push(Rewrite {
                start,
                end,
                replacement: String::new(),
            });
        }
    }
    out
}

/// Apply collected rewrites to `source`. Sorts by start descending so that
/// applying the rewrites in order does not invalidate earlier (lower-
/// indexed) byte positions. Returns (new_source, num_rewrites_applied).
fn apply_rewrites(source: &str, mut rewrites: Vec<Rewrite>) -> (String, usize) {
    if rewrites.is_empty() {
        return (source.to_string(), 0);
    }
    rewrites.sort_by_key(|r| std::cmp::Reverse(r.start));
    rewrites.dedup_by(|a, b| a.start == b.start && a.end == b.end);

    // Build a char-offset → byte-offset lookup once. Source is owned-text
    // and may contain non-ASCII (comments, string literals), so we cannot
    // assume byte == char.
    let char_to_byte: Vec<usize> = std::iter::once(0)
        .chain(source.char_indices().map(|(i, c)| i + c.len_utf8()))
        .collect();
    let byte_at = |co: usize| -> usize {
        if co >= char_to_byte.len() {
            source.len()
        } else {
            char_to_byte[co]
        }
    };

    let mut buf = source.to_string();
    let count = rewrites.len();
    for r in rewrites {
        let bs = byte_at(r.start);
        let be = byte_at(r.end);
        buf.replace_range(bs..be, &r.replacement);
    }
    (buf, count)
}

/// Public entry (Phase 7 sub-track A): rewrite a single Taida source string
/// in memory by dropping every legacy `Mold[...] => ` prefix. Returns the
/// new source and the number of rewrites applied. Pure / deterministic so
/// the test suite can exercise it without any file I/O.
///
/// Idempotency: the function parses the input, locates legacy `ClassLikeDef`
/// nodes via the AST helper [`ClassLikeDef::is_legacy_e30_syntax`], and
/// rewrites only those. After one pass the legacy prefix is gone and the
/// definition is recognised as the new `BuchiPack` kind on re-parse, so a
/// second invocation finds zero proposals and returns the source unchanged.
pub fn upgrade_source(source: &str) -> (String, usize) {
    let (program, errors) = parse(source);
    if !errors.is_empty() {
        // Parse errors → conservative: leave file untouched, signal 0.
        return (source.to_string(), 0);
    }
    let rewrites = collect_legacy_rewrites(&program, source);
    apply_rewrites(source, rewrites)
}

/// Apply the upgrade to a single file at `path`, with optional check / dry-run
/// guards. Mirrors the `upgrade_d28::upgrade_file` shape so CLI dispatch can
/// stay symmetric.
pub fn upgrade_file(
    path: &std::path::Path,
    check_only: bool,
    dry_run: bool,
) -> std::io::Result<UpgradeFileResult> {
    let source = std::fs::read_to_string(path)?;
    let (new_source, rewrites) = upgrade_source(&source);
    let changed = rewrites > 0 && new_source != source;
    if changed && !check_only && !dry_run {
        std::fs::write(path, &new_source)?;
    }
    Ok(UpgradeFileResult {
        path: path.to_path_buf(),
        rewrites,
        changed,
    })
}

/// Per-file outcome of `upgrade_file`.
#[derive(Debug)]
pub struct UpgradeFileResult {
    pub path: std::path::PathBuf,
    pub rewrites: usize,
    pub changed: bool,
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

/// Public entry from the CLI: run the migration according to `config`.
/// Returns an `UpgradeE30Report` summarising the scan and (for default
/// mode) the rewrites applied.
///
/// Mode matrix (Phase 7 sub-track A):
///
/// | flag combo | proposals printed | files written | exit              |
/// |------------|-------------------|---------------|-------------------|
/// | `--check`  | yes (label `[check]`) | no    | `Err(CheckFailed)` if any legacy detected |
/// | `--dry-run`| yes (label `[dry-run]`) | no  | `Ok(report)`      |
/// | (default)  | yes (label `[upgraded]`) | **yes (in-place)** | `Ok(report)` |
pub fn run(config: UpgradeE30Config) -> Result<UpgradeE30Report, UpgradeE30Error> {
    let mut files = Vec::new();
    collect_td_files(&config.path, &mut files)?;

    let mut report = UpgradeE30Report {
        files_scanned: 0,
        legacy_count: 0,
        addon_binding_count: 0,
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

        if proposals.is_empty() {
            continue;
        }

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
            }
        }

        // Default mode: actually rewrite the file in place via upgrade_file.
        // The check / dry-run cases short-circuit so the file is never touched.
        if !config.check_only && !config.dry_run {
            match upgrade_file(f, config.check_only, config.dry_run) {
                Ok(result) => {
                    if result.changed {
                        println!(
                            "[upgraded] {} ({} legacy class-like def(s) migrated)",
                            f.display(),
                            result.rewrites
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Error rewriting {}: {}", f.display(), e);
                }
            }
        }

        report.proposals.extend(proposals);
    }

    if let Some((pkg_dir, stem)) = addon_facade_target(&config.path)? {
        let manifest_functions = read_manifest_functions(&pkg_dir)?;
        let facade_path = pkg_dir.join("taida").join(format!("{}.td", stem));
        let source = std::fs::read_to_string(&facade_path)?;
        let addon_proposals = collect_facade_addon_proposals(&source, &manifest_functions);
        report.addon_binding_count += addon_proposals.len();

        for p in &addon_proposals {
            if config.check_only {
                println!(
                    "[check] {} missing RustAddon binding: `{}` -> `{} <= RustAddon[\"{}\"](arity <= {})`",
                    facade_path.display(),
                    p.fn_name,
                    p.fn_name,
                    p.fn_name,
                    p.arity
                );
            } else if config.dry_run {
                println!(
                    "[dry-run] {} add RustAddon binding: `{} <= RustAddon[\"{}\"](arity <= {})`",
                    facade_path.display(),
                    p.fn_name,
                    p.fn_name,
                    p.arity
                );
            }
        }

        if !addon_proposals.is_empty() && !config.check_only && !config.dry_run {
            let (n, path) = upgrade_facade_file(&pkg_dir, &stem, false, false)?;
            if n > 0 {
                println!(
                    "[upgraded] {} ({} RustAddon binding(s) added)",
                    path.display(),
                    n
                );
            }
        }
    }

    let issue_count = report.legacy_count + report.addon_binding_count;
    if config.check_only && issue_count > 0 {
        return Err(UpgradeE30Error::CheckFailed {
            legacy_count: issue_count,
        });
    }

    Ok(report)
}

// ── Phase 7 sub-track B sub-step B-5: addon facade migration ──────────
//
// `taida-lang/terminal` etc. addon facade の **bare reference** から
// **明示 `RustAddon[...]` binding** への自動展開。Lock-G Sub-G4 verdict
// で legacy 暗黙 pre-inject が撤廃されたため、各 manifest [functions]
// entry には対応する `name <= RustAddon["name"](arity <= N)` が必要。
// 既存 facade に欠けている binding を検出して append する。

/// One missing `RustAddon[...]` binding proposal for an addon facade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonBindingProposal {
    /// camelCase function name as listed in `addon.toml::[functions]`.
    pub fn_name: String,
    /// Manifest-declared arity (matches the `arity <= N` field).
    pub arity: u32,
}

/// Returns the set of facade-bound RustAddon function names already present
/// as `Name <= RustAddon["name"](arity <= N)` assignments in `program`.
fn collect_existing_rust_addon_bindings(
    program: &crate::parser::Program,
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for stmt in &program.statements {
        if let Statement::Assignment(assign) = stmt
            && let Some((fn_name, _)) = extract_rust_addon_binding(&assign.value)
            && assign.target == fn_name
        {
            out.insert(fn_name);
        }
    }
    out
}

/// Match `Expr::MoldInst("RustAddon", [StringLit(fn)], [arity <= IntLit(n)])`
/// and extract `(fn_name, arity)` if the surface is well-formed.
fn extract_rust_addon_binding(expr: &Expr) -> Option<(String, u32)> {
    match expr {
        Expr::MoldInst(name, type_args, fields, _) if name == "RustAddon" => {
            let fn_name = match type_args.first() {
                Some(Expr::StringLit(s, _)) if type_args.len() == 1 => s.clone(),
                _ => return None,
            };
            if fields.len() != 1 {
                return None;
            }
            let f: &BuchiField = &fields[0];
            if f.name != "arity" {
                return None;
            }
            match &f.value {
                Expr::IntLit(n, _) if *n >= 0 => Some((fn_name, *n as u32)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Read manifest `[functions]` from `<pkg_dir>/native/addon.toml`. Minimal
/// TOML extraction (we only need `[functions] name = N` entries) so we
/// don't have to depend on a TOML crate at this layer.
fn read_manifest_functions(
    pkg_dir: &std::path::Path,
) -> std::io::Result<std::collections::BTreeMap<String, u32>> {
    let manifest_path = pkg_dir.join("native").join("addon.toml");
    let raw = std::fs::read_to_string(&manifest_path)?;
    let mut out = std::collections::BTreeMap::new();
    let mut in_functions = false;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_functions = line == "[functions]";
            continue;
        }
        if in_functions && let Some((k, v)) = line.split_once('=') {
            let k = k.trim().trim_matches('"').to_string();
            let v_str = v.trim();
            // strip trailing comment `# ...`
            let v_clean = v_str.split('#').next().unwrap_or(v_str).trim();
            if let Ok(n) = v_clean.parse::<u32>() {
                out.insert(k, n);
            }
        }
    }
    Ok(out)
}

fn read_manifest_package_id(pkg_dir: &std::path::Path) -> std::io::Result<Option<String>> {
    let manifest_path = pkg_dir.join("native").join("addon.toml");
    let raw = std::fs::read_to_string(&manifest_path)?;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "package"
        {
            let package_id = value
                .split('#')
                .next()
                .unwrap_or(value)
                .trim()
                .trim_matches('"')
                .to_string();
            if !package_id.is_empty() {
                return Ok(Some(package_id));
            }
        }
    }
    Ok(None)
}

fn addon_facade_target(
    path: &std::path::Path,
) -> std::io::Result<Option<(std::path::PathBuf, String)>> {
    if path.is_dir() && path.join("native").join("addon.toml").exists() {
        let stem = read_manifest_package_id(path)?
            .and_then(|id| id.rsplit('/').next().map(|s| s.to_string()))
            .or_else(|| {
                path.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            });
        return Ok(stem.map(|s| (path.to_path_buf(), s)));
    }

    if path.is_file()
        && path.extension().and_then(|s| s.to_str()) == Some("td")
        && let Some(taida_dir) = path.parent()
        && taida_dir.file_name().and_then(|s| s.to_str()) == Some("taida")
        && let Some(pkg_dir) = taida_dir.parent()
        && pkg_dir.join("native").join("addon.toml").exists()
        && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
    {
        return Ok(Some((pkg_dir.to_path_buf(), stem.to_string())));
    }

    Ok(None)
}

/// Compute the list of missing RustAddon bindings for a facade source given
/// the surrounding addon's manifest function table. Determinism: result is
/// sorted by manifest declaration order (BTreeMap iteration order).
pub fn collect_facade_addon_proposals(
    facade_source: &str,
    manifest_functions: &std::collections::BTreeMap<String, u32>,
) -> Vec<AddonBindingProposal> {
    let (program, errors) = parse(facade_source);
    if !errors.is_empty() {
        return Vec::new();
    }
    let existing = collect_existing_rust_addon_bindings(&program);
    let mut out = Vec::new();
    for (fn_name, arity) in manifest_functions {
        if !existing.contains(fn_name) {
            out.push(AddonBindingProposal {
                fn_name: fn_name.clone(),
                arity: *arity,
            });
        }
    }
    out
}

/// Render a missing-binding block (one binding per line, aligned to the
/// longest fn_name for readability) to be inserted in the facade source.
fn render_addon_binding_block(proposals: &[AddonBindingProposal]) -> String {
    if proposals.is_empty() {
        return String::new();
    }
    let max_len = proposals.iter().map(|p| p.fn_name.len()).max().unwrap_or(0);
    let mut out = String::new();
    out.push_str("// ── E30B-007 Sub-G4: explicit RustAddon[...] bindings (auto-migrated) ──\n");
    out.push_str("// `taida upgrade --e30` で legacy 暗黙 pre-inject から明示 binding に\n");
    out.push_str("// 自動展開された箇所。arity は `native/addon.toml` と同期する必要がある。\n\n");
    for p in proposals {
        out.push_str(&format!(
            "{:width$} <= RustAddon[\"{}\"](arity <= {})\n",
            p.fn_name,
            p.fn_name,
            p.arity,
            width = max_len
        ));
    }
    out
}

/// Locate the byte offset of the `<<< @(...)` export statement in the source.
/// Returns `None` if no export is found, in which case the caller should
/// append at the end of the file.
fn find_export_byte_offset(source: &str) -> Option<usize> {
    // Search for `<<<` token at start of line (skipping leading whitespace).
    // We scan line by line so a `<<<` inside a string literal or comment is
    // not falsely matched.
    let mut byte = 0usize;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("<<<") {
            return Some(byte);
        }
        byte += line.len();
    }
    None
}

/// Rewrite a facade source by inserting any missing RustAddon[...] bindings
/// just before the `<<<` export (or appended to EOF if absent).
/// Returns `(new_source, num_inserted_bindings)`.
pub fn upgrade_facade_source(
    facade_source: &str,
    manifest_functions: &std::collections::BTreeMap<String, u32>,
) -> (String, usize) {
    let proposals = collect_facade_addon_proposals(facade_source, manifest_functions);
    if proposals.is_empty() {
        return (facade_source.to_string(), 0);
    }
    let block = render_addon_binding_block(&proposals);
    let n = proposals.len();
    if let Some(offset) = find_export_byte_offset(facade_source) {
        let mut out = String::with_capacity(facade_source.len() + block.len() + 1);
        out.push_str(&facade_source[..offset]);
        out.push_str(&block);
        out.push('\n');
        out.push_str(&facade_source[offset..]);
        (out, n)
    } else {
        // No export — append to EOF (with separating newline if needed).
        let mut out = facade_source.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&block);
        (out, n)
    }
}

/// CLI helper: `taida upgrade --e30 <pkg-dir>` で package root を指定された
/// 場合に、`<pkg-dir>/taida/<stem>.td` facade を addon migration する。
/// `<stem>` は package id の `/` 後ろ segment (canonical name から復元する
/// のは tool 上層の責務、ここでは stem を引数で受け取る)。
///
/// Returns `Ok((rewrites_applied, facade_path))` or any I/O error.
pub fn upgrade_facade_file(
    pkg_dir: &std::path::Path,
    stem: &str,
    check_only: bool,
    dry_run: bool,
) -> std::io::Result<(usize, std::path::PathBuf)> {
    let manifest_functions = read_manifest_functions(pkg_dir)?;
    let facade_path = pkg_dir.join("taida").join(format!("{}.td", stem));
    let source = std::fs::read_to_string(&facade_path)?;
    let (new_source, n) = upgrade_facade_source(&source, &manifest_functions);
    if n > 0 && !check_only && !dry_run {
        std::fs::write(&facade_path, &new_source)?;
    }
    Ok((n, facade_path))
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

    // ── Phase 7 sub-track A: in-memory rewrite tests ────────────────

    #[test]
    fn upgrade_drops_single_param_mold_prefix() {
        let src = "Mold[T] => Box[T] = @(filling: T)\n";
        let (out, n) = upgrade_source(src);
        assert_eq!(out, "Box[T] = @(filling: T)\n", "got: {:?}", out);
        assert_eq!(n, 1);
    }

    #[test]
    fn upgrade_drops_multi_param_mold_prefix() {
        let src = "Mold[T, U] => Pair[T, U] = @(first: T, second: U)\n";
        let (out, n) = upgrade_source(src);
        assert_eq!(out, "Pair[T, U] = @(first: T, second: U)\n");
        assert_eq!(n, 1);
    }

    #[test]
    fn upgrade_drops_concrete_arg_mold_prefix() {
        let src = "Mold[:Int] => IntBox = @(value: Int)\n";
        let (out, n) = upgrade_source(src);
        assert_eq!(out, "IntBox = @(value: Int)\n");
        assert_eq!(n, 1);
    }

    #[test]
    fn upgrade_drops_constrained_param_mold_prefix() {
        let src = "Mold[T <= :Int] => IntBox[T] = @(value: T)\n";
        let (out, n) = upgrade_source(src);
        assert_eq!(out, "IntBox[T] = @(value: T)\n");
        assert_eq!(n, 1);
    }

    #[test]
    fn upgrade_handles_zero_arity_child() {
        // 親側 Mold[T] あり、子側に args なし
        let src = "Mold[T] => Box = @(value: T)\n";
        let (out, n) = upgrade_source(src);
        assert_eq!(out, "Box = @(value: T)\n");
        assert_eq!(n, 1);
    }

    #[test]
    fn upgrade_handles_consecutive_legacy_defs() {
        let src = "Mold[T] => Box[T] = @(filling: T)\n\
                   Mold[T, U] => Pair[T, U] = @(first: T, second: U)\n";
        let (out, n) = upgrade_source(src);
        let expected = "Box[T] = @(filling: T)\n\
                        Pair[T, U] = @(first: T, second: U)\n";
        assert_eq!(out, expected);
        assert_eq!(n, 2);
    }

    #[test]
    fn upgrade_is_idempotent() {
        // 1回目の書き換えで legacy prefix が消え、2回目以降は no-op
        let src = "Mold[T] => Box[T] = @(filling: T)\n";
        let (once, n1) = upgrade_source(src);
        assert_eq!(n1, 1);
        let (twice, n2) = upgrade_source(&once);
        assert_eq!(n2, 0, "second pass must be a no-op");
        assert_eq!(once, twice);
    }

    #[test]
    fn upgrade_leaves_new_form_untouched() {
        // 新構文 (Lock-B Sub-B1 / Sub-B2) は migration 対象外
        let src = "Pilot = @(name: Str)\n\
                   Pilot[] = @(name: Str)\n\
                   Box[T] = @(filling: T)\n\
                   Error => NotFound = @(msg: Str)\n";
        let (out, n) = upgrade_source(src);
        assert_eq!(out, src, "new-form must pass through unchanged");
        assert_eq!(n, 0);
    }

    #[test]
    fn upgrade_preserves_following_content() {
        // 旧構文の前後のコメント / 別 class-like def / 空行を維持
        let src = "// banner\n\
                   Mold[T] => Box[T] = @(filling: T)\n\
                   \n\
                   Pilot = @(name: Str)\n";
        let (out, n) = upgrade_source(src);
        let expected = "// banner\n\
                        Box[T] = @(filling: T)\n\
                        \n\
                        Pilot = @(name: Str)\n";
        assert_eq!(out, expected);
        assert_eq!(n, 1);
    }

    #[test]
    fn upgrade_preserves_doc_comments_above_def() {
        let src = "///@ Purpose: 任意型を箱に詰める鋳型\n\
                   Mold[T] => Box[T] = @(filling: T)\n";
        let (out, n) = upgrade_source(src);
        // doc-comments are above the legacy header and untouched by the
        // prefix-drop rewrite (span.start points at `Mold`).
        let expected = "///@ Purpose: 任意型を箱に詰める鋳型\n\
                        Box[T] = @(filling: T)\n";
        assert_eq!(out, expected);
        assert_eq!(n, 1);
    }

    #[test]
    fn upgrade_handles_extra_whitespace_after_arrow() {
        // `=>   ` の余分な空白は consume されて正しく `Box` 開始に到達
        let src = "Mold[T] =>     Box[T] = @(filling: T)\n";
        let (out, n) = upgrade_source(src);
        assert_eq!(out, "Box[T] = @(filling: T)\n");
        assert_eq!(n, 1);
    }

    #[test]
    fn upgrade_returns_unchanged_on_parse_error() {
        // パース失敗時は保守的に no-op を返す
        let src = "Mold[T] =>\n";
        let (out, n) = upgrade_source(src);
        assert_eq!(out, src);
        assert_eq!(n, 0);
    }

    // ── Phase 7 sub-track B sub-step B-5: addon facade migration tests ──

    fn manifest_2() -> std::collections::BTreeMap<String, u32> {
        let mut m = std::collections::BTreeMap::new();
        m.insert("terminalSize".to_string(), 0);
        m.insert("readKey".to_string(), 0);
        m
    }

    #[test]
    fn collect_addon_proposals_detects_all_missing_when_facade_empty() {
        let src = "<<< @(terminalSize, readKey)\n";
        let proposals = collect_facade_addon_proposals(src, &manifest_2());
        assert_eq!(proposals.len(), 2, "got: {:?}", proposals);
        // BTreeMap iteration order = alphabetical → readKey, terminalSize
        assert_eq!(proposals[0].fn_name, "readKey");
        assert_eq!(proposals[0].arity, 0);
        assert_eq!(proposals[1].fn_name, "terminalSize");
        assert_eq!(proposals[1].arity, 0);
    }

    #[test]
    fn collect_addon_proposals_skips_already_bound_fns() {
        // facade already has explicit binding for terminalSize → only readKey is proposed
        let src = concat!(
            "terminalSize <= RustAddon[\"terminalSize\"](arity <= 0)\n",
            "<<< @(terminalSize, readKey)\n",
        );
        let proposals = collect_facade_addon_proposals(src, &manifest_2());
        assert_eq!(proposals.len(), 1, "got: {:?}", proposals);
        assert_eq!(proposals[0].fn_name, "readKey");
    }

    #[test]
    fn collect_addon_proposals_no_op_when_all_bound() {
        let src = concat!(
            "terminalSize <= RustAddon[\"terminalSize\"](arity <= 0)\n",
            "readKey <= RustAddon[\"readKey\"](arity <= 0)\n",
            "<<< @(terminalSize, readKey)\n",
        );
        let proposals = collect_facade_addon_proposals(src, &manifest_2());
        assert!(proposals.is_empty(), "got: {:?}", proposals);
    }

    #[test]
    fn upgrade_facade_inserts_missing_bindings_before_export() {
        let src = concat!(
            "// header\n", //
            "<<< @(terminalSize, readKey)\n",
        );
        let (out, n) = upgrade_facade_source(src, &manifest_2());
        assert_eq!(n, 2);
        // Both bindings must appear before `<<<` (alignment may insert
        // padding so we match against the RHS substring only).
        let export_pos = out.find("<<<").expect("export must remain");
        let read_pos = out
            .find("RustAddon[\"readKey\"](arity <= 0)")
            .expect("readKey binding must be added");
        let term_pos = out
            .find("RustAddon[\"terminalSize\"](arity <= 0)")
            .expect("terminalSize binding must be added");
        assert!(read_pos < export_pos);
        assert!(term_pos < export_pos);
        // The original `// header` and `<<<` line are preserved
        assert!(out.contains("// header\n"));
        assert!(out.contains("<<< @(terminalSize, readKey)\n"));
    }

    #[test]
    fn upgrade_facade_is_idempotent() {
        let src = concat!(
            "// header\n", //
            "<<< @(terminalSize, readKey)\n",
        );
        let (once, n1) = upgrade_facade_source(src, &manifest_2());
        assert_eq!(n1, 2);
        let (twice, n2) = upgrade_facade_source(&once, &manifest_2());
        assert_eq!(n2, 0, "second pass must be a no-op");
        assert_eq!(once, twice);
    }

    #[test]
    fn upgrade_facade_preserves_existing_explicit_bindings() {
        let src = concat!(
            "terminalSize <= RustAddon[\"terminalSize\"](arity <= 0)\n",
            "<<< @(terminalSize, readKey)\n",
        );
        let (out, n) = upgrade_facade_source(src, &manifest_2());
        assert_eq!(n, 1, "only readKey should be inserted");
        // Original terminalSize binding must remain at original position
        assert!(out.contains("terminalSize <= RustAddon[\"terminalSize\"](arity <= 0)\n"));
        // readKey was inserted just before `<<<` (alignment-tolerant match)
        let read_pos = out.find("RustAddon[\"readKey\"](arity <= 0)").unwrap();
        let export_pos = out.find("<<<").unwrap();
        assert!(read_pos < export_pos);
    }

    #[test]
    fn extract_rust_addon_binding_recognises_well_formed_form() {
        let src = "f <= RustAddon[\"f\"](arity <= 3)\n";
        let (program, errors) = parse(src);
        assert!(errors.is_empty(), "{:?}", errors);
        let stmt = program.statements.first().unwrap();
        match stmt {
            Statement::Assignment(a) => {
                let r = extract_rust_addon_binding(&a.value);
                assert_eq!(r, Some(("f".to_string(), 3)));
            }
            other => panic!("expected Assignment, got {:?}", other),
        }
    }

    #[test]
    fn extract_rust_addon_binding_rejects_wrong_field_name() {
        // Field `arities` (not `arity`) should not be recognised as a RustAddon binding
        let src = "f <= RustAddon[\"f\"](arities <= 3)\n";
        let (program, errors) = parse(src);
        assert!(errors.is_empty(), "{:?}", errors);
        let stmt = program.statements.first().unwrap();
        match stmt {
            Statement::Assignment(a) => {
                let r = extract_rust_addon_binding(&a.value);
                assert!(r.is_none(), "wrong field name should be rejected: {:?}", r);
            }
            other => panic!("expected Assignment, got {:?}", other),
        }
    }

    #[test]
    fn upgrade_facade_appends_when_no_export_present() {
        let src = "// just doc comments and nothing else\n";
        let (out, n) = upgrade_facade_source(src, &manifest_2());
        assert_eq!(n, 2);
        assert!(out.contains("RustAddon[\"readKey\"](arity <= 0)"));
        assert!(out.contains("RustAddon[\"terminalSize\"](arity <= 0)"));
    }

    #[test]
    fn run_e30_migrates_addon_package_root_facade_bindings() {
        let tmp = std::env::temp_dir().join(format!(
            "e30_upgrade_addon_pkg_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("native")).unwrap();
        std::fs::create_dir_all(tmp.join("taida")).unwrap();
        std::fs::write(
            tmp.join("native").join("addon.toml"),
            r#"abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/demo"
library = "taida_lang_demo"

[functions]
foo = 0
"#,
        )
        .unwrap();
        std::fs::write(tmp.join("taida").join("demo.td"), "<<< @(foo)\n").unwrap();

        let report = run(UpgradeE30Config {
            path: tmp.clone(),
            check_only: false,
            dry_run: false,
        })
        .expect("upgrade should succeed");
        assert_eq!(report.addon_binding_count, 1);
        let migrated = std::fs::read_to_string(tmp.join("taida").join("demo.td")).unwrap();
        assert!(migrated.contains("foo <= RustAddon[\"foo\"](arity <= 0)"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn run_e30_check_fails_for_missing_addon_facade_bindings() {
        let tmp = std::env::temp_dir().join(format!(
            "e30_upgrade_addon_check_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("native")).unwrap();
        std::fs::create_dir_all(tmp.join("taida")).unwrap();
        std::fs::write(
            tmp.join("native").join("addon.toml"),
            r#"package = "taida-lang/demo"

[functions]
foo = 0
"#,
        )
        .unwrap();
        std::fs::write(tmp.join("taida").join("demo.td"), "<<< @(foo)\n").unwrap();

        let err = run(UpgradeE30Config {
            path: tmp.clone(),
            check_only: true,
            dry_run: false,
        })
        .expect_err("check should fail");
        match err {
            UpgradeE30Error::CheckFailed { legacy_count } => assert_eq!(legacy_count, 1),
            other => panic!("expected CheckFailed, got {:?}", other),
        }
        let unchanged = std::fs::read_to_string(tmp.join("taida").join("demo.td")).unwrap();
        assert!(!unchanged.contains("RustAddon"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

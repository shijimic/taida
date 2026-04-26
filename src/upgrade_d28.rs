//! `taida upgrade --d28 <path>` — D28B-007 AST-aware rewrite tool.
//!
//! Rewrites Taida source files to comply with the D28B-001 (2026-04-26 Phase 0
//! Lock) category-based naming rules. Operates on parsed AST (not on text
//! patterns) so the rewrite respects category × value-type:
//!
//! | Category                                        | Rule                     |
//! |-------------------------------------------------|--------------------------|
//! | クラスライク型 / モールド型 / スキーマ           | PascalCase               |
//! | 関数 / ぶちパックフィールド (関数値) / 変数 (関数値) | camelCase             |
//! | ぶちパックフィールド (非関数値) / 変数 (非関数値)   | snake_case             |
//! | 定数                                            | SCREAMING_SNAKE_CASE     |
//! | エラー variant                                   | PascalCase               |
//! | 型変数                                          | 単一大文字 (T/U/V/...)   |
//!
//! ## Scope (Round 3 wJ initial implementation)
//!
//! The most common pre-Lock violation in user code is **buchi-pack fields
//! holding non-function values (strings / ints / packs / lists) named in
//! camelCase**. Examples found in the existing codebase:
//!
//! - `examples/03_buchi_pack.td`: `@(callSign <= "Eva-02")`
//! - `examples/api_client.td`: `@(updatedBy: Str, reason: Str)`
//!
//! The Lock requires snake_case for non-function values
//! (`@(call_sign <= "Eva-02")`) and camelCase only when the value is a
//! function (e.g. `@(handler <= myFunc)`).
//!
//! This initial implementation handles **buchi-pack literal fields** with
//! detectable non-function value (`StringLit` / `IntLit` / `FloatLit` /
//! `BoolLit` / `BuchiPack` / `ListLit` / `MoldInst` of non-function molds).
//! Lambda values (`_ x = ...`) and Ident references are conservatively
//! treated as **function-shape ambiguous** and left untouched (camelCase
//! and snake_case both pass this category since the rule is value-typed).
//!
//! Schema field declarations (`name: Str` style in `User = @(name: Str, ...)`)
//! are also covered for non-function types.
//!
//! ## Idempotency
//!
//! Multiple invocations of `taida upgrade --d28 <path>` on the same source
//! must produce identical output. The `--check` flag returns exit code 1 if
//! any rewrite would happen, and exit code 0 if the file is already
//! compliant.
//!
//! ## Out of scope (Round 3 wJ initial)
//!
//! - PascalCase named type variables (`Item` → `T`): no hits in current
//!   codebase per audit, leave as future enhancement
//! - SCREAMING_SNAKE_CASE constant detection: requires usage tracking
//!   across statements (whether the name is reassigned), deferred
//! - Function-name renames: surface symbols are already Lock-compliant
//!   per Round 1 wA audit
//!
//! The tool does not modify symbols in `import` / `export` / cross-module
//! references — those require global rename which is out of scope.

use crate::parser::{
    BuchiField, CondArm, Expr, FieldDef, FuncDef, MoldDef, Param, Program, Statement, TypeDef,
    TypeExpr,
};

/// Returns true if `name` is a single-character ASCII upper letter
/// (PascalCase but length=1, used for type variables `T`/`U`/...). These
/// MUST be preserved verbatim and never rewritten.
#[allow(dead_code)]
fn is_single_letter_type_var(name: &str) -> bool {
    name.len() == 1 && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// Returns true if `name` is already valid snake_case (lowercase ASCII,
/// digits, and `_` only — no uppercase). Empty / leading-digit are also
/// considered "not violation" (parser would have rejected them earlier).
#[allow(dead_code)]
fn is_snake_case(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Returns true if `name` looks like camelCase: starts lowercase, contains
/// at least one uppercase letter, no underscores. Used to detect the
/// snake_case violation candidates among buchi-pack fields holding
/// non-function values.
fn is_camel_case(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let mut has_upper = false;
    for c in chars {
        if c == '_' {
            return false;
        }
        if c.is_ascii_uppercase() {
            has_upper = true;
        }
    }
    has_upper
}

/// Convert a camelCase identifier to snake_case.
/// `callSign` → `call_sign`, `updatedBy` → `updated_by`, `httpRequest` → `http_request`.
/// Already-snake names pass through unchanged.
pub fn camel_to_snake(name: &str) -> String {
    if !is_camel_case(name) {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Returns true if the expression value at this position is detectably a
/// **non-function** value, meaning the field holding it should follow
/// snake_case naming. Conservative: any uncertain case returns false
/// (leaves the field name untouched).
fn is_non_function_value(value: &Expr) -> bool {
    match value {
        // Concrete non-function literals
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::TemplateLit(..)
        | Expr::BoolLit(..)
        | Expr::Gorilla(..)
        | Expr::ListLit(..)
        | Expr::BuchiPack(..)
        // Field access / mold instantiation typically yields data
        | Expr::FieldAccess(..) => true,

        // Lambda / placeholder are function-typed
        Expr::Lambda(..) | Expr::Placeholder(..) | Expr::Hole(..) => false,

        // Ident: ambiguous (could be function variable or data variable).
        // Conservative: treat as ambiguous, do not rewrite.
        Expr::Ident(..) => false,

        // Function call returns whatever the function returns. Could be
        // data or could be a function (currying). Conservative: leave alone.
        Expr::FuncCall(..) | Expr::MethodCall(..) => false,

        // Mold instantiation: PascalCase mold names. Common data-shape
        // molds (`Result`, `Lax`, `Map`, `Filter`, ...) yield data values.
        // Conservative: treat as non-function so common data fields
        // (`@(items <= Map[xs, _ x = x])`) get renamed.
        Expr::MoldInst(..) => true,

        // Type instantiation `User(name <= ...)` yields a pack/data.
        Expr::TypeInst(..) => true,

        // Pipelines yield the pipeline's final value. Conservative: false
        // (could end in a function reference).
        Expr::Pipeline(..) => false,

        // Conditional / unmold / throw / binary / unary all return values
        // of various types. Binary ops on numbers/strings yield non-fn.
        // Conservative: only mark BinaryOp / UnaryOp as non-function.
        Expr::BinaryOp(..) | Expr::UnaryOp(..) => true,

        // Type literal / enum variant yield non-function tags.
        Expr::TypeLiteral(..) | Expr::EnumVariant(..) => true,

        // Unmold / cond / throw: conservative
        Expr::Unmold(..) | Expr::CondBranch(..) | Expr::Throw(..) => false,
    }
}

/// Returns true if the type expression denotes a non-function type
/// (used for schema-style declarations like `User = @(name: Str, age: Int)`).
fn is_non_function_type(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Named(name) => {
            // Concrete type names like Int / Str / Bool / Bytes / Float
            // are non-function values.
            !name.contains("=>")
        }
        TypeExpr::Generic(_, _) => true,
        TypeExpr::List(_) => true,
        TypeExpr::BuchiPack(_) => true,
        TypeExpr::Function(_, _) => false,
    }
}

/// A single rename to apply: replace `[start..end]` (char offsets) with `replacement`.
#[derive(Debug, Clone)]
struct Rewrite {
    /// Start char offset (0-based, into source).
    start: usize,
    /// End char offset (exclusive).
    end: usize,
    /// Replacement text.
    replacement: String,
}

/// Visitor that collects rewrites from a parsed `Program` AST.
struct UpgradeVisitor {
    rewrites: Vec<Rewrite>,
    /// Set of field names rewritten by the visitor (e.g. `callSign`).
    /// Used in a second pass to also rewrite matching `FieldAccess`
    /// expressions in the same file (heuristic best-effort: if a file
    /// declares `@(callSign <= ...)` then `obj.callSign` reads almost
    /// certainly refer to the same pack and should be renamed in
    /// lockstep). This keeps single-file `taida upgrade --d28` runs
    /// internally consistent without requiring whole-program type
    /// information.
    renamed_fields: std::collections::HashSet<String>,
}

impl UpgradeVisitor {
    fn new() -> Self {
        Self {
            rewrites: Vec::new(),
            renamed_fields: std::collections::HashSet::new(),
        }
    }

    /// Record a rewrite if the field name is camelCase AND the value is
    /// detectably non-function. This implements the D28B-001 Lock rule for
    /// "ぶちパックフィールド (非関数値) → snake_case".
    fn maybe_rewrite_buchi_field(&mut self, field: &BuchiField) {
        if !is_camel_case(&field.name) {
            return;
        }
        if !is_non_function_value(&field.value) {
            return;
        }
        let new_name = camel_to_snake(&field.name);
        if new_name == field.name {
            return;
        }
        // The `field.span` covers the whole `name <= value` expression.
        // We rename only the prefix `name` (length = field.name.chars().count()).
        let name_len = field.name.chars().count();
        self.rewrites.push(Rewrite {
            start: field.span.start,
            end: field.span.start + name_len,
            replacement: new_name.clone(),
        });
        // Track for second-pass FieldAccess rewriting.
        self.renamed_fields.insert(field.name.clone());
    }

    /// Same rule for schema-style field declarations (`name: Type`).
    /// Method fields (`is_method = true`) are skipped — those are function
    /// values and the camelCase form is correct per Lock.
    fn maybe_rewrite_field_def(&mut self, field: &FieldDef) {
        if field.is_method {
            return;
        }
        if !is_camel_case(&field.name) {
            return;
        }
        let Some(ty) = &field.type_annotation else {
            return;
        };
        if !is_non_function_type(ty) {
            return;
        }
        let new_name = camel_to_snake(&field.name);
        if new_name == field.name {
            return;
        }
        let name_len = field.name.chars().count();
        self.rewrites.push(Rewrite {
            start: field.span.start,
            end: field.span.start + name_len,
            replacement: new_name.clone(),
        });
        self.renamed_fields.insert(field.name.clone());
    }

    fn visit_program(&mut self, program: &Program) {
        for stmt in &program.statements {
            self.visit_statement(stmt);
        }
    }

    fn visit_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Expr(e) => self.visit_expr(e),
            Statement::Assignment(a) => self.visit_expr(&a.value),
            Statement::FuncDef(f) => self.visit_func_def(f),
            Statement::TypeDef(td) => self.visit_type_def(td),
            Statement::MoldDef(md) => self.visit_mold_def(md),
            Statement::InheritanceDef(id) => {
                for f in &id.fields {
                    self.maybe_rewrite_field_def(f);
                    if let Some(default) = &f.default_value {
                        self.visit_expr(default);
                    }
                    if let Some(method) = &f.method_def {
                        self.visit_func_def(method);
                    }
                }
            }
            Statement::ErrorCeiling(ec) => {
                for s in &ec.handler_body {
                    self.visit_statement(s);
                }
            }
            Statement::UnmoldForward(u) => self.visit_expr(&u.source),
            Statement::UnmoldBackward(u) => self.visit_expr(&u.source),
            // No-rewrite statements: enums (variant names are PascalCase by
            // construction), imports, exports.
            Statement::EnumDef(_) | Statement::Import(_) | Statement::Export(_) => {}
        }
    }

    fn visit_type_def(&mut self, td: &TypeDef) {
        for f in &td.fields {
            self.maybe_rewrite_field_def(f);
            if let Some(default) = &f.default_value {
                self.visit_expr(default);
            }
            if let Some(method) = &f.method_def {
                self.visit_func_def(method);
            }
        }
    }

    fn visit_mold_def(&mut self, md: &MoldDef) {
        for f in &md.fields {
            self.maybe_rewrite_field_def(f);
            if let Some(default) = &f.default_value {
                self.visit_expr(default);
            }
            if let Some(method) = &f.method_def {
                self.visit_func_def(method);
            }
        }
    }

    fn visit_func_def(&mut self, f: &FuncDef) {
        for s in &f.body {
            self.visit_statement(s);
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::BuchiPack(fields, _) => {
                for f in fields {
                    self.maybe_rewrite_buchi_field(f);
                    self.visit_expr(&f.value);
                }
            }
            Expr::ListLit(items, _) => {
                for it in items {
                    self.visit_expr(it);
                }
            }
            Expr::BinaryOp(a, _, b, _) => {
                self.visit_expr(a);
                self.visit_expr(b);
            }
            Expr::UnaryOp(_, e, _) => self.visit_expr(e),
            Expr::FuncCall(callee, args, _) => {
                self.visit_expr(callee);
                for a in args {
                    self.visit_expr(a);
                }
            }
            Expr::MethodCall(recv, _, args, _) => {
                self.visit_expr(recv);
                for a in args {
                    self.visit_expr(a);
                }
            }
            Expr::FieldAccess(e, _, _) => self.visit_expr(e),
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    self.visit_cond_arm(arm);
                }
            }
            Expr::Pipeline(steps, _) => {
                for s in steps {
                    self.visit_expr(s);
                }
            }
            Expr::MoldInst(_, args, fields, _) => {
                for a in args {
                    self.visit_expr(a);
                }
                for f in fields {
                    self.maybe_rewrite_buchi_field(f);
                    self.visit_expr(&f.value);
                }
            }
            Expr::Unmold(e, _) => self.visit_expr(e),
            Expr::Lambda(_, body, _) => self.visit_expr(body),
            Expr::TypeInst(_, fields, _) => {
                for f in fields {
                    self.maybe_rewrite_buchi_field(f);
                    self.visit_expr(&f.value);
                }
            }
            Expr::Throw(e, _) => self.visit_expr(e),
            // Leaf nodes: nothing to recurse into.
            _ => {}
        }
    }

    fn visit_cond_arm(&mut self, arm: &CondArm) {
        if let Some(c) = &arm.condition {
            self.visit_expr(c);
        }
        for s in &arm.body {
            self.visit_statement(s);
        }
    }
}

// Param visitor (functions): currently no rewrites applied here because
// argument names are scoped per-function and rewriting would require
// global call-site updating. Kept as an explicit no-op for clarity.
#[allow(dead_code)]
fn visit_params(_params: &[Param]) {}

/// Apply collected rewrites to source. Returns (new_source, num_rewrites).
fn apply_rewrites(source: &str, mut rewrites: Vec<Rewrite>) -> (String, usize) {
    if rewrites.is_empty() {
        return (source.to_string(), 0);
    }
    // Sort by start desc so byte-offset shifting after a replacement does
    // not invalidate earlier (lower-indexed) rewrite positions.
    rewrites.sort_by(|a, b| b.start.cmp(&a.start));

    // Convert char offsets to byte offsets via the source's `char_indices`.
    // Build a forward lookup table once.
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
    for rw in rewrites {
        let bs = byte_at(rw.start);
        let be = byte_at(rw.end);
        buf.replace_range(bs..be, &rw.replacement);
    }
    (buf, count)
}

/// Configuration for the upgrade run.
#[derive(Debug, Clone)]
pub struct UpgradeD28Config {
    /// File or directory path to process. Directory recursion processes
    /// every `.td` file.
    pub path: std::path::PathBuf,
    /// `--check` mode: do not write changes; return non-zero exit if any
    /// would be applied.
    pub check_only: bool,
    /// `--dry-run`: print rewrites but do not write.
    pub dry_run: bool,
}

/// Result of upgrading a single file.
#[derive(Debug)]
pub struct UpgradeFileResult {
    pub path: std::path::PathBuf,
    pub rewrites: usize,
    pub changed: bool,
}

/// Second-pass visitor: rewrites `FieldAccess` reads (`obj.callSign`)
/// whose field name was renamed in the first pass. Heuristic best-effort:
/// if a file declares a literal `@(callSign <= ...)` that we renamed to
/// `call_sign`, then ALL `obj.callSign` reads in the same file almost
/// certainly refer to the same pack and should be renamed in lockstep.
/// This keeps single-file `taida upgrade --d28` outputs internally
/// consistent without whole-program type information.
struct FieldAccessRewriter<'a> {
    rewrites: Vec<Rewrite>,
    renamed_fields: &'a std::collections::HashSet<String>,
    source: &'a str,
    char_to_byte: Vec<usize>,
}

impl<'a> FieldAccessRewriter<'a> {
    fn new(source: &'a str, renamed_fields: &'a std::collections::HashSet<String>) -> Self {
        let char_to_byte: Vec<usize> = std::iter::once(0)
            .chain(source.char_indices().map(|(i, c)| i + c.len_utf8()))
            .collect();
        Self {
            rewrites: Vec::new(),
            renamed_fields,
            source,
            char_to_byte,
        }
    }

    fn byte_at(&self, char_offset: usize) -> usize {
        if char_offset >= self.char_to_byte.len() {
            self.source.len()
        } else {
            self.char_to_byte[char_offset]
        }
    }

    /// For a `FieldAccess` expression, find the `.field` text within the
    /// span and emit a rewrite if `field` is in the renamed set. Walks
    /// from the END of the span backwards looking for `.<name>`.
    fn maybe_rewrite_field_access(
        &mut self,
        field_name: &str,
        span_start_char: usize,
        span_end_char: usize,
    ) {
        if !self.renamed_fields.contains(field_name) {
            return;
        }
        let new_name = camel_to_snake(field_name);
        if new_name == field_name {
            return;
        }
        // Source text within span (byte slice).
        let bs = self.byte_at(span_start_char);
        let be = self.byte_at(span_end_char);
        let region = &self.source[bs..be];
        // Find the LAST occurrence of `.<field_name>` in the region —
        // for chained accesses (`a.b.c.callSign`) this targets the
        // outermost `.callSign`. Other matches are nested FieldAccess
        // expressions and will be visited separately.
        let needle = format!(".{}", field_name);
        if let Some(rel_byte) = region.rfind(&needle) {
            // Convert region byte offset back to char offset.
            // Since identifiers are ASCII, rel_byte == rel_char for our
            // purposes within a Latin-only region. For safety, count chars
            // in region[..rel_byte].
            let rel_char = region[..rel_byte].chars().count();
            // The `.field` substring starts at `rel_char` (the dot).
            // The field name itself starts at `rel_char + 1`.
            let name_start_char = span_start_char + rel_char + 1;
            let name_end_char = name_start_char + field_name.chars().count();
            self.rewrites.push(Rewrite {
                start: name_start_char,
                end: name_end_char,
                replacement: new_name,
            });
        }
    }

    fn visit_program(&mut self, program: &Program) {
        for stmt in &program.statements {
            self.visit_statement(stmt);
        }
    }

    fn visit_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Expr(e) => self.visit_expr(e),
            Statement::Assignment(a) => self.visit_expr(&a.value),
            Statement::FuncDef(f) => {
                for s in &f.body {
                    self.visit_statement(s);
                }
            }
            Statement::TypeDef(td) => {
                for f in &td.fields {
                    if let Some(default) = &f.default_value {
                        self.visit_expr(default);
                    }
                    if let Some(method) = &f.method_def {
                        for s in &method.body {
                            self.visit_statement(s);
                        }
                    }
                }
            }
            Statement::MoldDef(md) => {
                for f in &md.fields {
                    if let Some(default) = &f.default_value {
                        self.visit_expr(default);
                    }
                    if let Some(method) = &f.method_def {
                        for s in &method.body {
                            self.visit_statement(s);
                        }
                    }
                }
            }
            Statement::InheritanceDef(id) => {
                for f in &id.fields {
                    if let Some(default) = &f.default_value {
                        self.visit_expr(default);
                    }
                    if let Some(method) = &f.method_def {
                        for s in &method.body {
                            self.visit_statement(s);
                        }
                    }
                }
            }
            Statement::ErrorCeiling(ec) => {
                for s in &ec.handler_body {
                    self.visit_statement(s);
                }
            }
            Statement::UnmoldForward(u) => self.visit_expr(&u.source),
            Statement::UnmoldBackward(u) => self.visit_expr(&u.source),
            _ => {}
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::FieldAccess(recv, field_name, span) => {
                self.visit_expr(recv);
                self.maybe_rewrite_field_access(field_name, span.start, span.end);
            }
            Expr::BuchiPack(fields, _) => {
                for f in fields {
                    self.visit_expr(&f.value);
                }
            }
            Expr::ListLit(items, _) => {
                for it in items {
                    self.visit_expr(it);
                }
            }
            Expr::BinaryOp(a, _, b, _) => {
                self.visit_expr(a);
                self.visit_expr(b);
            }
            Expr::UnaryOp(_, e, _) => self.visit_expr(e),
            Expr::FuncCall(callee, args, _) => {
                self.visit_expr(callee);
                for a in args {
                    self.visit_expr(a);
                }
            }
            Expr::MethodCall(recv, _, args, _) => {
                self.visit_expr(recv);
                for a in args {
                    self.visit_expr(a);
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(c) = &arm.condition {
                        self.visit_expr(c);
                    }
                    for s in &arm.body {
                        self.visit_statement(s);
                    }
                }
            }
            Expr::Pipeline(steps, _) => {
                for s in steps {
                    self.visit_expr(s);
                }
            }
            Expr::MoldInst(_, args, fields, _) => {
                for a in args {
                    self.visit_expr(a);
                }
                for f in fields {
                    self.visit_expr(&f.value);
                }
            }
            Expr::Unmold(e, _) => self.visit_expr(e),
            Expr::Lambda(_, body, _) => self.visit_expr(body),
            Expr::TypeInst(_, fields, _) => {
                for f in fields {
                    self.visit_expr(&f.value);
                }
            }
            Expr::Throw(e, _) => self.visit_expr(e),
            // Note: TemplateLit string interpolations like `${pilot.callSign}`
            // are stored as raw template text and not re-parsed into Expr at
            // this level. They are handled by a separate template-string
            // pass (below in `rewrite_template_strings`).
            _ => {}
        }
    }
}

/// Best-effort rewrite of field references inside template strings
/// (`` `Name: ${pilot.callSign}` ``). The parser stores templates as
/// raw strings, so we cannot use AST spans here. Instead, we collect
/// the (renamed_old, new) pairs and replace `\.<old>` patterns in the
/// source byte range covered by template-literal tokens.
fn rewrite_template_strings(
    source: &str,
    renamed_fields: &std::collections::HashSet<String>,
) -> Vec<Rewrite> {
    let mut out = Vec::new();
    if renamed_fields.is_empty() {
        return out;
    }
    // For each renamed field, find every byte occurrence of `${...<.field>...}`
    // by simple textual scan inside backtick-delimited spans.
    // NOTE: This is intentionally a textual heuristic. The simpler scan
    // is "every `.<field>` in source where `<field>` is in renamed_fields"
    // — but that would also rewrite comments / string contents we should
    // not touch. Restricting to template strings (the only place where
    // the Taida parser does NOT re-parse interpolations into AST in this
    // codebase) is the minimum-surprise scope.

    // Collect byte ranges of template literals (regions enclosed in `).
    let bytes = source.as_bytes();
    let mut tpl_ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    let mut in_string = None::<u8>;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(quote) = in_string {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == quote {
                if quote == b'`' {
                    // close
                }
                in_string = None;
            }
            i += 1;
            continue;
        }
        // Skip line comments
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'"' || b == b'\'' {
            in_string = Some(b);
            i += 1;
            continue;
        }
        if b == b'`' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] != b'`' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            tpl_ranges.push((start, i));
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        i += 1;
    }

    // For each template range and each renamed field, find `.<field>`
    // followed by a non-identifier byte (so `.callSignX` is not matched).
    for (start, end) in tpl_ranges {
        let region = &source[start..end];
        for old_name in renamed_fields {
            let needle = format!(".{}", old_name);
            let mut search_start = 0;
            while let Some(found) = region[search_start..].find(&needle) {
                let abs_byte_start = start + search_start + found;
                let after = abs_byte_start + needle.len();
                let next_byte = source.as_bytes().get(after).copied().unwrap_or(0);
                let is_ident_continuer = next_byte.is_ascii_alphanumeric() || next_byte == b'_';
                if !is_ident_continuer {
                    // Replace just the field-name portion (skip the dot).
                    let name_byte_start = abs_byte_start + 1;
                    let _name_byte_end = name_byte_start + old_name.len();
                    // Convert to char offsets for Rewrite (assuming ASCII
                    // identifiers; in surrounding non-ASCII text the start
                    // would differ, but field names themselves are ASCII).
                    let name_char_start = source[..name_byte_start].chars().count();
                    let name_char_end = name_char_start + old_name.chars().count();
                    out.push(Rewrite {
                        start: name_char_start,
                        end: name_char_end,
                        replacement: camel_to_snake(old_name),
                    });
                }
                search_start += found + needle.len();
            }
        }
    }
    out
}

/// Public entry: rewrite a single Taida source string. Returns the new
/// source and the number of rewrites applied. Used by both the CLI and
/// the regression tests so the function is pure / deterministic.
pub fn upgrade_source(source: &str) -> (String, usize) {
    let (program, errors) = crate::parser::parse(source);
    if !errors.is_empty() {
        // Parse errors → conservative: leave file untouched. The caller
        // sees `rewrites = 0` and unchanged source.
        return (source.to_string(), 0);
    }
    let mut visitor = UpgradeVisitor::new();
    visitor.visit_program(&program);

    // Second pass: rewrite FieldAccess reads in the same file.
    let mut all_rewrites = visitor.rewrites;
    let renamed = visitor.renamed_fields;
    if !renamed.is_empty() {
        let mut access_rewriter = FieldAccessRewriter::new(source, &renamed);
        access_rewriter.visit_program(&program);
        all_rewrites.extend(access_rewriter.rewrites);

        // Third pass: rewrite template-string interpolation references.
        all_rewrites.extend(rewrite_template_strings(source, &renamed));
    }

    apply_rewrites(source, all_rewrites)
}

/// Apply the upgrade to a single file at `path`, returning the result.
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

/// Recursively walk a directory and collect all `.td` files.
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
            // Skip dotted directories (`.git`, `.dev`, target/) and
            // build artifacts.
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

/// Public entry from the CLI: run the upgrade according to `config`.
/// Returns (total_rewrites, files_changed). Errors are printed to stderr.
pub fn run(config: UpgradeD28Config) -> Result<(usize, usize), String> {
    let mut files = Vec::new();
    collect_td_files(&config.path, &mut files)
        .map_err(|e| format!("Failed to read {}: {}", config.path.display(), e))?;

    if files.is_empty() {
        eprintln!("No .td files found under {}", config.path.display());
        return Ok((0, 0));
    }

    let mut total_rewrites = 0usize;
    let mut files_changed = 0usize;
    for f in &files {
        match upgrade_file(f, config.check_only, config.dry_run) {
            Ok(r) => {
                if r.rewrites > 0 {
                    total_rewrites += r.rewrites;
                    files_changed += 1;
                    if config.check_only {
                        println!(
                            "[check] {} would have {} rewrite(s)",
                            r.path.display(),
                            r.rewrites
                        );
                    } else if config.dry_run {
                        println!("[dry-run] {} +{} rewrite(s)", r.path.display(), r.rewrites);
                    } else {
                        println!(
                            "[upgraded] {} ({} rewrite(s))",
                            r.path.display(),
                            r.rewrites
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("Error processing {}: {}", f.display(), e);
            }
        }
    }

    if config.check_only && total_rewrites > 0 {
        return Err(format!(
            "{} file(s) need upgrade ({} total rewrite(s))",
            files_changed, total_rewrites
        ));
    }
    Ok((total_rewrites, files_changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_to_snake_basic() {
        assert_eq!(camel_to_snake("callSign"), "call_sign");
        assert_eq!(camel_to_snake("updatedBy"), "updated_by");
        assert_eq!(camel_to_snake("httpRequest"), "http_request");
        // Already snake_case: unchanged
        assert_eq!(camel_to_snake("call_sign"), "call_sign");
        // Already lowercase no-cap: unchanged (not camelCase)
        assert_eq!(camel_to_snake("foo"), "foo");
        // PascalCase: not camelCase → unchanged
        assert_eq!(camel_to_snake("MyType"), "MyType");
    }

    #[test]
    fn detects_camel_case() {
        assert!(is_camel_case("callSign"));
        assert!(is_camel_case("httpRequest"));
        assert!(!is_camel_case("call_sign"));
        assert!(!is_camel_case("MyType"));
        assert!(!is_camel_case("foo"));
    }

    #[test]
    fn rewrite_string_field_pack_literal() {
        let src = r#"pilot <= @(name <= "Asuka", callSign <= "Eva-02")
stdout(pilot.callSign)
"#;
        let (out, n) = upgrade_source(src);
        // Field with non-function string value should rename.
        assert!(out.contains("call_sign <= \"Eva-02\""), "got: {}", out);
        // Note: the field-access `pilot.callSign` is NOT auto-renamed
        // (would require type-aware rename of read sites; current scope
        // is the literal field declaration only). This is acknowledged in
        // the module docstring.
        assert_eq!(n, 1);
    }

    #[test]
    fn idempotent_double_run() {
        let src = r#"@(callSign <= "X")
"#;
        let (once, _) = upgrade_source(src);
        let (twice, n2) = upgrade_source(&once);
        assert_eq!(once, twice, "second run must be a no-op");
        assert_eq!(n2, 0);
    }

    #[test]
    fn function_value_field_untouched() {
        // Lambda value: function-shape, camelCase is correct → no rewrite.
        let src = r#"obj <= @(safeDiv <= _ x y = x / y)
"#;
        let (out, n) = upgrade_source(src);
        assert_eq!(out, src);
        assert_eq!(n, 0);
    }

    #[test]
    fn schema_field_def_rewrite() {
        // Schema-style field declarations: type-typed, non-function, camelCase
        // → snake_case.
        let src = r#"User = @(callSign: Str, age: Int)
"#;
        let (out, n) = upgrade_source(src);
        assert!(out.contains("call_sign: Str"), "got: {}", out);
        assert_eq!(n, 1);
    }
}

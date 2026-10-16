//! Documentation generator for Taida Lang.
//!
//! Extracts doc comments (`///@`) from AST nodes and generates Markdown documentation.

use crate::parser::{
    Assignment, ClassLikeDef, ClassLikeKind, ExportStmt, FieldDef, FuncDef, MoldHeaderArg, Program,
    Statement, TypeExpr, TypeParam,
};

// ── Data structures ─────────────────────────────────────────────────

/// Parsed documentation tags from `///@` comments.
#[derive(Debug, Clone, Default)]
pub struct DocTags {
    /// Free-form description lines (lines without a recognized tag).
    pub description: Vec<String>,
    /// `@Purpose` tag value.
    pub purpose: Option<String>,
    /// `@Params` tag entries (name, description).
    pub params: Vec<(String, String)>,
    /// `@Returns` tag value.
    pub returns: Option<String>,
    /// `@Throws` tag entries.
    pub throws: Vec<String>,
    /// `@Example` tag lines.
    pub example: Vec<String>,
    /// `@Since` tag value.
    pub since: Option<String>,
    /// `@Deprecated` tag value.
    pub deprecated: Option<String>,
    /// `@See` tag value.
    pub see: Option<String>,
    // AI collaboration tags
    /// `@AI-Context` tag lines.
    pub ai_context: Vec<String>,
    /// `@AI-Hint` tag lines.
    pub ai_hint: Vec<String>,
    /// `@AI-Examples` tag lines.
    pub ai_examples: Vec<String>,
    /// `@AI-Constraints` tag lines.
    pub ai_constraints: Vec<String>,
    /// `@AI-Related` tag lines.
    pub ai_related: Vec<String>,
    /// `@AI-Category` tag value.
    pub ai_category: Option<String>,
    /// `@AI-Complexity` tag lines.
    pub ai_complexity: Vec<String>,
    /// `@AI-SideEffects` tag lines.
    pub ai_side_effects: Vec<String>,
}

impl DocTags {
    /// Whether no tag and no description was written.
    pub fn is_empty(&self) -> bool {
        self.description.is_empty()
            && self.purpose.is_none()
            && self.params.is_empty()
            && self.returns.is_none()
            && self.throws.is_empty()
            && self.example.is_empty()
            && self.since.is_none()
            && self.deprecated.is_none()
            && self.see.is_none()
            && self.ai_context.is_empty()
            && self.ai_hint.is_empty()
            && self.ai_examples.is_empty()
            && self.ai_constraints.is_empty()
            && self.ai_related.is_empty()
            && self.ai_category.is_none()
            && self.ai_complexity.is_empty()
            && self.ai_side_effects.is_empty()
    }
}

/// Documentation for a single field.
#[derive(Debug, Clone)]
pub struct FieldDoc {
    pub name: String,
    pub type_str: Option<String>,
    pub doc: String,
}

/// Documentation for a type definition.
#[derive(Debug, Clone)]
pub struct TypeDoc {
    pub name: String,
    pub tags: DocTags,
    pub fields: Vec<FieldDoc>,
    /// Methods declared in the type's body.
    pub methods: Vec<FuncDoc>,
}

/// Documentation for an enum definition.
#[derive(Debug, Clone)]
pub struct EnumDoc {
    pub name: String,
    pub tags: DocTags,
    pub variants: Vec<String>,
}

/// Documentation for a function definition.
#[derive(Debug, Clone)]
pub struct FuncDoc {
    pub name: String,
    pub type_params: Vec<String>,
    pub tags: DocTags,
    pub params: Vec<(String, Option<String>)>,
    pub return_type: Option<String>,
}

/// Documentation for a mold definition.
#[derive(Debug, Clone)]
pub struct MoldDoc {
    pub name: String,
    pub header_args: Vec<String>,
    pub tags: DocTags,
    pub fields: Vec<FieldDoc>,
    pub methods: Vec<FuncDoc>,
}

/// Documentation for an inheritance definition.
#[derive(Debug, Clone)]
pub struct InheritDoc {
    pub parent: String,
    pub parent_header_args: Vec<String>,
    pub child: String,
    pub child_header_args: Vec<String>,
    pub tags: DocTags,
    pub fields: Vec<FieldDoc>,
    pub methods: Vec<FuncDoc>,
}

/// Documentation for a variable assignment.
#[derive(Debug, Clone)]
pub struct AssignmentDoc {
    pub name: String,
    pub tags: DocTags,
}

/// Documentation for the entire module.
#[derive(Debug, Clone)]
pub struct ModuleDoc {
    pub name: String,
    pub types: Vec<TypeDoc>,
    pub enums: Vec<EnumDoc>,
    pub functions: Vec<FuncDoc>,
    pub molds: Vec<MoldDoc>,
    pub inheritances: Vec<InheritDoc>,
    pub assignments: Vec<AssignmentDoc>,
    pub exports: Vec<String>,
}

// ── Tag parsing ─────────────────────────────────────────────────────

/// Known tag names (single-line and multi-line).
const SINGLE_LINE_TAGS: &[&str] = &[
    "Purpose",
    "Returns",
    "Since",
    "Deprecated",
    "See",
    "AI-Category",
];

const MULTI_LINE_TAGS: &[&str] = &[
    "Params",
    "Throws",
    "Example",
    "AI-Context",
    "AI-Hint",
    "AI-Examples",
    "AI-Constraints",
    "AI-Related",
    "AI-Complexity",
    "AI-SideEffects",
];

/// Parse doc comment lines into structured tags.
///
/// Each line in `comments` is the raw content after stripping the `///@` prefix
/// (the parser already does this).
///
/// A tag runs from its own line to the next tag line, so the lines after it
/// belong to it: a value that wraps over several lines is read whole, list
/// items (`Params`, `Throws`, ...) are one per line, and the code of an
/// `Example` keeps its indentation and blank lines. A line that opens no
/// known tag is a continuation of the tag before it (free-form description
/// when no tag has been seen yet), so nothing written is dropped.
pub fn parse_doc_tags(comments: &[String]) -> DocTags {
    let mut tags = DocTags::default();
    // The tag being read: its name and the lines that belong to it, the
    // first being whatever follows the tag name on its own line.
    let mut current: Option<(String, Vec<String>)> = None;

    for line in comments {
        if let Some((tag_name, rest)) = try_match_tag(line.trim()) {
            if let Some((tag, lines)) = current.take() {
                push_tag_block(&mut tags, &tag, &lines);
            }
            current = Some((tag_name, vec![rest]));
        } else if let Some((_, lines)) = current.as_mut() {
            lines.push(line.clone());
        } else {
            // Free-form description (no tag context yet)
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                tags.description.push(trimmed.to_string());
            }
        }
    }
    if let Some((tag, lines)) = current.take() {
        push_tag_block(&mut tags, &tag, &lines);
    }

    tags
}

/// Try to match a tag at the start of a line. Returns (tag_name, rest_of_line).
///
/// Accepts both `@Purpose:...` (raw doc comment content where `///@` prefix
/// left the `@`) and `Purpose:...` (where the lexer already consumed the `@`
/// as part of the `///@` prefix).
pub(crate) fn try_match_tag(line: &str) -> Option<(String, String)> {
    // Strip optional leading `@`
    let body = line.strip_prefix('@').unwrap_or(line);

    // Try each known tag (case-sensitive)
    for &tag in SINGLE_LINE_TAGS.iter().chain(MULTI_LINE_TAGS.iter()) {
        if let Some(remaining) = body.strip_prefix(tag) {
            // Must be followed by `:`, whitespace, or end of line
            if remaining.is_empty() || remaining.starts_with(':') || remaining.starts_with(' ') {
                // A colon after some spaces (`Since : e.32`) is the tag's
                // colon too; it is not the start of the value.
                let after_spaces = remaining.trim_start();
                let rest = if after_spaces.starts_with(':') {
                    after_spaces.trim_start_matches(':')
                } else {
                    remaining
                };
                return Some((tag.to_string(), rest.to_string()));
            }
        }
    }

    None
}

/// The non-blank lines, trimmed.
fn trimmed_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// A tag's lines as one value: a value wrapped over several lines is joined
/// with spaces.
fn joined_value(lines: &[String]) -> Option<String> {
    let value = trimmed_lines(lines).join(" ");
    (!value.is_empty()).then_some(value)
}

/// Code lines, with the indentation the block as a whole is written at
/// removed (and what is deeper kept) and blank lines inside kept. Blank
/// lines at either end are dropped.
fn code_lines(lines: &[String]) -> Vec<String> {
    // Counted in characters: the indentation may be any whitespace, and what
    // is cut off must be whole characters.
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);
    let mut out: Vec<String> = lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                let start = line
                    .char_indices()
                    .nth(indent)
                    .map_or(line.len(), |(i, _)| i);
                line[start..].trim_end().to_string()
            }
        })
        .collect();
    while out.first().is_some_and(|line| line.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|line| line.is_empty()) {
        out.pop();
    }
    out
}

/// List items: a line opening with `-` starts an item, and the lines after
/// it that do not are the rest of that item. A list written without any `-`
/// has one item per line.
fn list_items(lines: &[String]) -> Vec<String> {
    let lines = trimmed_lines(lines);
    let dashed = lines.iter().any(|line| line.starts_with('-'));
    let mut items: Vec<String> = Vec::new();
    for line in lines {
        match line.strip_prefix('-') {
            Some(item) => items.push(item.trim().to_string()),
            None => match items.last_mut() {
                Some(last) if dashed => {
                    last.push(' ');
                    last.push_str(&line);
                }
                _ => items.push(line),
            },
        }
    }
    items
}

/// Store the finished block of lines of `tag` in its field.
fn push_tag_block(tags: &mut DocTags, tag: &str, lines: &[String]) {
    match tag {
        "Purpose" => tags.purpose = joined_value(lines),
        "Returns" => tags.returns = joined_value(lines),
        "Since" => tags.since = joined_value(lines),
        "Deprecated" => tags.deprecated = joined_value(lines),
        "See" => tags.see = joined_value(lines),
        "AI-Category" => tags.ai_category = joined_value(lines),
        "Params" => {
            // Parse `- name: description` format
            for item in list_items(lines) {
                match item.find(':') {
                    Some(colon_pos) => {
                        let name = item[..colon_pos].trim().to_string();
                        let desc = item[colon_pos + 1..].trim().to_string();
                        tags.params.push((name, desc));
                    }
                    None => tags.params.push((item, String::new())),
                }
            }
        }
        "Throws" => tags.throws.extend(list_items(lines)),
        "Example" => tags.example.extend(code_lines(lines)),
        "AI-Examples" => tags.ai_examples.extend(code_lines(lines)),
        "AI-Context" => tags.ai_context.extend(trimmed_lines(lines)),
        "AI-Hint" => tags.ai_hint.extend(trimmed_lines(lines)),
        "AI-Constraints" => tags.ai_constraints.extend(trimmed_lines(lines)),
        "AI-Related" => tags.ai_related.extend(trimmed_lines(lines)),
        "AI-Complexity" => tags.ai_complexity.extend(trimmed_lines(lines)),
        "AI-SideEffects" => tags.ai_side_effects.extend(trimmed_lines(lines)),
        _ => {}
    }
}

// ── Type expression formatting ──────────────────────────────────────

/// Format a TypeExpr as a readable string.
pub fn format_type_expr(te: &TypeExpr) -> String {
    match te {
        TypeExpr::Named(name) => name.clone(),
        TypeExpr::BuchiPack(fields) => {
            let fs: Vec<String> = fields
                .iter()
                .map(|f| {
                    if let Some(ty) = &f.type_annotation {
                        format!("{}: {}", f.name, format_type_expr(ty))
                    } else {
                        f.name.clone()
                    }
                })
                .collect();
            format!("@({})", fs.join(", "))
        }
        TypeExpr::List(inner) => format!("@[{}]", format_type_expr(inner)),
        TypeExpr::Generic(name, params) => {
            let ps: Vec<String> = params.iter().map(format_type_expr).collect();
            format!("{}[{}]", name, ps.join(", "))
        }
        TypeExpr::Function(args, ret) => {
            let as_: Vec<String> = args.iter().map(format_type_expr).collect();
            let args_str = match as_.as_slice() {
                [] => "_".to_string(),
                [single] => single.clone(),
                _ => format!("({})", as_.join(", ")),
            };
            format!("{} => :{}", args_str, format_type_expr(ret))
        }
    }
}

fn format_type_param(tp: &TypeParam) -> String {
    match &tp.constraint {
        Some(constraint) => format!("{} <= :{}", tp.name, format_type_expr(constraint)),
        None => tp.name.clone(),
    }
}

fn format_mold_header_arg(arg: &MoldHeaderArg) -> String {
    match arg {
        MoldHeaderArg::TypeParam(tp) => format_type_param(tp),
        MoldHeaderArg::Concrete(ty) => format!(":{}", format_type_expr(ty)),
    }
}

// ── Extraction ──────────────────────────────────────────────────────

/// Extract field documentation from AST FieldDef.
///
/// The description is the field's `Purpose` (or, without one, what its
/// comment says) followed by the notes it carries (`Deprecated`, `Since`,
/// `See`), on one line for the table cell.
fn extract_field_doc(field: &FieldDef) -> FieldDoc {
    let type_str = field.type_annotation.as_ref().map(format_type_expr);
    let tags = parse_doc_tags(&field.doc_comments);
    let mut parts: Vec<String> = Vec::new();
    match &tags.purpose {
        Some(purpose) => parts.push(purpose.clone()),
        None if !tags.description.is_empty() => parts.push(tags.description.join(" ")),
        None => {}
    }
    for (label, note) in [
        ("Deprecated", &tags.deprecated),
        ("Since", &tags.since),
        ("See", &tags.see),
    ] {
        if let Some(note) = note {
            parts.push(format!("{}: {}", label, note));
        }
    }
    let doc = parts.join("; ");
    FieldDoc {
        name: field.name.clone(),
        type_str,
        doc,
    }
}

/// The value fields of a definition, for its table (methods are documented
/// on their own).
fn extract_field_docs(fields: &[FieldDef]) -> Vec<FieldDoc> {
    fields
        .iter()
        .filter(|field| field.method_def.is_none())
        .map(extract_field_doc)
        .collect()
}

/// The methods of a definition, documented like functions.
fn extract_method_docs(fields: &[FieldDef]) -> Vec<FuncDoc> {
    fields
        .iter()
        .filter_map(|field| {
            let method = field.method_def.as_ref()?;
            let mut doc = extract_func_doc(method);
            // The comment above the method is attached to its field.
            if doc.tags.is_empty() {
                doc.tags = parse_doc_tags(&field.doc_comments);
            }
            Some(doc)
        })
        .collect()
}

/// Extract documentation from a program AST.
pub fn extract_docs(program: &Program, module_name: &str) -> ModuleDoc {
    let mut doc = ModuleDoc {
        name: module_name.to_string(),
        types: Vec::new(),
        enums: Vec::new(),
        functions: Vec::new(),
        molds: Vec::new(),
        inheritances: Vec::new(),
        assignments: Vec::new(),
        exports: Vec::new(),
    };

    for stmt in &program.statements {
        match stmt {
            Statement::EnumDef(ed) => doc.enums.push(EnumDoc {
                name: ed.name.clone(),
                tags: parse_doc_tags(&ed.doc_comments),
                variants: ed.variants.iter().map(|v| v.name.clone()).collect(),
            }),
            // (E30 Sub-step 2.1) ClassLikeDef 単一 variant + kind dispatch。
            Statement::ClassLikeDef(cl) => match &cl.kind {
                ClassLikeKind::BuchiPack
                    if !cl.doc_comments.is_empty() || !cl.fields.is_empty() =>
                {
                    doc.types.push(extract_type_doc(cl));
                }
                ClassLikeKind::Mold { mold_args }
                    if !cl.doc_comments.is_empty()
                        || !cl.fields.is_empty()
                        || !mold_args.is_empty()
                        || cl.name_args.as_ref().is_some_and(|args| !args.is_empty()) =>
                {
                    doc.molds.push(extract_mold_doc(cl));
                }
                ClassLikeKind::Inheritance { .. }
                    if !cl.doc_comments.is_empty() || !cl.fields.is_empty() =>
                {
                    doc.inheritances.push(extract_inherit_doc(cl));
                }
                _ => {}
            },
            Statement::FuncDef(fd)
                if !fd.doc_comments.is_empty()
                    || !fd.params.is_empty()
                    || !fd.type_params.is_empty() =>
            {
                doc.functions.push(extract_func_doc(fd));
            }
            // Explicit addon bindings are surfaced as function docs.
            // `Name <= RustAddon["fn"](arity <= N)` binding を **public
            // function** として doc-gen 出力する。AST 上は Assignment だが、
            // public binding contract であり、call site から見ると関数。
            // doc comment の有無に関わらず必ず FuncDoc に出すことで、
            // `taida doc generate` の addon facade 出力に explicit bindings が
            // 全件表示される。Match の order が重要 — RustAddon
            // 判定を doc-comments-only の AssignmentDoc 分岐より前に置くこと。
            Statement::Assignment(a) if a.as_rust_addon_binding().is_some() => {
                doc.functions.push(extract_rust_addon_func_doc(a));
            }
            Statement::Assignment(a) if !a.doc_comments.is_empty() => {
                doc.assignments.push(extract_assignment_doc(a));
            }
            Statement::Export(es) => {
                doc.exports.extend(extract_export_symbols(es));
            }
            _ => {}
        }
    }

    doc
}

fn extract_assignment_doc(a: &Assignment) -> AssignmentDoc {
    AssignmentDoc {
        name: a.target.clone(),
        tags: parse_doc_tags(&a.doc_comments),
    }
}

/// Render an explicit
/// `Name <= RustAddon["fn"](arity <= N)` binding as a `FuncDoc` so
/// downstream doc-gen / LSP / introspection consumers surface it as a
/// public function. Params / return type are not declared in the
/// surface form (they live in the `///@` doc comment block above the
/// binding) — we mark return_type as `None` and produce N positional
/// placeholder params so the doc tool can render
/// `<name>(_, _, ...)` even before the user adds richer signatures.
fn extract_rust_addon_func_doc(a: &Assignment) -> FuncDoc {
    let (_fn_name, arity) = a
        .as_rust_addon_binding()
        .expect("caller already verified RustAddon binding");
    let tags = parse_doc_tags(&a.doc_comments);
    let params = rust_addon_doc_params(&tags, arity as usize);
    FuncDoc {
        name: a.target.clone(),
        type_params: Vec::new(),
        tags,
        params,
        return_type: None,
    }
}

fn rust_addon_doc_params(tags: &DocTags, arity: usize) -> Vec<(String, Option<String>)> {
    (0..arity)
        .map(|i| {
            let name = tags
                .params
                .get(i)
                .map(|(name, _)| name.trim())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("_arg{}", i));
            (name, None)
        })
        .collect()
}

fn extract_type_doc(td: &ClassLikeDef) -> TypeDoc {
    // (E30 Sub-step 2.1) BuchiPack kind の ClassLikeDef を旧 TypeDoc 形式に抽出。
    TypeDoc {
        name: td.name.clone(),
        tags: parse_doc_tags(&td.doc_comments),
        fields: extract_field_docs(&td.fields),
        methods: extract_method_docs(&td.fields),
    }
}

fn extract_func_doc(fd: &FuncDef) -> FuncDoc {
    let params: Vec<(String, Option<String>)> = fd
        .params
        .iter()
        .map(|p| {
            let type_str = p.type_annotation.as_ref().map(format_type_expr);
            (p.name.clone(), type_str)
        })
        .collect();

    let return_type = fd.return_type.as_ref().map(format_type_expr);

    FuncDoc {
        name: fd.name.clone(),
        type_params: fd.type_params.iter().map(format_type_param).collect(),
        tags: parse_doc_tags(&fd.doc_comments),
        params,
        return_type,
    }
}

fn extract_mold_doc(md: &ClassLikeDef) -> MoldDoc {
    // (E30 Sub-step 2.1) Mold kind の ClassLikeDef を旧 MoldDoc 形式に抽出。
    let mold_args = md.mold_args().cloned().unwrap_or_default();
    MoldDoc {
        name: md.name.clone(),
        header_args: md
            .name_args
            .as_ref()
            .unwrap_or(&mold_args)
            .iter()
            .map(format_mold_header_arg)
            .collect(),
        tags: parse_doc_tags(&md.doc_comments),
        fields: extract_field_docs(&md.fields),
        methods: extract_method_docs(&md.fields),
    }
}

fn extract_inherit_doc(id: &ClassLikeDef) -> InheritDoc {
    // (E30 Sub-step 2.1) Inheritance kind の ClassLikeDef を旧 InheritDoc 形式に抽出。
    let parent = id
        .parent()
        .expect("inheritance kind has parent")
        .to_string();
    let parent_args = id.parent_args();
    let parent_header_args: Vec<String> = parent_args
        .into_iter()
        .flatten()
        .map(format_mold_header_arg)
        .collect();
    let child_header_args: Vec<String> = id
        .name_args
        .as_ref()
        .or(parent_args)
        .into_iter()
        .flatten()
        .map(format_mold_header_arg)
        .collect();
    InheritDoc {
        parent,
        parent_header_args,
        child: id.name.clone(),
        child_header_args,
        tags: parse_doc_tags(&id.doc_comments),
        fields: extract_field_docs(&id.fields),
        methods: extract_method_docs(&id.fields),
    }
}

fn extract_export_symbols(es: &ExportStmt) -> Vec<String> {
    es.symbols.clone()
}

// ── Markdown rendering ──────────────────────────────────────────────

/// Render a ModuleDoc into Markdown.
pub fn render_markdown(doc: &ModuleDoc) -> String {
    let mut out = String::new();

    out.push_str(&format!("# Module: {}\n\n", doc.name));

    // Exports
    if !doc.exports.is_empty() {
        out.push_str("## Exports\n\n");
        for sym in &doc.exports {
            out.push_str(&format!("- `{}`\n", sym));
        }
        out.push('\n');
    }

    // Types
    if !doc.types.is_empty() {
        out.push_str("## Types\n\n");
        for td in &doc.types {
            render_type_doc(&mut out, td);
        }
    }

    // Enums
    if !doc.enums.is_empty() {
        out.push_str("## Enums\n\n");
        for ed in &doc.enums {
            render_enum_doc(&mut out, ed);
        }
    }

    // Functions
    if !doc.functions.is_empty() {
        out.push_str("## Functions\n\n");
        for fd in &doc.functions {
            render_func_doc(&mut out, fd);
        }
    }

    // Molds
    if !doc.molds.is_empty() {
        out.push_str("## Molds\n\n");
        for md in &doc.molds {
            render_mold_doc(&mut out, md);
        }
    }

    // Inheritances
    if !doc.inheritances.is_empty() {
        out.push_str("## Inheritances\n\n");
        for id in &doc.inheritances {
            render_inherit_doc(&mut out, id);
        }
    }

    // Assignments (documented bindings)
    if !doc.assignments.is_empty() {
        out.push_str("## Bindings\n\n");
        for ad in &doc.assignments {
            out.push_str(&format!("### {}\n\n", ad.name));
            render_tags_header(&mut out, &ad.tags);
            if let Some(ref ret) = ad.tags.returns {
                out.push_str(&format!("**Returns**: {}\n\n", ret));
            }
            render_tags_body(&mut out, &ad.tags);
        }
    }

    out
}

fn render_enum_doc(out: &mut String, ed: &EnumDoc) {
    out.push_str(&format!("### {}\n\n", ed.name));
    render_tags_header(out, &ed.tags);

    if !ed.variants.is_empty() {
        out.push_str("| Variant |\n");
        out.push_str("|---------|\n");
        for variant in &ed.variants {
            out.push_str(&format!("| `{}` |\n", escape_table_cell(variant)));
        }
        out.push('\n');
    }

    render_tags_body(out, &ed.tags);
}

/// Methods of a type, one `####` section each.
fn render_methods(out: &mut String, methods: &[FuncDoc]) {
    for method in methods {
        render_func_doc_at(out, method, "####");
    }
}

fn render_type_doc(out: &mut String, td: &TypeDoc) {
    out.push_str(&format!("### {}\n\n", td.name));
    render_tags_header(out, &td.tags);

    if !td.fields.is_empty() {
        out.push_str("| Field | Type | Description |\n");
        out.push_str("|-------|------|-------------|\n");
        for f in &td.fields {
            let type_str = f.type_str.as_deref().unwrap_or("-");
            let desc = if f.doc.is_empty() { "-" } else { &f.doc };
            out.push_str(&format!(
                "| `{}` | `{}` | {} |\n",
                escape_table_cell(&f.name),
                escape_table_cell(type_str),
                escape_table_cell(desc)
            ));
        }
        out.push('\n');
    }

    render_tags_body(out, &td.tags);
    render_methods(out, &td.methods);
}

fn render_func_doc(out: &mut String, fd: &FuncDoc) {
    render_func_doc_at(out, fd, "###");
}

fn render_func_doc_at(out: &mut String, fd: &FuncDoc, heading: &str) {
    let type_params_str = if fd.type_params.is_empty() {
        String::new()
    } else {
        format!("[{}]", fd.type_params.join(", "))
    };
    out.push_str(&format!("{} {}{}\n\n", heading, fd.name, type_params_str));
    render_tags_header(out, &fd.tags);

    // Parameters table (merge AST params with @Params tag descriptions)
    if !fd.params.is_empty() {
        out.push_str("| Parameter | Type | Description |\n");
        out.push_str("|-----------|------|-------------|\n");
        for (name, type_opt) in &fd.params {
            let type_str = type_opt.as_deref().unwrap_or("-");
            // Look up description from @Params tag
            let desc = fd
                .tags
                .params
                .iter()
                .find(|(pn, _)| pn == name)
                .map(|(_, d)| d.as_str())
                .unwrap_or("-");
            out.push_str(&format!(
                "| `{}` | `{}` | {} |\n",
                escape_table_cell(name),
                escape_table_cell(type_str),
                escape_table_cell(desc)
            ));
        }
        out.push('\n');
    }

    if let Some(ref ret) = fd.return_type {
        let ret_desc = fd.tags.returns.as_deref().unwrap_or("");
        if ret_desc.is_empty() {
            out.push_str(&format!("**Returns**: `{}`\n\n", ret));
        } else {
            out.push_str(&format!("**Returns**: `{}` - {}\n\n", ret, ret_desc));
        }
    } else if let Some(ref ret_desc) = fd.tags.returns {
        out.push_str(&format!("**Returns**: {}\n\n", ret_desc));
    }

    render_tags_body(out, &fd.tags);
}

fn render_mold_doc(out: &mut String, md: &MoldDoc) {
    let header_args_str = if md.header_args.is_empty() {
        String::new()
    } else {
        format!("[{}]", md.header_args.join(", "))
    };
    out.push_str(&format!("### {}{}\n\n", md.name, header_args_str));
    render_tags_header(out, &md.tags);

    if !md.fields.is_empty() {
        out.push_str("| Field | Type | Description |\n");
        out.push_str("|-------|------|-------------|\n");
        for f in &md.fields {
            let type_str = f.type_str.as_deref().unwrap_or("-");
            let desc = if f.doc.is_empty() { "-" } else { &f.doc };
            out.push_str(&format!(
                "| `{}` | `{}` | {} |\n",
                escape_table_cell(&f.name),
                escape_table_cell(type_str),
                escape_table_cell(desc)
            ));
        }
        out.push('\n');
    }

    render_tags_body(out, &md.tags);
    render_methods(out, &md.methods);
}

fn render_inherit_doc(out: &mut String, id: &InheritDoc) {
    let parent_header = if id.parent_header_args.is_empty() {
        id.parent.clone()
    } else {
        format!("{}[{}]", id.parent, id.parent_header_args.join(", "))
    };
    let child_header = if id.child_header_args.is_empty() {
        id.child.clone()
    } else {
        format!("{}[{}]", id.child, id.child_header_args.join(", "))
    };
    out.push_str(&format!("### {} => {}\n\n", parent_header, child_header));
    render_tags_header(out, &id.tags);

    if !id.fields.is_empty() {
        out.push_str("| Field | Type | Description |\n");
        out.push_str("|-------|------|-------------|\n");
        for f in &id.fields {
            let type_str = f.type_str.as_deref().unwrap_or("-");
            let desc = if f.doc.is_empty() { "-" } else { &f.doc };
            out.push_str(&format!(
                "| `{}` | `{}` | {} |\n",
                escape_table_cell(&f.name),
                escape_table_cell(type_str),
                escape_table_cell(desc)
            ));
        }
        out.push('\n');
    }

    render_tags_body(out, &id.tags);
    render_methods(out, &id.methods);
}

fn escape_table_cell(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "<br>")
        .replace("```", "`\\`\\`")
}

fn escape_fenced_code_line(s: &str) -> String {
    s.replace("```", "`\\`\\`")
}

/// Render the header section of tags: Purpose, Deprecated, Description.
fn render_tags_header(out: &mut String, tags: &DocTags) {
    if let Some(ref purpose) = tags.purpose {
        out.push_str(&format!("> {}\n\n", purpose));
    }

    if let Some(ref dep) = tags.deprecated {
        out.push_str(&format!("> **Deprecated**: {}\n\n", dep));
    }

    if !tags.description.is_empty() {
        for line in &tags.description {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
}

/// Render the body section of tags: Throws, Example, Since, See, and AI tags.
fn render_tags_body(out: &mut String, tags: &DocTags) {
    // Throws
    if !tags.throws.is_empty() {
        out.push_str("**Throws**:\n");
        for t in &tags.throws {
            out.push_str(&format!("- {}\n", t));
        }
        out.push('\n');
    }

    // Example
    if !tags.example.is_empty() {
        out.push_str("**Example**:\n\n```taida\n");
        for line in &tags.example {
            out.push_str(&escape_fenced_code_line(line));
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    // Since
    if let Some(ref since) = tags.since {
        out.push_str(&format!("**Since**: {}\n\n", since));
    }

    // See
    if let Some(ref see) = tags.see {
        out.push_str(&format!("**See**: {}\n\n", see));
    }

    // AI tags
    render_ai_tags(out, tags);
}

/// Render AI collaboration tags as dedicated sections.
fn render_ai_tags(out: &mut String, tags: &DocTags) {
    if let Some(ref cat) = tags.ai_category {
        out.push_str(&format!("**AI-Category**: {}\n\n", cat));
    }

    if !tags.ai_context.is_empty() {
        out.push_str("**AI-Context**:\n");
        for line in &tags.ai_context {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }

    if !tags.ai_hint.is_empty() {
        out.push_str("**AI-Hint**:\n");
        for line in &tags.ai_hint {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }

    if !tags.ai_examples.is_empty() {
        out.push_str("**AI-Examples**:\n\n```taida\n");
        for line in &tags.ai_examples {
            out.push_str(&escape_fenced_code_line(line));
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    if !tags.ai_constraints.is_empty() {
        out.push_str("**AI-Constraints**:\n");
        for line in &tags.ai_constraints {
            out.push_str(&format!("- {}\n", line.trim_start_matches('-').trim()));
        }
        out.push('\n');
    }

    if !tags.ai_related.is_empty() {
        out.push_str("**AI-Related**:\n");
        for line in &tags.ai_related {
            out.push_str(&format!("- {}\n", line.trim_start_matches('-').trim()));
        }
        out.push('\n');
    }

    if !tags.ai_complexity.is_empty() {
        out.push_str("**AI-Complexity**:\n");
        for line in &tags.ai_complexity {
            out.push_str(&format!("- {}\n", line.trim_start_matches('-').trim()));
        }
        out.push('\n');
    }

    if !tags.ai_side_effects.is_empty() {
        out.push_str("**AI-SideEffects**:\n");
        for line in &tags.ai_side_effects {
            out.push_str(&format!("- {}\n", line.trim_start_matches('-').trim()));
        }
        out.push('\n');
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_doc_tags_purpose() {
        let comments = vec!["@Purpose: Calculate the total score".to_string()];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.purpose, Some("Calculate the total score".to_string()));
    }

    #[test]
    fn test_parse_doc_tags_params() {
        let comments = vec![
            "@Params:".to_string(),
            "  - name: The user name".to_string(),
            "  - age: The user age".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.params.len(), 2);
        assert_eq!(
            tags.params[0],
            ("name".to_string(), "The user name".to_string())
        );
        assert_eq!(
            tags.params[1],
            ("age".to_string(), "The user age".to_string())
        );
    }

    #[test]
    fn test_parse_doc_tags_returns() {
        let comments = vec!["@Returns: The computed result".to_string()];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.returns, Some("The computed result".to_string()));
    }

    #[test]
    fn test_parse_doc_tags_throws() {
        let comments = vec![
            "@Throws:".to_string(),
            "  - ValidationError: when input is invalid".to_string(),
            "  - NetworkError: when connection fails".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.throws.len(), 2);
    }

    #[test]
    fn test_parse_doc_tags_example() {
        let comments = vec![
            "@Example:".to_string(),
            "  result <= compute(42)".to_string(),
            "  stdout(result)".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.example.len(), 2);
        assert!(tags.example[0].contains("compute(42)"));
    }

    #[test]
    fn test_parse_doc_tags_since() {
        let comments = vec!["@Since: 1.0.0".to_string()];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.since, Some("1.0.0".to_string()));
    }

    #[test]
    fn test_parse_doc_tags_deprecated() {
        let comments = vec!["@Deprecated: Use newFunc instead".to_string()];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.deprecated, Some("Use newFunc instead".to_string()));
    }

    #[test]
    fn test_parse_doc_tags_see() {
        let comments = vec!["@See: relatedFunc, otherFunc".to_string()];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.see, Some("relatedFunc, otherFunc".to_string()));
    }

    #[test]
    fn test_parse_doc_tags_ai_context() {
        let comments = vec![
            "@AI-Context:".to_string(),
            "  Use this in authentication flow.".to_string(),
            "  Requires valid session.".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.ai_context.len(), 2);
        assert!(tags.ai_context[0].contains("authentication"));
    }

    #[test]
    fn test_parse_doc_tags_ai_hint() {
        let comments = vec![
            "@AI-Context: Call from render loop.".to_string(),
            "@AI-Hint: Prefer renderFrame over manual cursor movement.".to_string(),
            "  Keep enter/leave calls paired.".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.ai_context, vec!["Call from render loop."]);
        assert_eq!(
            tags.ai_hint,
            vec![
                "Prefer renderFrame over manual cursor movement.",
                "Keep enter/leave calls paired.",
            ]
        );
    }

    #[test]
    fn test_parse_doc_tags_ai_category() {
        let comments = vec!["@AI-Category: utility, validation".to_string()];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.ai_category, Some("utility, validation".to_string()));
    }

    #[test]
    fn test_parse_doc_tags_ai_constraints() {
        let comments = vec![
            "@AI-Constraints:".to_string(),
            "  - Do not pass empty string".to_string(),
            "  - Limit must be positive".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.ai_constraints.len(), 2);
    }

    #[test]
    fn test_parse_doc_tags_ai_related() {
        let comments = vec![
            "@AI-Related:".to_string(),
            "  - createUser: creates a new user".to_string(),
            "  - deleteUser: deletes a user".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.ai_related.len(), 2);
    }

    #[test]
    fn test_parse_doc_tags_ai_complexity() {
        let comments = vec![
            "@AI-Complexity:".to_string(),
            "  - Time: O(n log n)".to_string(),
            "  - Space: O(n)".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.ai_complexity.len(), 2);
    }

    #[test]
    fn test_parse_doc_tags_ai_side_effects() {
        let comments = vec![
            "@AI-SideEffects:".to_string(),
            "  - Database: writes record".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.ai_side_effects.len(), 1);
    }

    #[test]
    fn test_parse_doc_tags_ai_examples() {
        let comments = vec![
            "@AI-Examples:".to_string(),
            "  result <= compute(1)".to_string(),
            "  result <= compute(2)".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.ai_examples.len(), 2);
    }

    #[test]
    fn test_parse_doc_tags_description() {
        let comments = vec![
            "This is a free-form description.".to_string(),
            "It spans multiple lines.".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.description.len(), 2);
        assert_eq!(tags.description[0], "This is a free-form description.");
    }

    #[test]
    fn test_parse_doc_tags_mixed() {
        let comments = vec![
            "@Purpose: Do something useful".to_string(),
            "".to_string(),
            "@Params:".to_string(),
            "  - x: first value".to_string(),
            "".to_string(),
            "@Returns: the result".to_string(),
            "@Since: 1.2.0".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.purpose, Some("Do something useful".to_string()));
        assert_eq!(tags.params.len(), 1);
        assert_eq!(tags.params[0], ("x".to_string(), "first value".to_string()));
        assert_eq!(tags.returns, Some("the result".to_string()));
        assert_eq!(tags.since, Some("1.2.0".to_string()));
    }

    #[test]
    fn test_parse_doc_tags_wrapped_values_keep_every_line() {
        let comments = vec![
            "Purpose: first line".to_string(),
            "  second line".to_string(),
            "Returns: the result,".to_string(),
            "  and more".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(tags.purpose.as_deref(), Some("first line second line"));
        assert_eq!(tags.returns.as_deref(), Some("the result, and more"));
    }

    #[test]
    fn test_parse_doc_tags_unknown_tag_line_does_not_replace_the_value() {
        let comments = vec![
            "Purpose: what it is".to_string(),
            "Note: something the tags do not know".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        let purpose = tags.purpose.expect("purpose");
        assert!(purpose.starts_with("what it is"), "{purpose}");
        assert!(purpose.contains("Note: something"), "{purpose}");
    }

    #[test]
    fn test_parse_doc_tags_throws_items_carry_no_dash() {
        let comments = vec![
            "Throws:".to_string(),
            "  - ValidationError: id is negative".to_string(),
            "  - NetworkError: cannot connect,".to_string(),
            "    retry later".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(
            tags.throws,
            vec![
                "ValidationError: id is negative".to_string(),
                "NetworkError: cannot connect, retry later".to_string(),
            ]
        );
        let mut out = String::new();
        render_tags_body(&mut out, &tags);
        assert!(out.contains("- ValidationError: id is negative\n"), "{out}");
        assert!(!out.contains("- - "), "{out}");
    }

    #[test]
    fn test_parse_doc_tags_example_keeps_indentation_and_blank_lines() {
        let comments = vec![
            "Example:".to_string(),
            "  f <= _ x: Int =".to_string(),
            "    y <= x * 2".to_string(),
            "    y + 1".to_string(),
            "".to_string(),
            "  stdout(f(1).toString())".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(
            tags.example,
            vec![
                "f <= _ x: Int =",
                "  y <= x * 2",
                "  y + 1",
                "",
                "stdout(f(1).toString())",
            ]
        );
    }

    #[test]
    fn test_param_description_continues_on_the_next_line() {
        let comments = vec![
            "Params:".to_string(),
            "  - id: the identifier".to_string(),
            "      of the pilot".to_string(),
            "  - limit: how many".to_string(),
        ];
        let tags = parse_doc_tags(&comments);
        assert_eq!(
            tags.params,
            vec![
                ("id".to_string(), "the identifier of the pilot".to_string()),
                ("limit".to_string(), "how many".to_string()),
            ]
        );
    }

    fn documented(source: &str) -> String {
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        render_markdown(&extract_docs(&program, "m.td"))
    }

    #[test]
    fn test_enum_is_documented_with_its_variants() {
        let md = documented("///@ Purpose: colours\nEnum => Color = :Red :Green\n");
        assert!(md.contains("## Enums"), "{md}");
        assert!(md.contains("### Color"), "{md}");
        assert!(md.contains("> colours"), "{md}");
        assert!(md.contains("| `Red` |"), "{md}");
        assert!(md.contains("| `Green` |"), "{md}");
    }

    #[test]
    fn test_method_comment_is_expanded_not_flattened_into_the_field_table() {
        let source = "Point = @(\n  x: Int\n  ///@ Purpose: add x\n  ///@ Params:\n  ///@   - other: the addend\n  ///@ Returns: the sum\n  plus other: Int =\n    x + other\n  => :Int\n)\n";
        let md = documented(source);
        assert!(md.contains("#### plus"), "{md}");
        assert!(md.contains("> add x"), "{md}");
        assert!(md.contains("| `other` | `Int` | the addend |"), "{md}");
        assert!(md.contains("**Returns**: `Int` - the sum"), "{md}");
        assert!(
            !md.contains("| `plus` |"),
            "a method is not a field row: {md}"
        );
    }

    #[test]
    fn test_format_type_expr_named() {
        let te = TypeExpr::Named("Int".to_string());
        assert_eq!(format_type_expr(&te), "Int");
    }

    #[test]
    fn test_format_type_expr_list() {
        let te = TypeExpr::List(Box::new(TypeExpr::Named("Str".to_string())));
        assert_eq!(format_type_expr(&te), "@[Str]");
    }

    #[test]
    fn test_format_type_expr_generic() {
        let te = TypeExpr::Generic(
            "Result".to_string(),
            vec![
                TypeExpr::Named("Int".to_string()),
                TypeExpr::Named("Error".to_string()),
            ],
        );
        assert_eq!(format_type_expr(&te), "Result[Int, Error]");
    }

    #[test]
    fn test_extract_docs_basic() {
        use crate::lexer::Span;
        let span = Span {
            start: 0,
            end: 0,
            line: 1,
            column: 1,
            node_id: 0,
        };

        let program = Program {
            statements: vec![
                Statement::ClassLikeDef(ClassLikeDef {
                    name: "User".to_string(),
                    fields: vec![FieldDef {
                        name: "name".to_string(),
                        type_annotation: Some(TypeExpr::Named("Str".to_string())),
                        default_value: None,
                        is_method: false,
                        method_def: None,
                        doc_comments: vec!["The user name".to_string()],
                        span: span.clone(),
                    }],
                    doc_comments: vec!["@Purpose: Represents a user".to_string()],
                    span: span.clone(),
                    kind: ClassLikeKind::BuchiPack,
                    name_args: None,
                    type_params: Vec::new(),
                }),
                Statement::FuncDef(FuncDef {
                    name: "greet".to_string(),
                    type_params: vec![],
                    params: vec![crate::parser::Param {
                        name: "name".to_string(),
                        type_annotation: Some(TypeExpr::Named("Str".to_string())),
                        default_value: None,
                        span: span.clone(),
                    }],
                    body: vec![],
                    return_type: Some(TypeExpr::Named("Str".to_string())),
                    doc_comments: vec![
                        "@Purpose: Greet the user".to_string(),
                        "@Returns: A greeting message".to_string(),
                    ],
                    span: span.clone(),
                }),
                Statement::Export(ExportStmt {
                    version: None,
                    symbols: vec!["User".to_string(), "greet".to_string()],
                    path: None,
                    span: span.clone(),
                }),
            ],
        };

        let doc = extract_docs(&program, "example.td");
        assert_eq!(doc.name, "example.td");
        assert_eq!(doc.types.len(), 1);
        assert_eq!(doc.types[0].name, "User");
        assert_eq!(
            doc.types[0].tags.purpose,
            Some("Represents a user".to_string())
        );
        assert_eq!(doc.types[0].fields.len(), 1);
        assert_eq!(doc.types[0].fields[0].name, "name");
        assert_eq!(doc.types[0].fields[0].doc, "The user name");

        assert_eq!(doc.functions.len(), 1);
        assert_eq!(doc.functions[0].name, "greet");
        assert_eq!(
            doc.functions[0].tags.purpose,
            Some("Greet the user".to_string())
        );
        assert_eq!(doc.functions[0].return_type, Some("Str".to_string()));

        assert_eq!(doc.exports, vec!["User".to_string(), "greet".to_string()]);
    }

    #[test]
    fn test_extract_rust_addon_docs_use_param_tags() {
        let source = r#"
///@ Purpose: Checks a terminal stream.
///@ Params:
///@   - stream: stream selector
///@ Returns: Bool
isTerminal <= RustAddon["isTerminal"](arity <= 1)
"#;
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "parse errors: {:?}", errors);

        let doc = extract_docs(&program, "terminal.td");
        assert_eq!(doc.functions.len(), 1);
        assert_eq!(doc.functions[0].name, "isTerminal");
        assert_eq!(doc.functions[0].params, vec![("stream".to_string(), None)]);

        let md = render_markdown(&doc);
        assert!(md.contains("| `stream` | `-` | stream selector |"));
        assert!(!md.contains("_arg0"));
    }

    #[test]
    fn test_render_markdown_types() {
        let doc = ModuleDoc {
            name: "test.td".to_string(),
            enums: vec![],
            types: vec![TypeDoc {
                name: "Pilot".to_string(),
                tags: DocTags {
                    purpose: Some("Represents a pilot".to_string()),
                    ..DocTags::default()
                },
                fields: vec![
                    FieldDoc {
                        name: "id".to_string(),
                        type_str: Some("Int".to_string()),
                        doc: "Unique identifier".to_string(),
                    },
                    FieldDoc {
                        name: "name".to_string(),
                        type_str: Some("Str".to_string()),
                        doc: "Pilot name".to_string(),
                    },
                ],
                methods: vec![],
            }],
            functions: vec![],
            molds: vec![],
            inheritances: vec![],
            assignments: vec![],
            exports: vec![],
        };

        let md = render_markdown(&doc);
        assert!(md.contains("# Module: test.td"));
        assert!(md.contains("### Pilot"));
        assert!(md.contains("> Represents a pilot"));
        assert!(md.contains("| `id` | `Int` | Unique identifier |"));
        assert!(md.contains("| `name` | `Str` | Pilot name |"));
    }

    #[test]
    fn test_render_markdown_functions() {
        let doc = ModuleDoc {
            name: "test.td".to_string(),
            enums: vec![],
            types: vec![],
            functions: vec![FuncDoc {
                name: "search".to_string(),
                type_params: vec!["T".to_string()],
                tags: DocTags {
                    purpose: Some("Search for items".to_string()),
                    params: vec![("query".to_string(), "Search query".to_string())],
                    returns: Some("List of results".to_string()),
                    ..DocTags::default()
                },
                params: vec![("query".to_string(), Some("Str".to_string()))],
                return_type: Some("@[Item]".to_string()),
            }],
            molds: vec![],
            inheritances: vec![],
            assignments: vec![],
            exports: vec![],
        };

        let md = render_markdown(&doc);
        assert!(md.contains("### search[T]"));
        assert!(md.contains("> Search for items"));
        assert!(md.contains("| `query` | `Str` | Search query |"));
        assert!(md.contains("**Returns**: `@[Item]` - List of results"));
    }

    #[test]
    fn test_render_markdown_molds() {
        let doc = ModuleDoc {
            name: "test.td".to_string(),
            enums: vec![],
            types: vec![],
            functions: vec![],
            molds: vec![MoldDoc {
                name: "ApiResult".to_string(),
                header_args: vec!["T".to_string(), "P <= :T => :Bool".to_string()],
                tags: DocTags {
                    purpose: Some("Wraps API response".to_string()),
                    ..DocTags::default()
                },
                fields: vec![FieldDoc {
                    name: "success".to_string(),
                    type_str: Some("Bool".to_string()),
                    doc: "Whether request succeeded".to_string(),
                }],
                methods: vec![],
            }],
            inheritances: vec![],
            assignments: vec![],
            exports: vec![],
        };

        let md = render_markdown(&doc);
        assert!(md.contains("### ApiResult[T, P <= :T => :Bool]"));
        assert!(md.contains("> Wraps API response"));
        assert!(md.contains("| `success` | `Bool` | Whether request succeeded |"));
    }

    #[test]
    fn test_render_markdown_inheritances() {
        let doc = ModuleDoc {
            name: "test.td".to_string(),
            enums: vec![],
            types: vec![],
            functions: vec![],
            molds: vec![],
            inheritances: vec![InheritDoc {
                parent: "Base".to_string(),
                parent_header_args: vec!["T".to_string()],
                child: "Derived".to_string(),
                child_header_args: vec!["T".to_string(), "U <= :T => :Bool".to_string()],
                tags: DocTags {
                    purpose: Some("Derived extends Base with a predicate slot".to_string()),
                    ..DocTags::default()
                },
                fields: vec![FieldDoc {
                    name: "breed".to_string(),
                    type_str: Some("Str".to_string()),
                    doc: "Dog breed".to_string(),
                }],
                methods: vec![],
            }],
            assignments: vec![],
            exports: vec![],
        };

        let md = render_markdown(&doc);
        assert!(md.contains("### Base[T] => Derived[T, U <= :T => :Bool]"));
        assert!(md.contains("> Derived extends Base with a predicate slot"));
        assert!(md.contains("| `breed` | `Str` | Dog breed |"));
    }

    #[test]
    fn test_render_markdown_exports() {
        let doc = ModuleDoc {
            name: "test.td".to_string(),
            enums: vec![],
            types: vec![],
            functions: vec![],
            molds: vec![],
            inheritances: vec![],
            assignments: vec![],
            exports: vec!["Pilot".to_string(), "createPilot".to_string()],
        };

        let md = render_markdown(&doc);
        assert!(md.contains("## Exports"));
        assert!(md.contains("- `Pilot`"));
        assert!(md.contains("- `createPilot`"));
    }

    #[test]
    fn test_render_markdown_ai_tags() {
        let doc = ModuleDoc {
            name: "test.td".to_string(),
            enums: vec![],
            types: vec![],
            functions: vec![FuncDoc {
                name: "process".to_string(),
                type_params: vec![],
                tags: DocTags {
                    purpose: Some("Process data".to_string()),
                    ai_category: Some("data-processing".to_string()),
                    ai_context: vec!["Use in batch processing pipeline.".to_string()],
                    ai_hint: vec!["Prefer batch size under 100.".to_string()],
                    ai_constraints: vec!["- Input must not be empty".to_string()],
                    ai_related: vec!["- validate: input validation".to_string()],
                    ai_complexity: vec!["- Time: O(n)".to_string()],
                    ai_side_effects: vec!["- Database: writes log".to_string()],
                    ..DocTags::default()
                },
                params: vec![],
                return_type: None,
            }],
            molds: vec![],
            inheritances: vec![],
            assignments: vec![],
            exports: vec![],
        };

        let md = render_markdown(&doc);
        assert!(md.contains("**AI-Category**: data-processing"));
        assert!(md.contains("**AI-Context**:"));
        assert!(md.contains("Use in batch processing pipeline."));
        assert!(md.contains("**AI-Hint**:"));
        assert!(md.contains("Prefer batch size under 100."));
        assert!(md.contains("**AI-Constraints**:"));
        assert!(md.contains("- Input must not be empty"));
        assert!(md.contains("**AI-Related**:"));
        assert!(md.contains("- validate: input validation"));
        assert!(md.contains("**AI-Complexity**:"));
        assert!(md.contains("- Time: O(n)"));
        assert!(md.contains("**AI-SideEffects**:"));
        assert!(md.contains("- Database: writes log"));
    }

    #[test]
    fn test_extract_docs_mold() {
        use crate::lexer::Span;
        let span = Span {
            start: 0,
            end: 0,
            line: 1,
            column: 1,
            node_id: 0,
        };

        let program = Program {
            statements: vec![Statement::ClassLikeDef(ClassLikeDef {
                name: "Container".to_string(),
                fields: vec![FieldDef {
                    name: "value".to_string(),
                    type_annotation: None,
                    default_value: None,
                    is_method: false,
                    method_def: None,
                    doc_comments: vec!["The contained value".to_string()],
                    span: span.clone(),
                }],
                doc_comments: vec!["@Purpose: A generic container".to_string()],
                span: span.clone(),
                kind: ClassLikeKind::Mold {
                    mold_args: vec![crate::parser::MoldHeaderArg::TypeParam(
                        crate::parser::TypeParam {
                            name: "T".to_string(),
                            constraint: None,
                        },
                    )],
                },
                name_args: None,
                type_params: vec![crate::parser::TypeParam {
                    name: "T".to_string(),
                    constraint: None,
                }],
            })],
        };

        let doc = extract_docs(&program, "container.td");
        assert_eq!(doc.molds.len(), 1);
        assert_eq!(doc.molds[0].name, "Container");
        assert_eq!(doc.molds[0].header_args, vec!["T".to_string()]);
        assert_eq!(
            doc.molds[0].tags.purpose,
            Some("A generic container".to_string())
        );
    }

    #[test]
    fn test_extract_docs_keeps_generic_function_and_mold_headers() {
        use crate::lexer::Span;
        let span = Span {
            start: 0,
            end: 0,
            line: 1,
            column: 1,
            node_id: 0,
        };

        let program = Program {
            statements: vec![
                Statement::FuncDef(FuncDef {
                    name: "id".to_string(),
                    type_params: vec![crate::parser::TypeParam {
                        name: "T".to_string(),
                        constraint: Some(TypeExpr::Named("Num".to_string())),
                    }],
                    params: vec![crate::parser::Param {
                        name: "value".to_string(),
                        type_annotation: Some(TypeExpr::Named("T".to_string())),
                        default_value: None,
                        span: span.clone(),
                    }],
                    body: vec![],
                    return_type: Some(TypeExpr::Named("T".to_string())),
                    doc_comments: vec![],
                    span: span.clone(),
                }),
                Statement::ClassLikeDef(ClassLikeDef {
                    name: "IntBox".to_string(),
                    fields: vec![FieldDef {
                        name: "count".to_string(),
                        type_annotation: Some(TypeExpr::Named("Int".to_string())),
                        default_value: None,
                        is_method: false,
                        method_def: None,
                        doc_comments: vec![],
                        span: span.clone(),
                    }],
                    doc_comments: vec!["@Purpose: int wrapper".to_string()],
                    span,
                    kind: ClassLikeKind::Mold {
                        mold_args: vec![
                            crate::parser::MoldHeaderArg::Concrete(TypeExpr::Named(
                                "Int".to_string(),
                            )),
                            crate::parser::MoldHeaderArg::TypeParam(crate::parser::TypeParam {
                                name: "T".to_string(),
                                constraint: Some(TypeExpr::Named("Int".to_string())),
                            }),
                        ],
                    },
                    name_args: None,
                    type_params: vec![crate::parser::TypeParam {
                        name: "T".to_string(),
                        constraint: Some(TypeExpr::Named("Int".to_string())),
                    }],
                }),
            ],
        };

        let doc = extract_docs(&program, "headers.td");
        assert_eq!(doc.functions[0].type_params, vec!["T <= :Num".to_string()]);
        assert_eq!(
            doc.molds[0].header_args,
            vec![":Int".to_string(), "T <= :Int".to_string()]
        );
    }

    #[test]
    fn test_extract_docs_keeps_header_only_declarations() {
        use crate::lexer::Span;
        let span = Span {
            start: 0,
            end: 0,
            line: 1,
            column: 1,
            node_id: 0,
        };

        let program = Program {
            statements: vec![
                Statement::FuncDef(FuncDef {
                    name: "make".to_string(),
                    type_params: vec![crate::parser::TypeParam {
                        name: "T".to_string(),
                        constraint: None,
                    }],
                    params: vec![],
                    body: vec![],
                    return_type: Some(TypeExpr::Named("T".to_string())),
                    doc_comments: vec![],
                    span: span.clone(),
                }),
                Statement::ClassLikeDef(ClassLikeDef {
                    name: "Box".to_string(),
                    fields: vec![],
                    doc_comments: vec![],
                    span,
                    kind: ClassLikeKind::Mold {
                        mold_args: vec![crate::parser::MoldHeaderArg::Concrete(TypeExpr::Named(
                            "Int".to_string(),
                        ))],
                    },
                    name_args: None,
                    type_params: vec![],
                }),
            ],
        };

        let doc = extract_docs(&program, "header-only.td");
        assert_eq!(doc.functions.len(), 1);
        assert_eq!(doc.functions[0].name, "make");
        assert_eq!(doc.functions[0].type_params, vec!["T".to_string()]);
        assert_eq!(doc.molds.len(), 1);
        assert_eq!(doc.molds[0].name, "Box");
        assert_eq!(doc.molds[0].header_args, vec![":Int".to_string()]);
    }

    #[test]
    fn test_extract_docs_inheritance() {
        use crate::lexer::Span;
        let span = Span {
            start: 0,
            end: 0,
            line: 1,
            column: 1,
            node_id: 0,
        };

        let program = Program {
            statements: vec![Statement::ClassLikeDef(ClassLikeDef {
                name: "Derived".to_string(),
                fields: vec![FieldDef {
                    name: "extra".to_string(),
                    type_annotation: Some(TypeExpr::Named("Int".to_string())),
                    default_value: None,
                    is_method: false,
                    method_def: None,
                    doc_comments: vec!["Extra field".to_string()],
                    span: span.clone(),
                }],
                doc_comments: vec!["Purpose: Derived extends Base".to_string()],
                span: span.clone(),
                kind: ClassLikeKind::Inheritance {
                    parent: "Base".to_string(),
                    parent_args: Some(vec![crate::parser::MoldHeaderArg::TypeParam(
                        crate::parser::TypeParam {
                            name: "T".to_string(),
                            constraint: None,
                        },
                    )]),
                },
                name_args: Some(vec![
                    crate::parser::MoldHeaderArg::TypeParam(crate::parser::TypeParam {
                        name: "T".to_string(),
                        constraint: None,
                    }),
                    crate::parser::MoldHeaderArg::TypeParam(crate::parser::TypeParam {
                        name: "U".to_string(),
                        constraint: Some(TypeExpr::Function(
                            vec![TypeExpr::Named("T".to_string())],
                            Box::new(TypeExpr::Named("Bool".to_string())),
                        )),
                    }),
                ]),
                type_params: vec![],
            })],
        };

        let doc = extract_docs(&program, "inherit.td");
        assert_eq!(doc.inheritances.len(), 1);
        assert_eq!(doc.inheritances[0].parent, "Base");
        assert_eq!(doc.inheritances[0].child, "Derived");
        assert_eq!(
            doc.inheritances[0].parent_header_args,
            vec!["T".to_string()]
        );
        assert_eq!(
            doc.inheritances[0].child_header_args,
            vec!["T".to_string(), "U <= :T => :Bool".to_string()]
        );
    }
}

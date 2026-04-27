//! Documentation generator for Taida Lang.
//!
//! Extracts doc comments (`///@`) from AST nodes and generates Markdown documentation.

use crate::parser::{
    Assignment, ExportStmt, FieldDef, FuncDef, InheritanceDef, MoldDef, MoldHeaderArg, Program,
    Statement, TypeDef, TypeExpr, TypeParam,
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
pub fn parse_doc_tags(comments: &[String]) -> DocTags {
    let mut tags = DocTags::default();
    let mut current_tag: Option<String> = None;

    for line in comments {
        let trimmed = line.trim();

        // Check if this line starts a new tag: `@TagName:` or `@TagName`
        if let Some(tag_match) = try_match_tag(trimmed) {
            let (tag_name, rest) = tag_match;
            current_tag = Some(tag_name.to_string());

            let rest_trimmed = rest.trim();
            if !rest_trimmed.is_empty() {
                push_tag_content(&mut tags, &tag_name, rest_trimmed);
            }
        } else if let Some(ref tag) = current_tag {
            // Continuation line for the current tag
            if trimmed.is_empty() {
                // Blank line within a multi-line tag: keep it if relevant
                // (skip for now to avoid trailing blanks)
            } else {
                push_tag_content(&mut tags, tag, trimmed);
            }
        } else {
            // Free-form description (no tag context yet)
            if !trimmed.is_empty() {
                tags.description.push(trimmed.to_string());
            }
        }
    }

    tags
}

/// Try to match a tag at the start of a line. Returns (tag_name, rest_of_line).
///
/// Accepts both `@Purpose: ...` (raw doc comment content where `///@` prefix
/// left the `@`) and `Purpose: ...` (where the lexer already consumed the `@`
/// as part of the `///@` prefix).
fn try_match_tag(line: &str) -> Option<(String, String)> {
    // Strip optional leading `@`
    let body = line.strip_prefix('@').unwrap_or(line);

    // Try each known tag (case-sensitive)
    for &tag in SINGLE_LINE_TAGS.iter().chain(MULTI_LINE_TAGS.iter()) {
        if let Some(remaining) = body.strip_prefix(tag) {
            // Must be followed by `:`, whitespace, or end of line
            if remaining.is_empty() || remaining.starts_with(':') || remaining.starts_with(' ') {
                let rest = remaining.trim_start_matches(':').to_string();
                return Some((tag.to_string(), rest));
            }
        }
    }

    None
}

/// Push content into the appropriate tag field.
fn push_tag_content(tags: &mut DocTags, tag: &str, content: &str) {
    match tag {
        "Purpose" => {
            tags.purpose = Some(content.to_string());
        }
        "Returns" => {
            tags.returns = Some(content.to_string());
        }
        "Since" => {
            tags.since = Some(content.to_string());
        }
        "Deprecated" => {
            tags.deprecated = Some(content.to_string());
        }
        "See" => {
            tags.see = Some(content.to_string());
        }
        "AI-Category" => {
            tags.ai_category = Some(content.to_string());
        }
        "Params" => {
            // Parse `- name: description` format
            let stripped = content.trim_start_matches('-').trim();
            if let Some(colon_pos) = stripped.find(':') {
                let name = stripped[..colon_pos].trim().to_string();
                let desc = stripped[colon_pos + 1..].trim().to_string();
                tags.params.push((name, desc));
            } else {
                tags.params.push((stripped.to_string(), String::new()));
            }
        }
        "Throws" => {
            tags.throws.push(content.to_string());
        }
        "Example" => {
            tags.example.push(content.to_string());
        }
        "AI-Context" => {
            tags.ai_context.push(content.to_string());
        }
        "AI-Hint" => {
            tags.ai_hint.push(content.to_string());
        }
        "AI-Examples" => {
            tags.ai_examples.push(content.to_string());
        }
        "AI-Constraints" => {
            tags.ai_constraints.push(content.to_string());
        }
        "AI-Related" => {
            tags.ai_related.push(content.to_string());
        }
        "AI-Complexity" => {
            tags.ai_complexity.push(content.to_string());
        }
        "AI-SideEffects" => {
            tags.ai_side_effects.push(content.to_string());
        }
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
            let args_str = if as_.len() == 1 {
                as_[0].clone()
            } else {
                format!("({})", as_.join(", "))
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
fn extract_field_doc(field: &FieldDef) -> FieldDoc {
    let type_str = field.type_annotation.as_ref().map(format_type_expr);
    let doc = field.doc_comments.join(" ").trim().to_string();
    FieldDoc {
        name: field.name.clone(),
        type_str,
        doc,
    }
}

/// Extract documentation from a program AST.
pub fn extract_docs(program: &Program, module_name: &str) -> ModuleDoc {
    let mut doc = ModuleDoc {
        name: module_name.to_string(),
        types: Vec::new(),
        functions: Vec::new(),
        molds: Vec::new(),
        inheritances: Vec::new(),
        assignments: Vec::new(),
        exports: Vec::new(),
    };

    for stmt in &program.statements {
        match stmt {
            Statement::TypeDef(td) if !td.doc_comments.is_empty() || !td.fields.is_empty() => {
                doc.types.push(extract_type_doc(td));
            }
            Statement::FuncDef(fd)
                if !fd.doc_comments.is_empty()
                    || !fd.params.is_empty()
                    || !fd.type_params.is_empty() =>
            {
                doc.functions.push(extract_func_doc(fd));
            }
            Statement::MoldDef(md)
                if !md.doc_comments.is_empty()
                    || !md.fields.is_empty()
                    || !md.mold_args.is_empty()
                    || md.name_args.as_ref().is_some_and(|args| !args.is_empty()) =>
            {
                doc.molds.push(extract_mold_doc(md));
            }
            Statement::InheritanceDef(id)
                if !id.doc_comments.is_empty() || !id.fields.is_empty() =>
            {
                doc.inheritances.push(extract_inherit_doc(id));
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

fn extract_type_doc(td: &TypeDef) -> TypeDoc {
    TypeDoc {
        name: td.name.clone(),
        tags: parse_doc_tags(&td.doc_comments),
        fields: td.fields.iter().map(extract_field_doc).collect(),
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

fn extract_mold_doc(md: &MoldDef) -> MoldDoc {
    MoldDoc {
        name: md.name.clone(),
        header_args: md
            .name_args
            .as_ref()
            .unwrap_or(&md.mold_args)
            .iter()
            .map(format_mold_header_arg)
            .collect(),
        tags: parse_doc_tags(&md.doc_comments),
        fields: md.fields.iter().map(extract_field_doc).collect(),
    }
}

fn extract_inherit_doc(id: &InheritanceDef) -> InheritDoc {
    let parent_header_args: Vec<String> = id
        .parent_args
        .as_ref()
        .into_iter()
        .flatten()
        .map(format_mold_header_arg)
        .collect();
    let child_header_args: Vec<String> = id
        .child_args
        .as_ref()
        .or(id.parent_args.as_ref())
        .into_iter()
        .flatten()
        .map(format_mold_header_arg)
        .collect();
    InheritDoc {
        parent: id.parent.clone(),
        parent_header_args,
        child: id.child.clone(),
        child_header_args,
        tags: parse_doc_tags(&id.doc_comments),
        fields: id.fields.iter().map(extract_field_doc).collect(),
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

fn render_type_doc(out: &mut String, td: &TypeDoc) {
    out.push_str(&format!("### {}\n\n", td.name));
    render_tags_header(out, &td.tags);

    if !td.fields.is_empty() {
        out.push_str("| Field | Type | Description |\n");
        out.push_str("|-------|------|-------------|\n");
        for f in &td.fields {
            let type_str = f.type_str.as_deref().unwrap_or("-");
            let desc = if f.doc.is_empty() { "-" } else { &f.doc };
            out.push_str(&format!("| `{}` | `{}` | {} |\n", f.name, type_str, desc));
        }
        out.push('\n');
    }

    render_tags_body(out, &td.tags);
}

fn render_func_doc(out: &mut String, fd: &FuncDoc) {
    let type_params_str = if fd.type_params.is_empty() {
        String::new()
    } else {
        format!("[{}]", fd.type_params.join(", "))
    };
    out.push_str(&format!("### {}{}\n\n", fd.name, type_params_str));
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
            out.push_str(&format!("| `{}` | `{}` | {} |\n", name, type_str, desc));
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
            out.push_str(&format!("| `{}` | `{}` | {} |\n", f.name, type_str, desc));
        }
        out.push('\n');
    }

    render_tags_body(out, &md.tags);
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
            out.push_str(&format!("| `{}` | `{}` | {} |\n", f.name, type_str, desc));
        }
        out.push('\n');
    }

    render_tags_body(out, &id.tags);
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
            out.push_str(line);
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
            out.push_str(line);
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
        };

        let program = Program {
            statements: vec![
                Statement::TypeDef(TypeDef {
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
    fn test_render_markdown_types() {
        let doc = ModuleDoc {
            name: "test.td".to_string(),
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
        };

        let program = Program {
            statements: vec![Statement::MoldDef(MoldDef {
                name: "Container".to_string(),
                mold_args: vec![crate::parser::MoldHeaderArg::TypeParam(
                    crate::parser::TypeParam {
                        name: "T".to_string(),
                        constraint: None,
                    },
                )],
                name_args: None,
                type_params: vec![crate::parser::TypeParam {
                    name: "T".to_string(),
                    constraint: None,
                }],
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
                Statement::MoldDef(MoldDef {
                    name: "IntBox".to_string(),
                    mold_args: vec![
                        crate::parser::MoldHeaderArg::Concrete(TypeExpr::Named("Int".to_string())),
                        crate::parser::MoldHeaderArg::TypeParam(crate::parser::TypeParam {
                            name: "T".to_string(),
                            constraint: Some(TypeExpr::Named("Int".to_string())),
                        }),
                    ],
                    name_args: None,
                    type_params: vec![crate::parser::TypeParam {
                        name: "T".to_string(),
                        constraint: Some(TypeExpr::Named("Int".to_string())),
                    }],
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
                Statement::MoldDef(MoldDef {
                    name: "Box".to_string(),
                    mold_args: vec![crate::parser::MoldHeaderArg::Concrete(TypeExpr::Named(
                        "Int".to_string(),
                    ))],
                    name_args: None,
                    type_params: vec![],
                    fields: vec![],
                    doc_comments: vec![],
                    span,
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
        };

        let program = Program {
            statements: vec![Statement::InheritanceDef(InheritanceDef {
                parent: "Base".to_string(),
                parent_args: Some(vec![crate::parser::MoldHeaderArg::TypeParam(
                    crate::parser::TypeParam {
                        name: "T".to_string(),
                        constraint: None,
                    },
                )]),
                child: "Derived".to_string(),
                child_args: Some(vec![
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

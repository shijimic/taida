/// Hover information provider for Taida Lang LSP.
///
/// Provides type information and documentation when hovering over:
/// - Variables: type information
/// - Functions: signature (parameters + return type) + doc_comments
/// - Types: field list + doc_comments
/// - Molds: type parameters + doc_comments
/// - Expressions: inferred type
use tower_lsp::lsp_types::Position;

use crate::parser::{Expr, Statement, parse};
use crate::types::TypeChecker;

use super::format::{
    format_mold_header_arg, format_named_mold_header, format_registered_fields, format_type_expr,
};

fn format_mold_hover_block(
    signature: &str,
    fields: &[(String, crate::types::Type)],
    doc: &str,
) -> String {
    if fields.is_empty() {
        format!("```taida\n{} = @()\n```{}", signature, doc)
    } else {
        format!(
            "```taida\n{} = @(\n{}\n)\n```{}",
            signature,
            format_registered_fields(fields),
            doc
        )
    }
}

fn find_user_mold_hover_info(
    statements: &[Statement],
    name: &str,
    checker: &TypeChecker,
) -> Option<String> {
    if !checker.registry.mold_defs.contains_key(name) {
        return None;
    }
    let fields = checker.registry.get_type_fields(name).unwrap_or_default();
    for stmt in statements {
        // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch
        if let Statement::ClassLikeDef(cl) = stmt {
            if cl.name != name {
                continue;
            }
            match &cl.kind {
                crate::parser::ClassLikeKind::Mold { mold_args } => {
                    let child_args = cl.name_args.as_deref().unwrap_or(mold_args.as_slice());
                    let signature = format!(
                        "{} => {}",
                        format_named_mold_header("Mold", mold_args),
                        format_named_mold_header(&cl.name, child_args)
                    );
                    let doc = format_doc_comments(&cl.doc_comments);
                    return Some(format_mold_hover_block(&signature, &fields, &doc));
                }
                crate::parser::ClassLikeKind::Inheritance {
                    parent,
                    parent_args,
                } => {
                    let parent_header = match parent_args.as_deref() {
                        Some(args) => format_named_mold_header(parent, args),
                        None => parent.clone(),
                    };
                    let child_args = cl
                        .name_args
                        .as_deref()
                        .or(parent_args.as_deref())
                        .unwrap_or(&[]);
                    let signature = format!(
                        "{} => {}",
                        parent_header,
                        format_named_mold_header(&cl.name, child_args)
                    );
                    let doc = format_doc_comments(&cl.doc_comments);
                    return Some(format_mold_hover_block(&signature, &fields, &doc));
                }
                crate::parser::ClassLikeKind::Alias { target } => {
                    let doc = format_doc_comments(&cl.doc_comments);
                    return Some(format!(
                        "```taida\n{} = {}\n```{}",
                        cl.name,
                        format_type_expr(target),
                        doc
                    ));
                }
                crate::parser::ClassLikeKind::BuchiPack => {}
            }
        }
    }
    None
}

/// Format doc_comments into a markdown string.
fn format_doc_comments(doc_comments: &[String]) -> String {
    if doc_comments.is_empty() {
        return String::new();
    }
    format!("\n\n---\n\n{}", doc_comments.join("\n"))
}

/// Get hover information for a position in the source.
/// Returns a markdown string with type info, or None if no info available.
pub fn get_hover_info(source: &str, position: Position) -> Option<String> {
    get_hover_info_for_file(source, position, None)
}

/// [`get_hover_info`] for a document stored at `path`, so the names it
/// imports from other modules are typed as `taida way check` types them.
pub fn get_hover_info_for_file(
    source: &str,
    position: Position,
    path: Option<&std::path::Path>,
) -> Option<String> {
    let (program, parse_errors) = parse(source);
    if !parse_errors.is_empty() {
        return None;
    }

    // The server answers on its main thread, whose stack is the process
    // default, so a deeply nested program is checked, walked and dropped on a
    // stack sized for it.
    crate::graph::walk::with_stack_for_owned(program, |program| {
        // Run type checker to populate scope info
        let mut checker = TypeChecker::new();
        checker.set_compile_target(crate::types::CompileTarget::Interpreter);
        if let Some(path) = path {
            checker.set_source_file(path);
        }
        checker.check_program(program);

        // Convert LSP 0-based UTF-16 offset to 0-based char index, then to
        // 1-based column to match Span's convention.
        let target_line = position.line as usize + 1; // Span uses 1-based
        let line_text = source.lines().nth(position.line as usize).unwrap_or("");
        let mut char_index =
            super::utf16::utf16_offset_to_char_index(line_text, position.character as usize);
        // The lexer numbers columns from the first character after a byte order
        // mark, which the editor counts as the first character of the line.
        if position.line == 0 && source.starts_with('\u{feff}') {
            char_index = char_index.saturating_sub(1);
        }
        let target_col = char_index + 1;

        // Walk the AST to find the identifier at this position
        for stmt in &program.statements {
            if let Some(info) = find_hover_in_statement(
                stmt,
                target_line,
                target_col,
                &checker,
                &program.statements,
            ) {
                return Some(info);
            }
        }

        None
    })
}

/// Search a statement for hover-relevant nodes at the given position.
fn find_hover_in_statement(
    stmt: &Statement,
    line: usize,
    col: usize,
    checker: &TypeChecker,
    all_stmts: &[Statement],
) -> Option<String> {
    match stmt {
        Statement::Assignment(assign) => {
            // Check if cursor is on the value expression
            if let Some(info) = find_hover_in_expr(&assign.value, line, col, checker, all_stmts) {
                return Some(info);
            }
            // The statement starts at the target only in the `name <= expr`
            // form; in `expr => name` the position of the target is not kept.
            if assign.span.line == line && !assign.forward {
                // Check if cursor is near the target name
                if assign.span.column <= col
                    && col <= assign.span.column + assign.target.chars().count()
                {
                    if let Some((fn_name, arity)) = assign.as_rust_addon_binding() {
                        let params = (0..arity)
                            .map(|i| format!("_arg{}: Unknown", i))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let spacer = if params.is_empty() { "" } else { " " };
                        return Some(format!(
                            "```taida\n{}{}{} => :Unknown\n```\nRustAddon[\"{}\"](arity <= {})",
                            assign.target, spacer, params, fn_name, arity
                        ));
                    }
                    // A binding inside a function is not in the top-level scope
                    // the checker keeps, so fall back to the declared type or
                    // the type recorded for the bound value.
                    let ty = assign
                        .type_annotation
                        .as_ref()
                        .map(format_type_expr)
                        .or_else(|| recorded_type(&assign.value, checker).map(|ty| ty.to_string()))
                        .or_else(|| checker.lookup_var(&assign.target).map(|ty| ty.to_string()));
                    if let Some(ty) = ty {
                        return Some(format!("```taida\n{}: {}\n```", assign.target, ty));
                    }
                }
            }
            None
        }
        Statement::FuncDef(fd) => {
            // Check if cursor is on the function name
            if fd.span.line == line {
                let ret_type = fd
                    .return_type
                    .as_ref()
                    .map(format_type_expr)
                    .unwrap_or_else(|| "Unknown".to_string());
                let params: Vec<String> = fd
                    .params
                    .iter()
                    .map(|p| {
                        if let Some(ann) = &p.type_annotation {
                            format!("{}: {}", p.name, format_type_expr(ann))
                        } else {
                            p.name.clone()
                        }
                    })
                    .collect();
                if fd.span.column <= col && col <= fd.span.column + fd.name.chars().count() {
                    let doc = format_doc_comments(&fd.doc_comments);
                    return Some(format!(
                        "```taida\n{} {} => :{}\n```{}",
                        fd.name,
                        params.join(" "),
                        ret_type,
                        doc
                    ));
                }
            }
            // Search function body
            for body_stmt in &fd.body {
                if let Some(info) =
                    find_hover_in_statement(body_stmt, line, col, checker, all_stmts)
                {
                    return Some(info);
                }
            }
            None
        }
        // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch (旧 TypeDef/MoldDef/InheritanceDef を統合)
        Statement::ClassLikeDef(cl) => {
            if let Some(info) = find_hover_in_class_header(cl, line, col, checker) {
                return Some(info);
            }
            // Field defaults and method bodies are code like any other.
            for field in &cl.fields {
                if let Some(default) = &field.default_value
                    && let Some(info) = find_hover_in_expr(default, line, col, checker, all_stmts)
                {
                    return Some(info);
                }
                if let Some(method) = &field.method_def {
                    for body_stmt in &method.body {
                        if let Some(info) =
                            find_hover_in_statement(body_stmt, line, col, checker, all_stmts)
                        {
                            return Some(info);
                        }
                    }
                }
            }
            None
        }
        Statement::Expr(expr) => find_hover_in_expr(expr, line, col, checker, all_stmts),
        Statement::ErrorCeiling(ec) => {
            for body_stmt in &ec.handler_body {
                if let Some(info) =
                    find_hover_in_statement(body_stmt, line, col, checker, all_stmts)
                {
                    return Some(info);
                }
            }
            None
        }
        Statement::UnmoldForward(uf) => {
            // Check source expression (e.g., in `expr >=> name`)
            if let Some(info) = find_hover_in_expr(&uf.source, line, col, checker, all_stmts) {
                return Some(info);
            }
            // Check if cursor is on the target variable
            if uf.span.line == line
                && let Some(ty) = uf
                    .type_annotation
                    .as_ref()
                    .map(format_type_expr)
                    .or_else(|| checker.lookup_var(&uf.target).map(|ty| ty.to_string()))
            {
                return Some(format!("```taida\n{}: {}\n```", uf.target, ty));
            }
            None
        }
        Statement::UnmoldBackward(ub) => {
            // Check source expression (e.g., in `name <=< expr`)
            if let Some(info) = find_hover_in_expr(&ub.source, line, col, checker, all_stmts) {
                return Some(info);
            }
            // The statement starts at the target it binds.
            if ub.span.line == line
                && ub.span.column <= col
                && col <= ub.span.column + ub.target.chars().count()
                && let Some(ty) = ub
                    .type_annotation
                    .as_ref()
                    .map(format_type_expr)
                    .or_else(|| checker.lookup_var(&ub.target).map(|ty| ty.to_string()))
            {
                return Some(format!("```taida\n{}: {}\n```", ub.target, ty));
            }
            None
        }
        _ => None,
    }
}

/// Hover on the name in the header of a type, mold or inheritance definition.
fn find_hover_in_class_header(
    cl: &crate::parser::ClassLikeDef,
    line: usize,
    col: usize,
    checker: &TypeChecker,
) -> Option<String> {
    match &cl.kind {
        crate::parser::ClassLikeKind::Alias { target } => {
            if cl.span.line == line
                && cl.span.column <= col
                && col <= cl.span.column + cl.name.chars().count()
            {
                let doc = format_doc_comments(&cl.doc_comments);
                return Some(format!(
                    "```taida\n{} = {}\n```{}",
                    cl.name,
                    format_type_expr(target),
                    doc
                ));
            }
            None
        }
        crate::parser::ClassLikeKind::BuchiPack => {
            let td = cl;
            if td.span.line == line
                && td.span.column <= col
                && col <= td.span.column + td.name.chars().count()
            {
                let fields: Vec<String> = td
                    .fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .map(|f| {
                        if let Some(ty) = &f.type_annotation {
                            format!("  {}: {}", f.name, format_type_expr(ty))
                        } else {
                            format!("  {}", f.name)
                        }
                    })
                    .collect();
                let doc = format_doc_comments(&td.doc_comments);
                return Some(format!(
                    "```taida\n{} = @(\n{}\n)\n```{}",
                    td.name,
                    fields.join(",\n"),
                    doc
                ));
            }
            None
        }
        crate::parser::ClassLikeKind::Mold { mold_args } => {
            let md = cl;
            if md.span.line == line
                && md.span.column <= col
                && col <= md.span.column + md.name.chars().count()
            {
                let parent_args: Vec<String> =
                    mold_args.iter().map(format_mold_header_arg).collect();
                let child_args: Vec<String> = md
                    .name_args
                    .as_ref()
                    .unwrap_or(mold_args)
                    .iter()
                    .map(format_mold_header_arg)
                    .collect();
                let fields: Vec<String> = md
                    .fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .map(|f| {
                        if let Some(ty) = &f.type_annotation {
                            format!("  {}: {}", f.name, format_type_expr(ty))
                        } else {
                            format!("  {}", f.name)
                        }
                    })
                    .collect();
                let doc = format_doc_comments(&md.doc_comments);
                return Some(format!(
                    "```taida\nMold[{}] => {}[{}] = @(\n{}\n)\n```{}",
                    parent_args.join(", "),
                    md.name,
                    child_args.join(", "),
                    fields.join(",\n"),
                    doc
                ));
            }
            None
        }
        crate::parser::ClassLikeKind::Inheritance {
            parent,
            parent_args,
        } => {
            let inh = cl;
            if inh.span.line == line {
                let parent_header = parent_args
                    .as_ref()
                    .map(|args| {
                        format!(
                            "{}[{}]",
                            parent,
                            args.iter()
                                .map(format_mold_header_arg)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
                    .unwrap_or_else(|| parent.clone());
                let child_header = inh
                    .name_args
                    .as_ref()
                    .map(|args| {
                        format!(
                            "{}[{}]",
                            inh.name,
                            args.iter()
                                .map(format_mold_header_arg)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
                    .unwrap_or_else(|| inh.name.clone());
                let fields: Vec<String> = inh
                    .fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .map(|f| {
                        if let Some(ty) = &f.type_annotation {
                            format!("  {}: {}", f.name, format_type_expr(ty))
                        } else {
                            format!("  {}", f.name)
                        }
                    })
                    .collect();
                let parent_fields = checker.registry.get_type_fields(parent).unwrap_or_default();
                let parent_fields_str: Vec<String> = parent_fields
                    .iter()
                    .map(|(n, t)| format!("  {}: {} (inherited)", n, t))
                    .collect();

                let mut all_fields = parent_fields_str;
                all_fields.extend(fields);

                let doc = format_doc_comments(&inh.doc_comments);
                return Some(format!(
                    "```taida\n{} => {} = @(\n{}\n)\n```{}",
                    parent_header,
                    child_header,
                    all_fields.join(",\n"),
                    doc
                ));
            }
            None
        }
    }
}

/// Whether the cursor (1-based `line` / `col`) is on the `len`-character token
/// that starts at `span`.
fn on_token(span: &crate::lexer::Span, line: usize, col: usize, len: usize) -> bool {
    span.line == line && span.column <= col && col < span.column + len.max(1)
}

/// The type the checker recorded for `expr`. A bare name the checker recorded
/// nothing for falls back to the top-level binding of that name; one it
/// recorded as unknown stays unknown, since a local of the same name may be
/// what is meant.
fn recorded_type(expr: &Expr, checker: &TypeChecker) -> Option<crate::types::Type> {
    match checker.typed_expr_table.lookup(expr) {
        Some(crate::types::Type::Unknown) => None,
        Some(ty) => Some(ty.clone()),
        None => match expr {
            Expr::Ident(name, _) => checker.lookup_var(name),
            _ => None,
        },
    }
}

/// Search an expression for hover-relevant nodes at the given position.
///
/// Only the token under the cursor answers: the children of an expression
/// are searched first (a name inside a call is the name, not the call), and
/// the expression's own operator or member name comes last.
fn find_hover_in_expr(
    expr: &Expr,
    line: usize,
    col: usize,
    checker: &TypeChecker,
    all_stmts: &[Statement],
) -> Option<String> {
    // The children of an expression, searched in source order. Template
    // literals hold their interpolations as text, whose positions are not
    // the document's, so they are left out.
    let search_children = |expr: &Expr| -> Option<String> {
        if matches!(expr, Expr::TemplateLit(..)) {
            return None;
        }
        let mut found = None;
        crate::graph::walk::for_each_child_of_expr(expr, &mut |child| {
            if found.is_some() {
                return;
            }
            found = match child {
                crate::graph::walk::Node::Expr(inner) => {
                    find_hover_in_expr(inner, line, col, checker, all_stmts)
                }
                crate::graph::walk::Node::Stmt(stmt) => {
                    find_hover_in_statement(stmt, line, col, checker, all_stmts)
                }
            };
        });
        found
    };

    match expr {
        Expr::Ident(name, span) => {
            if on_token(span, line, col, name.chars().count())
                && let Some(ty) = recorded_type(expr, checker)
            {
                return Some(format!("```taida\n{}: {}\n```", name, ty));
            }
            None
        }
        Expr::MethodCall(obj, method, _args, span) => {
            if let Some(info) = search_children(expr) {
                return Some(info);
            }
            // The call's span is the `(`; the method name is written just
            // before it.
            let name_len = method.chars().count();
            if span.line == line && span.column >= name_len && {
                let start = span.column - name_len;
                start <= col && col < span.column
            } {
                let obj_type = recorded_type(obj, checker)
                    .map(|ty| ty.to_string())
                    .unwrap_or_else(|| "?".to_string());
                return Some(format!(
                    "```taida\n{}.{}\nReceiver type: {}\n```",
                    obj_type, method, obj_type
                ));
            }
            None
        }
        Expr::FuncCall(func, _args, span) => {
            if let Some(info) = search_children(expr) {
                return Some(info);
            }
            // Show return type on the function name (or its parenthesis)
            if let Expr::Ident(name, name_span) = func.as_ref()
                && (on_token(name_span, line, col, name.chars().count())
                    || on_token(span, line, col, 1))
            {
                let ret = recorded_type(expr, checker)
                    .map(|ty| ty.to_string())
                    .unwrap_or_else(|| "?".to_string());
                return Some(format!("```taida\n{}(...) => :{}\n```", name, ret));
            }
            None
        }
        Expr::FieldAccess(obj, field, span) => {
            if let Some(info) = search_children(expr) {
                return Some(info);
            }
            // The access's span is the `.`; the field name follows it.
            let name_len = field.chars().count();
            if span.line == line && span.column <= col && col <= span.column + name_len {
                let obj_type = recorded_type(obj, checker).unwrap_or(crate::types::Type::Unknown);
                // Try to get the field type from the registry
                let field_type = match &obj_type {
                    crate::types::Type::Named(name) => {
                        checker.registry.get_type_fields(name).and_then(|fields| {
                            fields
                                .iter()
                                .find(|(n, _)| n == field)
                                .map(|(_, t)| format!("{}", t))
                        })
                    }
                    crate::types::Type::BuchiPack(fields) => fields
                        .iter()
                        .find(|(n, _)| n == field)
                        .map(|(_, t)| format!("{}", t)),
                    _ => None,
                };
                let type_info = field_type.unwrap_or_else(|| format!("{}", obj_type));
                return Some(format!(
                    "```taida\n.{}: {}\nObject type: {}\n```",
                    field, type_info, obj_type
                ));
            }
            None
        }
        Expr::MoldInst(name, _type_args, _fields, span) => {
            if let Some(info) = search_children(expr) {
                return Some(info);
            }
            if on_token(span, line, col, name.chars().count()) {
                if let Some(info) = find_user_mold_hover_info(all_stmts, name, checker) {
                    return Some(info);
                }
                // Built-in mold
                return Some(format!("```taida\n{}[...]\n```", name));
            }
            None
        }
        Expr::TypeInst(name, _fields, span) => {
            if let Some(info) = search_children(expr) {
                return Some(info);
            }
            if on_token(span, line, col, name.chars().count())
                && let Some(fields) = checker.registry.get_type_fields(name)
            {
                let fields_str: Vec<String> = fields
                    .iter()
                    .map(|(n, t)| format!("{}: {}", n, t))
                    .collect();
                return Some(format!(
                    "```taida\n{} = @({})\n```",
                    name,
                    fields_str.join(", ")
                ));
            }
            None
        }
        _ => search_children(expr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::TypeExpr;

    #[test]
    fn test_hover_variable_type() {
        let source = "x <= 42";
        // x is at line 1, col 1 (1-based in Span)
        // LSP Position is 0-based
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover info for variable x");
        let info = result.unwrap();
        assert!(info.contains("x"), "Should contain variable name");
        assert!(info.contains("Int"), "Should contain type Int");
    }

    #[test]
    fn test_hover_rust_addon_binding_as_function() {
        let source = "isTerminal <= RustAddon[\"isTerminal\"](arity <= 1)";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        let info = result.expect("Should get hover info for RustAddon binding");
        assert!(info.contains("isTerminal _arg0: Unknown => :Unknown"));
        assert!(info.contains("RustAddon[\"isTerminal\"](arity <= 1)"));
    }

    #[test]
    fn test_hover_function_def() {
        let source = "add a b = a + b => :Int";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover info for function add");
        let info = result.unwrap();
        assert!(info.contains("add"), "Should contain function name");
        assert!(info.contains("Int"), "Should contain return type");
    }

    #[test]
    fn test_hover_type_def() {
        let source = "Person = @(name: Str, age: Int)";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover info for type Person");
        let info = result.unwrap();
        assert!(info.contains("Person"), "Should contain type name");
        assert!(info.contains("name"), "Should contain field name");
        assert!(info.contains("age"), "Should contain field age");
    }

    #[test]
    fn test_hover_with_doc_comments() {
        let source = "///@ A person type\nPerson = @(name: Str, age: Int)";
        let result = get_hover_info(
            source,
            Position {
                line: 1,
                character: 0,
            },
        );
        assert!(
            result.is_some(),
            "Should get hover info for documented type"
        );
        let info = result.unwrap();
        assert!(info.contains("Person"), "Should contain type name");
        assert!(info.contains("A person type"), "Should contain doc comment");
    }

    #[test]
    fn test_hover_no_info_on_whitespace() {
        let source = "x <= 42";
        // Position far beyond the statement
        let result = get_hover_info(
            source,
            Position {
                line: 5,
                character: 0,
            },
        );
        assert!(
            result.is_none(),
            "Should return None for position with no code"
        );
    }

    #[test]
    fn test_hover_parse_error_returns_none() {
        let source = "this is not valid taida @@@";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_none(), "Should return None on parse error");
    }

    #[test]
    fn test_hover_mold_inst_uses_declared_mold_headers() {
        let source = r#"always_true x: Int =
  true
=> :Bool

Mold[T] => Result[T, P <= :T => :Bool] = @(
  pred: P
)
value <= Result[1, always_true]()
"#;
        let result = get_hover_info(
            source,
            Position {
                line: 7,
                character: 9,
            },
        );
        let info = result.expect("Should get hover info for mold instantiation");
        assert!(info.contains("Mold[T] => Result["));
        assert!(info.contains("P <="));
    }

    #[test]
    fn test_hover_inherited_mold_inst_uses_effective_child_headers() {
        let source = r#"Mold[:Int] => Base[:Int] = @()
Base[:Int] => Child[:Int, U] = @(
  extra: U
)
value <= Child[1, 2]()
"#;
        let result = get_hover_info(
            source,
            Position {
                line: 4,
                character: 9,
            },
        );
        let info = result.expect("Should get hover info for inherited mold instantiation");
        assert!(info.contains("Base[:Int] => Child[:Int, U] = @("));
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
        let te = TypeExpr::Generic("Lax".to_string(), vec![TypeExpr::Named("Int".to_string())]);
        assert_eq!(format_type_expr(&te), "Lax[Int]");
    }

    // ── RC-4c: hover quality tests ──

    #[test]
    fn test_rc4c_hover_string_variable() {
        let source = "name <= \"hello\"";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover for string variable");
        let info = result.unwrap();
        assert!(info.contains("name"), "Should contain variable name");
        assert!(info.contains("Str"), "Should contain type Str");
    }

    #[test]
    fn test_rc4c_hover_bool_variable() {
        let source = "flag <= true";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover for bool variable");
        let info = result.unwrap();
        assert!(info.contains("flag"), "Should contain variable name");
        assert!(info.contains("Bool"), "Should contain type Bool");
    }

    #[test]
    fn test_rc4c_hover_float_variable() {
        let source = "pi <= 3.14";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover for float variable");
        let info = result.unwrap();
        assert!(info.contains("pi"), "Should contain variable name");
        assert!(info.contains("Float"), "Should contain type Float");
    }

    #[test]
    fn test_rc4c_hover_list_variable() {
        let source = "items <= @[1, 2, 3]";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover for list variable");
        let info = result.unwrap();
        assert!(info.contains("items"), "Should contain variable name");
    }

    #[test]
    fn test_rc4c_hover_function_with_typed_params() {
        let source = "greet name: Str = stdout(name) => :Unit";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover for function");
        let info = result.unwrap();
        assert!(info.contains("greet"), "Should contain function name");
        assert!(info.contains("name"), "Should contain param name");
        assert!(info.contains("Str"), "Should contain param type");
        assert!(info.contains("Unit"), "Should contain return type");
    }

    #[test]
    fn test_rc4c_hover_function_with_multiple_params() {
        let source = "compute x: Int y: Int = x + y => :Int";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover for function");
        let info = result.unwrap();
        assert!(info.contains("compute"), "Should contain function name");
        assert!(info.contains("x"), "Should contain first param");
        assert!(info.contains("y"), "Should contain second param");
    }

    #[test]
    fn test_rc4c_hover_type_def_with_multiple_fields() {
        let source = "Config = @(host: Str, port: Int, debug: Bool)";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover for type");
        let info = result.unwrap();
        assert!(info.contains("Config"), "Should contain type name");
        assert!(info.contains("host"), "Should contain field host");
        assert!(info.contains("port"), "Should contain field port");
        assert!(info.contains("debug"), "Should contain field debug");
    }

    #[test]
    fn test_rc4c_hover_doc_comments_multiline() {
        let source = "///@ A configuration type\n///@ with multiple fields\nConfig = @(host: Str)";
        let result = get_hover_info(
            source,
            Position {
                line: 2,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover for documented type");
        let info = result.unwrap();
        assert!(info.contains("Config"), "Should contain type name");
        assert!(
            info.contains("A configuration type"),
            "Should contain first doc line"
        );
        assert!(
            info.contains("with multiple fields"),
            "Should contain second doc line"
        );
    }

    #[test]
    fn test_rc4c_hover_inheritance_def() {
        let source = "Vehicle = @(name: Str, speed: Int)\nVehicle => Car = @(doors: Int)";
        let result = get_hover_info(
            source,
            Position {
                line: 1,
                character: 0,
            },
        );
        assert!(
            result.is_some(),
            "hover should return info for inheritance def"
        );
        let info = result.unwrap();
        assert!(
            info.contains("Vehicle") || info.contains("Car"),
            "Should show inheritance info: {}",
            info
        );
    }

    #[test]
    fn test_rc4c_hover_variable_in_function_body() {
        let source = "x <= 42\nshow =\n  stdout(x)\n=> :Unit";
        // Hover on x reference inside function body (line 2, where x is used in stdout(x))
        let result = get_hover_info(
            source,
            Position {
                line: 2,
                character: 9,
            },
        );
        assert!(
            result.is_some(),
            "hover should return info for variable in function body"
        );
        let info = result.unwrap();
        assert!(
            info.contains("Int") || info.contains("x"),
            "Should show variable info: {}",
            info
        );
    }

    #[test]
    fn test_rc4c_hover_markdown_code_block() {
        let source = "x <= 42";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert!(
            info.contains("```taida"),
            "Hover should use taida code block: {}",
            info
        );
        assert!(
            info.contains("```"),
            "Hover should have closing code block fence"
        );
    }

    #[test]
    fn test_rc4c_hover_error_type_inheritance() {
        let source = "Error => AppError = @(code: Int)";
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(
            result.is_some(),
            "hover should return info for error type inheritance"
        );
        let info = result.unwrap();
        assert!(
            info.contains("Error") && info.contains("AppError"),
            "Should show error inheritance: {}",
            info
        );
    }

    #[test]
    fn test_rc4c_format_type_expr_function() {
        let te = TypeExpr::Function(
            vec![TypeExpr::Named("Int".to_string())],
            Box::new(TypeExpr::Named("Bool".to_string())),
        );
        assert_eq!(format_type_expr(&te), "Int => :Bool");
    }

    #[test]
    fn test_rc4c_format_type_expr_buchi_pack() {
        use crate::lexer::Span;
        let te = TypeExpr::BuchiPack(vec![
            crate::parser::FieldDef {
                name: "name".to_string(),
                type_annotation: Some(TypeExpr::Named("Str".to_string())),
                default_value: None,
                doc_comments: vec![],
                is_method: false,
                method_def: None,
                span: Span {
                    start: 0,
                    end: 0,
                    line: 0,
                    column: 0,
                    node_id: 0,
                },
            },
            crate::parser::FieldDef {
                name: "age".to_string(),
                type_annotation: Some(TypeExpr::Named("Int".to_string())),
                default_value: None,
                doc_comments: vec![],
                is_method: false,
                method_def: None,
                span: Span {
                    start: 0,
                    end: 0,
                    line: 0,
                    column: 0,
                    node_id: 0,
                },
            },
        ]);
        assert_eq!(format_type_expr(&te), "@(name: Str, age: Int)");
    }

    // ── RC-4f: hover for UnmoldForward/UnmoldBackward ──

    #[test]
    fn test_rc4f_hover_unmold_forward_source() {
        // Lax[42]() >=> value
        let source = "opt <= Lax[42]()\nopt >=> value";
        // Hover on "opt" in the unmold forward statement (line 1)
        let result = get_hover_info(
            source,
            Position {
                line: 1,
                character: 0,
            },
        );
        assert!(
            result.is_some(),
            "hover should return info for unmold forward source"
        );
        let info = result.unwrap();
        assert!(
            info.contains("opt") || info.contains("Lax"),
            "Should show info about source: {}",
            info
        );
    }

    #[test]
    fn test_rc4f_hover_unmold_backward_source() {
        // value <=< Lax[42]()
        let source = "opt <= Lax[42]()\nvalue <=< opt";
        // Hover on "opt" in the unmold backward statement (line 1)
        let result = get_hover_info(
            source,
            Position {
                line: 1,
                character: 10,
            },
        );
        assert!(
            result.is_some(),
            "hover should return info for unmold backward source"
        );
        let info = result.unwrap();
        assert!(
            info.contains("opt") || info.contains("Lax"),
            "Should show info about source: {}",
            info
        );
    }

    // ── RCB-54: UTF-16 position handling regression tests ──

    #[test]
    fn test_rcb54_hover_after_japanese_string() {
        // 'name <= "hello"' on line 0, then 'y <= 99' on line 1.
        // Japanese variable on line 0 to verify that hover on line 1 still works
        // even though line 0 has multi-byte chars.
        let source = "\u{540D}\u{524D} <= \"\u{3053}\u{3093}\u{306B}\u{3061}\u{306F}\"\ny <= 99";
        // Hover on 'y' at line 1, character 0 (UTF-16). 'y' is pure ASCII.
        let result = get_hover_info(
            source,
            Position {
                line: 1,
                character: 0,
            },
        );
        assert!(
            result.is_some(),
            "Should get hover info for variable after Japanese content"
        );
        let info = result.unwrap();
        assert!(info.contains("Int"), "y should have type Int: {}", info);
    }

    #[test]
    fn test_rcb54_hover_japanese_variable_name() {
        // Variable name is Japanese: chars are each 1 UTF-16 code unit
        // but 3 UTF-8 bytes.
        // "\u{540D}\u{524D}" == "名前" (2 chars, 2 UTF-16 units, 6 UTF-8 bytes)
        let source = "\u{540D}\u{524D} <= 42";
        // Hover at UTF-16 offset 0 should find the variable.
        let result = get_hover_info(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert!(
            result.is_some(),
            "Should get hover info for Japanese variable name"
        );
        let info = result.unwrap();
        assert!(
            info.contains("\u{540D}\u{524D}"),
            "Should contain the Japanese variable name: {}",
            info
        );
    }

    #[test]
    fn test_rcb54_hover_variable_after_emoji_string() {
        // "a\u{1F600}b" -- emoji is 2 UTF-16 code units.
        // Line: `x <= "a\u{1F600}b"`  then  `y <= 10`
        // Hover on y (line 1, char 0).
        let source = "x <= \"a\u{1F600}b\"\ny <= 10";
        let result = get_hover_info(
            source,
            Position {
                line: 1,
                character: 0,
            },
        );
        assert!(result.is_some(), "Should get hover for y after emoji line");
        let info = result.unwrap();
        assert!(info.contains("Int"), "y should be Int: {}", info);
    }
}

/// Completion provider for Taida Lang LSP.
///
/// Provides context-aware completion items:
/// - Variables and functions defined in the current document
/// - User-defined types (ClassLikeDef — BuchiPack / Mold / Inheritance kinds)
/// - Built-in mold types (30+ operation molds)
/// - Prelude functions (stdout, stderr, stdin, jsonEncode, jsonPretty, etc.)
/// - Operators (token-level: 10 tokens; the semantic 10-operator list
/// in PHILOSOPHY.md splits `(|... |>)` into two tokens and treats
/// `:` as type-annotation grammar)
/// - Field/method completion after `.`
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionParams, Documentation, InsertTextFormat,
    MarkupContent, MarkupKind,
};

use crate::parser::{FuncDef, Statement, parse};
use crate::types::{Type, TypeChecker};

use super::format::{
    format_mold_header_arg, format_named_mold_header, format_registry_fields_inline,
    format_type_expr,
};

/// Generate completion items based on context.
pub fn get_completions(params: &CompletionParams, source: Option<&str>) -> Vec<CompletionItem> {
    get_completions_for_file(params, source, None)
}

/// [`get_completions`] for a document stored at `path`, so the names it
/// imports from other modules are typed as `taida way check` types them.
pub fn get_completions_for_file(
    params: &CompletionParams,
    source: Option<&str>,
    path: Option<&std::path::Path>,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Check if we are completing after a dot (field/method access)
    let is_dot_completion = is_dot_trigger(params, source);

    if is_dot_completion {
        // After a dot, provide field/method completions
        let mut receiver_known = false;
        if let Some(src) = source
            && let Some(found) = dot_completions(src, params, path)
        {
            items.extend(found);
            receiver_known = true;
        }
        // `toString` is available on every value, so it is offered when the
        // receiver's type could not be worked out.
        if !receiver_known {
            items.extend(common_method_completions());
        }
        return items;
    }

    // Source-aware completions: variables, functions, types from the document
    if let Some(src) = source {
        items.extend(source_completions_for_file(src, path));
    }

    // Prelude functions (always available)
    items.extend(prelude_completions());

    // Built-in mold types
    items.extend(builtin_mold_completions());

    // Operators
    items.extend(operator_completions());

    // Type constructors and literals
    items.extend(type_completions());

    items
}

fn find_user_mold_detail(
    statements: &[Statement],
    name: &str,
    fields: &[(String, Type)],
) -> String {
    let fields_str = format_registry_fields_inline(fields);
    for stmt in statements {
        // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch
        if let Statement::ClassLikeDef(cl) = stmt {
            if cl.name != name {
                continue;
            }
            match &cl.kind {
                crate::parser::ClassLikeKind::Mold { mold_args } => {
                    let child_args = cl.name_args.as_deref().unwrap_or(mold_args.as_slice());
                    return format!(
                        "{} => {} = @({})",
                        format_named_mold_header("Mold", mold_args),
                        format_named_mold_header(&cl.name, child_args),
                        fields_str
                    );
                }
                crate::parser::ClassLikeKind::Inheritance {
                    parent,
                    parent_args,
                } => {
                    let child_args = cl
                        .name_args
                        .as_deref()
                        .or(parent_args.as_deref())
                        .unwrap_or(&[]);
                    let parent_header = match parent_args.as_deref() {
                        Some(args) => format_named_mold_header(parent, args),
                        None => parent.clone(),
                    };
                    return format!(
                        "{} => {} = @({})",
                        parent_header,
                        format_named_mold_header(&cl.name, child_args),
                        fields_str
                    );
                }
                crate::parser::ClassLikeKind::Alias { target } => {
                    return format!("{} = {}", cl.name, super::format::format_type_expr(target));
                }
                crate::parser::ClassLikeKind::BuchiPack => {}
            }
        }
    }
    format!("{} = @({})", name, fields_str)
}

/// Check if the trigger is a dot (for field/method completion).
fn is_dot_trigger(params: &CompletionParams, source: Option<&str>) -> bool {
    // Check trigger character
    if let Some(ctx) = &params.context
        && let Some(trigger) = &ctx.trigger_character
        && trigger == "."
    {
        return true;
    }
    // Also check if the character before the cursor (or before the member
    // name typed so far) is a dot
    if let Some(src) = source {
        let line = params.text_document_position.position.line as usize;
        let utf16_col = params.text_document_position.position.character as usize;
        if utf16_col > 0
            && let Some(line_text) = src.lines().nth(line)
        {
            let char_col = super::utf16::utf16_offset_to_char_index(line_text, utf16_col);
            return dot_before_member(line_text, char_col).is_some();
        }
    }
    false
}

fn is_member_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The char index of the `.` that member name being typed at `char_col`
/// follows, if the text before the cursor is `.` plus a (possibly empty)
/// member name.
fn dot_before_member(line_text: &str, char_col: usize) -> Option<usize> {
    let chars: Vec<char> = line_text.chars().collect();
    let mut start = char_col.min(chars.len());
    while start > 0 && is_member_char(chars[start - 1]) {
        start -= 1;
    }
    (start > 0 && chars[start - 1] == '.').then(|| start - 1)
}

/// Get completions from source analysis (variables, functions, types).
#[cfg(test)]
fn source_completions(source: &str) -> Vec<CompletionItem> {
    source_completions_for_file(source, None)
}

fn source_completions_for_file(
    source: &str,
    path: Option<&std::path::Path>,
) -> Vec<CompletionItem> {
    let (program, parse_errors) = parse(source);
    // The server answers on its main thread, whose stack is the process
    // default, so a deeply nested program is checked, and dropped, on a stack
    // sized for it.
    crate::graph::walk::with_stack_for_owned(program, |program| {
        if !parse_errors.is_empty() {
            // Even with parse errors, try to extract what we can from partial AST
            return partial_source_completions(&program.statements);
        }
        checked_source_completions(program, path)
    })
}

fn checked_source_completions(
    program: &crate::parser::Program,
    path: Option<&std::path::Path>,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Run type checker to populate scope info
    let mut checker = TypeChecker::new();
    checker.set_compile_target(crate::types::CompileTarget::Interpreter);
    if let Some(path) = path {
        checker.set_source_file(path);
    }
    checker.check_program(program);

    // Variables from scope
    for (name, ty) in checker.all_visible_vars() {
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some(format!("{}", ty)),
            ..Default::default()
        });
    }

    // Functions from scope
    for (name, ret_ty) in checker.all_functions() {
        // Find the function def to get parameter info
        let params_str = find_func_params(&program.statements, &name);
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("{} => :{}", params_str, ret_ty)),
            ..Default::default()
        });
    }

    // User-defined types
    for (name, fields) in &checker.registry.type_defs {
        // Skip the built-in Error type
        if name == "Error" {
            continue;
        }
        let fields_str: Vec<String> = fields
            .iter()
            .map(|(n, t)| format!("{}: {}", n, t))
            .collect();
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::STRUCT),
            detail: Some(format!("@({})", fields_str.join(", "))),
            documentation: find_type_doc_comments(&program.statements, name),
            ..Default::default()
        });
    }

    // User-defined mold types
    for (name, (type_params, fields)) in &checker.registry.mold_defs {
        let _ = type_params;
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::CLASS),
            detail: Some(find_user_mold_detail(&program.statements, name, fields)),
            documentation: find_mold_doc_comments(&program.statements, name),
            ..Default::default()
        });
    }

    items
}

/// Extract completions from partial AST (when parse errors exist).
fn partial_source_completions(statements: &[Statement]) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    for stmt in statements {
        match stmt {
            // Explicit addon bindings are surfaced as function completions.
            // `Name <= RustAddon["fn"](arity <= N)` binding を `FUNCTION`
            // として補完候補に出す。AST 上は Assignment だが、public
            // callable surface であり、user perspective では関数。
            Statement::Assignment(assign) if assign.as_rust_addon_binding().is_some() => {
                let (fn_name, arity) = assign.as_rust_addon_binding().unwrap();
                items.push(CompletionItem {
                    label: assign.target.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(format!("RustAddon[\"{}\"](arity <= {})", fn_name, arity)),
                    documentation: format_doc_comments(&assign.doc_comments),
                    ..Default::default()
                });
            }
            Statement::Assignment(assign) => {
                items.push(CompletionItem {
                    label: assign.target.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: assign.type_annotation.as_ref().map(format_type_expr),
                    ..Default::default()
                });
            }
            Statement::FuncDef(fd) => {
                let params_str = format_func_params(fd);
                items.push(CompletionItem {
                    label: fd.name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(params_str),
                    documentation: format_doc_comments(&fd.doc_comments),
                    ..Default::default()
                });
            }
            // (E30 Sub-step 2.1) ClassLikeDef + kind dispatch
            Statement::ClassLikeDef(cl) => match &cl.kind {
                crate::parser::ClassLikeKind::BuchiPack => {
                    items.push(CompletionItem {
                        label: cl.name.clone(),
                        kind: Some(CompletionItemKind::STRUCT),
                        detail: Some(format!("type {}", cl.name)),
                        documentation: format_doc_comments(&cl.doc_comments),
                        ..Default::default()
                    });
                }
                crate::parser::ClassLikeKind::Mold { .. } => {
                    items.push(CompletionItem {
                        label: cl.name.clone(),
                        kind: Some(CompletionItemKind::CLASS),
                        detail: Some(format!("mold {}", cl.name)),
                        documentation: format_doc_comments(&cl.doc_comments),
                        ..Default::default()
                    });
                }
                crate::parser::ClassLikeKind::Alias { target } => {
                    items.push(CompletionItem {
                        label: cl.name.clone(),
                        kind: Some(CompletionItemKind::STRUCT),
                        detail: Some(format!(
                            "{} = {}",
                            cl.name,
                            super::format::format_type_expr(target)
                        )),
                        documentation: format_doc_comments(&cl.doc_comments),
                        ..Default::default()
                    });
                }
                crate::parser::ClassLikeKind::Inheritance {
                    parent,
                    parent_args,
                } => {
                    let detail = match (parent_args, &cl.name_args) {
                        (Some(parent_args), Some(child_args)) => format!(
                            "{}[{}] => {}[{}]",
                            parent,
                            parent_args
                                .iter()
                                .map(format_mold_header_arg)
                                .collect::<Vec<_>>()
                                .join(", "),
                            cl.name,
                            child_args
                                .iter()
                                .map(format_mold_header_arg)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        (Some(parent_args), None) => format!(
                            "{}[{}] => {}",
                            parent,
                            parent_args
                                .iter()
                                .map(format_mold_header_arg)
                                .collect::<Vec<_>>()
                                .join(", "),
                            cl.name
                        ),
                        _ => format!("{} => {}", parent, cl.name),
                    };
                    items.push(CompletionItem {
                        label: cl.name.clone(),
                        kind: Some(CompletionItemKind::STRUCT),
                        detail: Some(detail),
                        documentation: format_doc_comments(&cl.doc_comments),
                        ..Default::default()
                    });
                }
            },
            _ => {}
        }
    }

    items
}

/// Get completions for fields/methods after a dot.
///
/// Returns `None` when the type of the value before the dot cannot be worked
/// out. Typing `s.` leaves the document unparseable, so the completion is
/// worked out on a copy without the dot (and the member name typed so far),
/// closed up if the line is left with an open bracket.
fn dot_completions(
    source: &str,
    params: &CompletionParams,
    path: Option<&std::path::Path>,
) -> Option<Vec<CompletionItem>> {
    let line = params.text_document_position.position.line as usize;
    let utf16_col = params.text_document_position.position.character as usize;
    let line_text = source.lines().nth(line)?;
    // Guard on the converted char index, not the raw UTF-16 offset.
    let col = super::utf16::utf16_offset_to_char_index(line_text, utf16_col);
    let dot = dot_before_member(line_text, col)?;

    // The name written before the dot.
    let chars: Vec<char> = line_text.chars().collect();
    let mut name_start = dot;
    while name_start > 0 && is_member_char(chars[name_start - 1]) {
        name_start -= 1;
    }
    let ident: String = chars[name_start..dot].iter().collect();
    if ident.is_empty() {
        return None;
    }

    // The line without the dot and the member name typed after it.
    let mut repaired_line: String = chars[..dot].iter().collect();
    repaired_line.extend(chars[col.min(chars.len())..].iter());
    let repaired_line = close_open_brackets(&repaired_line);
    let repaired: String = source
        .split_inclusive('\n')
        .enumerate()
        .map(|(i, l)| {
            if i == line {
                let newline = if l.ends_with('\n') { "\n" } else { "" };
                format!("{}{}", repaired_line, newline)
            } else {
                l.to_string()
            }
        })
        .collect();

    // Other unfinished lines of the document may still fail to parse. The
    // statements read before the failure are enough to type the names defined
    // above them, so the partial program is used as it is.
    let (program, _parse_errors) = parse(&repaired);
    // The server answers on its main thread, whose stack is the process
    // default, so a deeply nested program is checked, walked and dropped on a
    // stack sized for it.
    crate::graph::walk::with_stack_for_owned(program, |program| {
        let mut checker = TypeChecker::new();
        checker.set_compile_target(crate::types::CompileTarget::Interpreter);
        if let Some(path) = path {
            checker.set_source_file(path);
        }
        checker.check_program(program);

        // The lexer numbers columns after a leading byte order mark.
        let bom = usize::from(line == 0 && source.starts_with('\u{feff}'));
        let target = (line + 1, name_start + 1 - bom.min(name_start));
        let mut receiver_type = None;
        for stmt in &program.statements {
            crate::graph::walk::for_each_nested_stmt(stmt, &mut |nested| {
                if receiver_type.is_some() {
                    return;
                }
                let mut visit = |expr: &crate::parser::Expr| {
                    find_ident_type(expr, &ident, target, &checker, &mut receiver_type);
                };
                match nested {
                    Statement::Expr(e) => visit(e),
                    Statement::Assignment(a) => visit(&a.value),
                    Statement::UnmoldForward(u) => visit(&u.source),
                    Statement::UnmoldBackward(u) => visit(&u.source),
                    _ => {}
                }
            });
        }
        // A word after another dot names a field, not a variable. A word
        // found in the code is what it is there (a local can share its name
        // with a top-level variable); only a word that was not found is
        // looked up among the variables.
        let is_field = name_start > 0 && chars[name_start - 1] == '.';
        let ty = match receiver_type {
            Some(found) => found?,
            None if is_field => return None,
            None => checker
                .lookup_var(&ident)
                .filter(|ty| !matches!(ty, Type::Unknown))?,
        };
        Some(fields_for_type(&ty, &checker))
    })
}

/// `line` with brackets it leaves open closed again, so that a statement
/// cut short by removing `.member` still parses.
fn close_open_brackets(line: &str) -> String {
    let mut open: Vec<char> = Vec::new();
    let mut in_string: Option<char> = None;
    for c in line.chars() {
        match in_string {
            Some(quote) => {
                if c == quote {
                    in_string = None;
                }
            }
            None => match c {
                '"' | '\'' | '`' => in_string = Some(c),
                '(' => open.push(')'),
                '[' => open.push(']'),
                '{' => open.push('}'),
                ')' | ']' | '}' => {
                    if open.last() == Some(&c) {
                        open.pop();
                    }
                }
                _ => {}
            },
        }
    }
    let mut closed = line.to_string();
    while let Some(closer) = open.pop() {
        closed.push(closer);
    }
    closed
}

/// Find the identifier `name` written at `target` (line, column) inside
/// `expr` and store the type recorded for it: `Some(None)` when it was found
/// but the checker could not type it.
fn find_ident_type(
    expr: &crate::parser::Expr,
    name: &str,
    target: (usize, usize),
    checker: &TypeChecker,
    found: &mut Option<Option<Type>>,
) {
    if found.is_some() {
        return;
    }
    if let crate::parser::Expr::Ident(ident, span) = expr
        && ident == name
        && (span.line, span.column) == target
    {
        *found = Some(
            checker
                .typed_expr_table
                .lookup(expr)
                .filter(|ty| !matches!(ty, Type::Unknown))
                .cloned(),
        );
        return;
    }
    // A field named at the target: the checker recorded its type on the
    // access, which starts at the dot before the name.
    if let crate::parser::Expr::FieldAccess(_, field, span) = expr
        && field == name
        && span.line == target.0
        && span.column + 1 == target.1
    {
        *found = Some(
            checker
                .typed_expr_table
                .lookup(expr)
                .filter(|ty| !matches!(ty, Type::Unknown))
                .cloned(),
        );
        return;
    }
    if matches!(expr, crate::parser::Expr::TemplateLit(..)) {
        return;
    }
    crate::graph::walk::for_each_child_of_expr(expr, &mut |child| {
        if let crate::graph::walk::Node::Expr(inner) = child {
            find_ident_type(inner, name, target, checker, found);
        }
    });
}

/// Get field/method completion items for a given type.
fn fields_for_type(ty: &crate::types::Type, checker: &TypeChecker) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    match ty {
        crate::types::Type::BuchiPack(fields) => {
            for (name, field_ty) in fields {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FIELD),
                    detail: Some(format!("{}", field_ty)),
                    ..Default::default()
                });
            }
        }
        crate::types::Type::Named(name) if checker.registry.get_type_fields(name).is_some() => {
            if let Some(fields) = checker.registry.get_type_fields(name) {
                for (fname, fty) in &fields {
                    items.push(CompletionItem {
                        label: fname.clone(),
                        kind: Some(CompletionItemKind::FIELD),
                        detail: Some(format!("{}", fty)),
                        ..Default::default()
                    });
                }
            }
        }
        _ => {}
    }

    // The builtin methods come from the table the checker itself reads, so
    // only methods it accepts are offered.
    for method in crate::types::builtin_methods_of(ty) {
        let params = vec!["_"; method.max_args].join(", ");
        let returns = if matches!(method.returns, Type::Unknown) {
            String::new()
        } else {
            format!(" => :{}", method.returns)
        };
        items.push(CompletionItem {
            label: method.name.to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(format!("{}({}){}", method.name, params, returns)),
            ..Default::default()
        });
    }

    items
}

/// The method every value has: `toString`.
fn common_method_completions() -> Vec<CompletionItem> {
    let methods = [("toString", "() => :Str -- convert to string representation")];

    methods
        .iter()
        .map(|(label, detail)| CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(detail.to_string()),
            ..Default::default()
        })
        .collect()
}

/// Prelude functions (always available without import).
fn prelude_completions() -> Vec<CompletionItem> {
    let functions = [
        (
            "stdout",
            "stdout(text: Str) => :Int -- print to stdout",
            "Print text to standard output. Returns Int (bytes written).",
        ),
        (
            "stderr",
            "stderr(text: Str) => :Int -- print to stderr",
            "Print text to standard error. Returns Int (bytes written).",
        ),
        (
            "stdin",
            "stdin() => :Str -- read line from stdin (cooked mode)",
            "Read a line from standard input. Returns Str (empty on EOF / error). \
             For UTF-8-aware editing (multibyte Backspace), use `stdinLine` instead.",
        ),
        (
            "stdinLine",
            "stdinLine(prompt?) => :Async[Lax[Str]] -- UTF-8-aware line editor",
            "Read a line with UTF-8-aware editing (rustyline / readline/promises \
             / linenoise-derived). Returns Async[Lax[Str]]; use `>=>` to unmold, \
             e.g. `stdinLine(\"name: \") >=> line`. EOF / Ctrl-C / Ctrl-D collapse \
             to Lax.failure(\"\").",
        ),
        (
            "argv",
            "argv() => :@[Str] -- CLI user arguments",
            "Get CLI arguments as a string list (excluding executable/script path).",
        ),
        (
            "nowMs",
            "nowMs() => :Int -- wall-clock epoch milliseconds",
            "Get current wall-clock time in milliseconds since Unix epoch.",
        ),
        (
            "sleep",
            "sleep(ms: Int) => :Async[Int] -- wait asynchronously",
            "Return a pending Async that resolves to Int (the requested ms count) \
             after ms milliseconds. Taida forbids `Async[Unit]`; the resolved \
             value carries elapsed-time meaning instead of being a placeholder.",
        ),
        (
            "jsonEncode",
            "jsonEncode(value) => :Str -- encode to JSON",
            "Encode a value to a JSON string.",
        ),
        (
            "jsonPretty",
            "jsonPretty(value) => :Str -- encode to pretty JSON",
            "Encode a value to a pretty-printed JSON string.",
        ),
        (
            "debug",
            "debug(value) -- casual output for debugging",
            "Print a debug representation of a value.",
        ),
        (
            "typeof",
            "typeof(value) => :Str -- get type name",
            "Returns the type name of a value as a string.",
        ),
        (
            "assert",
            "assert(condition: Bool, message: Str?) => :Bool -- throw if false",
            "Assert a condition is true. Returns Bool(true) on success; throws an \
             error if false. The success path surfaces a meaningful Bool value \
             because Taida forbids `Unit` on the language surface.",
        ),
        (
            "range",
            "range(start: Int, end: Int) => :@[Int]",
            "Generate a list of integers from start (inclusive) to end (exclusive).",
        ),
        (
            "hashMap",
            "hashMap(entries: @[...]) => :HashMap",
            "Create a HashMap from a list of key-value pairs.",
        ),
        (
            "setOf",
            "setOf(items: @[...]) => :Set",
            "Create a Set from a list of items.",
        ),
        ("true", "Bool -- boolean true literal", ""),
        ("false", "Bool -- boolean false literal", ""),
    ];

    functions
        .iter()
        .map(|(label, detail, doc)| {
            let kind = if *label == "true" || *label == "false" {
                CompletionItemKind::KEYWORD
            } else {
                CompletionItemKind::FUNCTION
            };
            CompletionItem {
                label: label.to_string(),
                kind: Some(kind),
                detail: Some(detail.to_string()),
                documentation: if doc.is_empty() {
                    None
                } else {
                    Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: doc.to_string(),
                    }))
                },
                ..Default::default()
            }
        })
        .collect()
}

/// Built-in mold type completions (operation molds).
fn builtin_mold_completions() -> Vec<CompletionItem> {
    let molds: Vec<(&str, &str, &str)> = vec![
        // String molds
        (
            "Upper",
            "Upper[str]() => :Str",
            "Convert string to uppercase.",
        ),
        (
            "Lower",
            "Lower[str]() => :Str",
            "Convert string to lowercase.",
        ),
        (
            "Trim",
            "Trim[str]() => :Str",
            "Remove leading and trailing whitespace.",
        ),
        (
            "Split",
            "Split[str, sep]() => :@[Str]",
            "Split string by separator.",
        ),
        (
            "Replace",
            "Replace[str, old, new]() => :Str",
            "Replace occurrences of old with new.",
        ),
        (
            "Slice",
            "Slice[str, start](end <= n) => :Str",
            "Extract substring by index range.",
        ),
        (
            "CharAt",
            "CharAt[str, index]() => :Lax[Str]",
            "Get character at index. Returns Lax (out-of-range = has_value=false, default \"\").",
        ),
        (
            "Repeat",
            "Repeat[str, count]() => :Str",
            "Repeat string n times.",
        ),
        (
            "Reverse",
            "Reverse[str]() => :Str",
            "Reverse string characters.",
        ),
        (
            "Pad",
            "Pad[str, length](char <= \" \", side <= \"right\") => :Str",
            "Pad string to target length.",
        ),
        // Number molds
        (
            "ToFixed",
            "ToFixed[num, digits]() => :Str",
            "Format number with fixed decimal places.",
        ),
        ("Abs", "Abs[num]() => :Num", "Absolute value."),
        (
            "Floor",
            "Floor[num]() => :Int",
            "Round down to nearest integer.",
        ),
        (
            "Ceil",
            "Ceil[num]() => :Int",
            "Round up to nearest integer.",
        ),
        ("Round", "Round[num]() => :Int", "Round to nearest integer."),
        (
            "Truncate",
            "Truncate[num]() => :Int",
            "Truncate decimal part.",
        ),
        (
            "Clamp",
            "Clamp[num, min, max]() => :Num",
            "Clamp value to range [min, max].",
        ),
        (
            "Div",
            "Div[x, y]() => :Lax[Num]",
            "Safe division. Returns Lax (avoids DivisionError).",
        ),
        (
            "Mod",
            "Mod[x, y]() => :Lax[Num]",
            "Safe modulo. Returns Lax (avoids DivisionError).",
        ),
        // Type conversion molds
        (
            "Str",
            "Str[value]() => :Lax[Str]",
            "Convert value to Str. Returns Lax.",
        ),
        (
            "Int",
            "Int[value]() => :Lax[Int]",
            "Convert value to Int. Returns Lax.",
        ),
        (
            "Float",
            "Float[value]() => :Lax[Float]",
            "Convert value to Float. Returns Lax.",
        ),
        (
            "Bool",
            "Bool[value]() => :Lax[Bool]",
            "Convert value to Bool. Returns Lax.",
        ),
        // List molds
        (
            "Concat",
            "Concat[listA, listB]() => :@[T]",
            "Concatenate two lists.",
        ),
        (
            "Append",
            "Append[list, item]() => :@[T]",
            "Append item to end of list.",
        ),
        (
            "Prepend",
            "Prepend[item, list]() => :@[T]",
            "Prepend item to start of list.",
        ),
        (
            "Join",
            "Join[list, sep]() => :Str",
            "Join list elements with separator.",
        ),
        ("Sum", "Sum[list]() => :Num", "Sum all numeric elements."),
        ("Sort", "Sort[list]() => :@[T]", "Sort list elements."),
        (
            "Unique",
            "Unique[list]() => :@[T]",
            "Remove duplicate elements.",
        ),
        (
            "Flatten",
            "Flatten[list]() => :@[T]",
            "Flatten nested list by one level.",
        ),
        (
            "Find",
            "Find[list, predicate]() => :Lax[T]",
            "Find first matching element.",
        ),
        (
            "FindIndex",
            "FindIndex[list, predicate]() => :Lax[Int]",
            "Find index of first matching element.",
        ),
        (
            "Count",
            "Count[list, predicate]() => :Int",
            "Count elements matching predicate.",
        ),
        (
            "Zip",
            "Zip[listA, listB]() => :@[@(first, second)]",
            "Combine two lists pairwise.",
        ),
        (
            "Enumerate",
            "Enumerate[list]() => :@[@(index, value)]",
            "Add indices to list elements.",
        ),
        ("Map", "Map[list, fn]() => :@[U]", "Transform each element."),
        (
            "Filter",
            "Filter[list, predicate]() => :@[T]",
            "Keep elements matching predicate.",
        ),
        (
            "Fold",
            "Fold[list, init, fn]() => :U",
            "Reduce list to single value.",
        ),
        (
            "Reduce",
            "Reduce[list, init, fn]() => :U",
            "Alias for Fold.",
        ),
        // Core mold types
        (
            "Lax",
            "Lax[value]() -- safe value with default guarantee",
            "Lax[T]: BuchiPack-based mold. has_value / __value / __default / __type fields. Unmold with `>=>`.",
        ),
        (
            "Result",
            "Result[value, predicate](throw <= error) -- predicate-based operation mold",
            "Result[T, P]: Predicate-validated mold. P is a function :T => :Bool. Unmold evaluates predicate.",
        ),
        (
            "Async",
            "Async[value] -- asynchronous value container",
            "Async[T]: Wraps a value for asynchronous computation. Unmold with `>=>` to await.",
        ),
        (
            "Gorillax",
            "Gorillax[value]() -- like Lax but unmold failure = gorilla",
            "Gorillax[T]: Protected value. Unmold failure triggers gorilla exception.",
        ),
        (
            "Cage",
            "Cage[subject, runner]() -- run a branch-specific CageRilla capability on a Molten subject",
            "Cage[subject, runner]: subject branch must match runner CageRilla[Branch, Out]. Sync JS runners return Gorillax[Out]; JSCallAsync returns Async[Out].",
        ),
        (
            "CageRilla",
            "CageRilla[Branch, Out] -- parent type of Cage runner descriptors",
            "CageRilla[Branch, Out]: abstract parent descriptor. Write concrete JS runner constructors such as JSGet/JSCall instead of calling CageRilla directly.",
        ),
        // JSON mold
        (
            "JSON",
            "JSON[raw, Schema]() => :Lax[T]",
            "Parse JSON with schema. Returns Lax containing typed value matching Schema.",
        ),
        // JSRilla[Out] subfamily (JS backend only)
        (
            "JSGet",
            "JSGet[path, Out]() -- JSRilla[Out] for property/value get",
            "JS backend only. Build a JSRilla[Out] descriptor that reads subject.path. Used as runner of Cage[subject, JSGet[...]()]() -> Gorillax[Out].",
        ),
        (
            "JSCall",
            "JSCall[path, args, Out]() -- JSRilla[Out] for function/method call",
            "JS backend only. Build a JSRilla[Out] descriptor that calls subject.path(args...). Used as runner of Cage[subject, JSCall[...]()]() -> Gorillax[Out].",
        ),
        (
            "JSCallAsync",
            "JSCallAsync[path, args, Out]() -- async JSRilla[Out] for Promise-returning calls",
            "JS backend only. Used as runner of Cage[subject, JSCallAsync[...]()]() -> Async[Out]. Out is the resolved non-Async type; Promise rejection becomes an Async rejection.",
        ),
        (
            "JSNew",
            "JSNew[path, args, Out]() -- JSRilla[Out] for `new` instantiation",
            "JS backend only. Build a JSRilla[Out] descriptor that runs `new subject.path(args...)`. Used as runner of Cage[subject, JSNew[...]()]() -> Gorillax[Out].",
        ),
        (
            "JSSet",
            "JSSet[path, value]() -- JSRilla[Bool] for property set",
            "JS backend only. Build a JSRilla[Bool] descriptor that sets subject.path = value. Used as runner of Cage[subject, JSSet[...]()]() -> Gorillax[Bool]. Bool indicates assignment success (typically true).",
        ),
        (
            "JSBind",
            "JSBind[path]() -- JSRilla[Molten] for `this` binding",
            "JS backend only. Build a JSRilla[Molten] descriptor that returns subject.path.bind(subject). Used as runner of Cage[subject, JSBind[...]()]() -> Gorillax[Molten].",
        ),
        (
            "JSSpread",
            "JSSpread[source]() -- JSRilla[Molten] for spread merge",
            "JS backend only. Build a JSRilla[Molten] descriptor that merges source into subject. Used as runner of Cage[subject, JSSpread[...]()]() -> Gorillax[Molten].",
        ),
    ];

    molds
        .iter()
        .map(|(label, detail, doc)| CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            detail: Some(detail.to_string()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: doc.to_string(),
            })),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            ..Default::default()
        })
        .collect()
}

/// LSP operator completions. Lists the **token-level** Taida operator
/// surface so editors can offer per-token completions; the count below
/// (10 entries) is therefore not the same as the semantic 10-operator
/// list documented in PHILOSOPHY.md / `docs/reference/operators.md`.
///
/// Semantic vs token mapping:
/// - `(|... |>)` is one semantic operator (a condition delimiter
/// pair) but two tokens (`|` and `|>`) at the lexer level — both
/// tokens are surfaced individually here for editor UX.
/// - `:` is the 10th semantic operator (type marker) but is part of
/// identifier/type-annotation grammar at the token level, so it is
/// intentionally omitted from this completion list.
///
/// The semantic 10-operator pin lives in
/// `docs/reference/operators.md` (canonical) and is asserted in
/// `tests/c25b_005_diagnostic_audit.rs` / doc-tests, not here.
fn operator_completions() -> Vec<CompletionItem> {
    let operators = [
        ("=", "Type/inheritance definition"),
        ("=>", "Forward assignment / forward pipe"),
        ("<=", "Backward assignment"),
        (">=>", "Unmold forward (extract value from mold)"),
        ("<=<", "Unmold backward"),
        ("|==", "Error ceiling (gorilla ceiling / try-catch)"),
        ("|", "Condition branch arm (start of `(| ... |>)` pair)"),
        ("|>", "Condition branch result (end of `(| ... |>)` pair)"),
        (">>>", "Import module"),
        ("<<<", "Export symbols"),
    ];

    operators
        .iter()
        .map(|(label, detail)| CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::OPERATOR),
            detail: Some(detail.to_string()),
            ..Default::default()
        })
        .collect()
}

/// Type constructor and literal completions.
fn type_completions() -> Vec<CompletionItem> {
    let types = [
        (
            "@(",
            "BuchiPack literal: @(field <= value, ...)",
            "Named field record. Taida's primary data structure.",
        ),
        (
            "@[",
            "List literal: @[item1, item2, ...]",
            "Typed list. All elements must be the same type.",
        ),
        (
            "Mold",
            "Mold[T] => TypeName[T] = @(...)",
            "Define a custom mold type with type parameters.",
        ),
        (
            "Error",
            "Error => CustomError = @(...)",
            "Define a custom error type inheriting from Error.",
        ),
    ];

    types
        .iter()
        .map(|(label, detail, doc)| CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::STRUCT),
            detail: Some(detail.to_string()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: doc.to_string(),
            })),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            ..Default::default()
        })
        .collect()
}

/// Find function parameter info from AST.
fn find_func_params(statements: &[Statement], name: &str) -> String {
    for stmt in statements {
        if let Statement::FuncDef(fd) = stmt
            && fd.name == name
        {
            return format_func_params(fd);
        }
    }
    String::new()
}

/// Format function parameters for display.
fn format_func_params(fd: &FuncDef) -> String {
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
    params.join(" ")
}

/// Find doc_comments for a TypeDef by name.
/// ClassLikeDef + kind dispatch
fn find_type_doc_comments(statements: &[Statement], name: &str) -> Option<Documentation> {
    for stmt in statements {
        if let Statement::ClassLikeDef(cl) = stmt
            && cl.name == name
            && (cl.is_buchi_pack() || cl.is_inheritance())
        {
            return format_doc_comments(&cl.doc_comments);
        }
    }
    None
}

/// Find doc_comments for a MoldDef by name.
/// ClassLikeDef + kind dispatch
fn find_mold_doc_comments(statements: &[Statement], name: &str) -> Option<Documentation> {
    for stmt in statements {
        if let Statement::ClassLikeDef(cl) = stmt
            && cl.name == name
            && (cl.is_mold() || cl.is_inheritance())
        {
            return format_doc_comments(&cl.doc_comments);
        }
    }
    None
}

/// Format doc_comments into LSP Documentation.
fn format_doc_comments(doc_comments: &[String]) -> Option<Documentation> {
    if doc_comments.is_empty() {
        return None;
    }
    let text = doc_comments.join("\n");
    Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: text,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prelude_completions_include_stdout() {
        let items = prelude_completions();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"stdout"), "Should include stdout");
        assert!(labels.contains(&"stderr"), "Should include stderr");
        assert!(labels.contains(&"stdin"), "Should include stdin");
        assert!(labels.contains(&"nowMs"), "Should include nowMs");
        assert!(labels.contains(&"sleep"), "Should include sleep");
        assert!(labels.contains(&"jsonEncode"), "Should include jsonEncode");
        assert!(labels.contains(&"jsonPretty"), "Should include jsonPretty");
        assert!(labels.contains(&"debug"), "Should include debug");
        assert!(labels.contains(&"typeof"), "Should include typeof");
        assert!(labels.contains(&"assert"), "Should include assert");
        assert!(labels.contains(&"range"), "Should include range");
        assert!(labels.contains(&"hashMap"), "Should include hashMap");
        assert!(labels.contains(&"setOf"), "Should include setOf");
    }

    #[test]
    fn test_prelude_completions_no_std_modules() {
        let items = prelude_completions();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // std modules were dissolved in v0.6.0
        assert!(
            !labels.contains(&"std/math"),
            "Should NOT include dissolved std/math"
        );
        assert!(
            !labels.contains(&"std/io"),
            "Should NOT include dissolved std/io"
        );
    }

    #[test]
    fn test_builtin_mold_completions() {
        let items = builtin_mold_completions();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // String molds
        assert!(labels.contains(&"Upper"), "Should include Upper mold");
        assert!(labels.contains(&"Lower"), "Should include Lower mold");
        assert!(labels.contains(&"Split"), "Should include Split mold");
        // Number molds
        assert!(labels.contains(&"Abs"), "Should include Abs mold");
        assert!(labels.contains(&"Div"), "Should include Div mold");
        assert!(labels.contains(&"Mod"), "Should include Mod mold");
        // List molds
        assert!(labels.contains(&"Map"), "Should include Map mold");
        assert!(labels.contains(&"Filter"), "Should include Filter mold");
        assert!(labels.contains(&"Fold"), "Should include Fold mold");
        // Core molds
        assert!(labels.contains(&"Lax"), "Should include Lax mold");
        assert!(labels.contains(&"Result"), "Should include Result mold");
        assert!(labels.contains(&"Async"), "Should include Async mold");
        assert!(labels.contains(&"JSON"), "Should include JSON mold");
        assert!(labels.contains(&"Cage"), "Should include Cage mold");
    }

    #[test]
    fn test_operator_token_completions() {
        // F42 sweep: `operator_completions` lists 10 **token-level**
        // entries. The semantic 10-operator list (PHILOSOPHY.md /
        // `docs/reference/operators.md`) folds `|` and `|>` into one
        // `(| ... |>)` pair and adds `:` as the 10th — that pin lives
        // in docs, not in this LSP completion harness.
        let items = operator_completions();
        assert_eq!(
            items.len(),
            10,
            "operator_completions returns 10 token-level entries"
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"=>"), "Should include =>");
        assert!(labels.contains(&"<="), "Should include <=");
        assert!(labels.contains(&">=>"), "Should include >=>");
        assert!(labels.contains(&">>>"), "Should include >>>");
        assert!(labels.contains(&"<<<"), "Should include <<<");
    }

    #[test]
    fn test_source_completions_variables() {
        let source = "x <= 42\nname <= \"hello\"";
        let items = source_completions(source);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"x"), "Should include variable x");
        assert!(labels.contains(&"name"), "Should include variable name");
    }

    #[test]
    fn test_source_completions_functions() {
        let source = "add a b = a + b => :Int";
        let items = source_completions(source);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"add"), "Should include function add");
    }

    #[test]
    fn test_source_completions_type_defs() {
        let source = "Person = @(name: Str, age: Int)";
        let items = source_completions(source);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"Person"), "Should include type Person");
    }

    #[test]
    fn test_source_completions_mold_defs_include_effective_headers() {
        let source = r#"
always_true x: Int =
  true
=> :Bool

Mold[T] => Result[T, P <= :T => :Bool] = @(
  pred: P
)
"#;
        let items = source_completions(source);
        let result_item = items
            .iter()
            .find(|item| item.label == "Result" && item.kind == Some(CompletionItemKind::CLASS))
            .expect("Result completion should exist");
        let detail = result_item.detail.as_deref().expect("detail should exist");
        assert!(detail.contains("Mold[T] => Result["));
        assert!(detail.contains("P <="));
    }

    #[test]
    fn test_source_completions_inherited_molds_include_effective_headers() {
        let source = r#"
Mold[:Int] => Base[:Int] = @()
Base[:Int] => Child[:Int, U] = @(
  extra: U
)
"#;
        let items = source_completions(source);
        let child_item = items
            .iter()
            .find(|item| item.label == "Child" && item.kind == Some(CompletionItemKind::CLASS))
            .expect("Child completion should exist");
        let detail = child_item.detail.as_deref().expect("detail should exist");
        assert!(detail.contains("Base[:Int] => Child[:Int, U] = @("));
    }

    #[test]
    fn test_common_method_completions() {
        let items = common_method_completions();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"toString"), "Should include toString");
        // `hasValue` exists on Lax and Gorillax only; the checker rejects it
        // on other values, so it is not offered for a value of unknown type.
        assert!(!labels.contains(&"hasValue"), "hasValue is not universal");
    }

    #[test]
    fn test_type_completions() {
        let items = type_completions();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"@("), "Should include BuchiPack literal");
        assert!(labels.contains(&"@["), "Should include List literal");
        assert!(labels.contains(&"Mold"), "Should include Mold");
        assert!(labels.contains(&"Error"), "Should include Error");
    }

    // ── RC-4d: completion quality tests ──

    #[test]
    fn test_rc4d_prelude_completions_count() {
        let items = prelude_completions();
        // stdout, stderr, stdin, argv, nowMs, sleep, jsonEncode, jsonPretty,
        // debug, typeof, assert, range, hashMap, setOf, true, false = 16
        assert!(
            items.len() >= 15,
            "Should have at least 15 prelude items, got {}",
            items.len()
        );
    }

    #[test]
    fn test_rc4d_prelude_completions_argv() {
        let items = prelude_completions();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"argv"), "Should include argv");
    }

    #[test]
    fn test_rc4d_prelude_completions_have_documentation() {
        let items = prelude_completions();
        for item in &items {
            if item.label != "true" && item.label != "false" {
                assert!(
                    item.documentation.is_some(),
                    "Prelude function '{}' should have documentation",
                    item.label
                );
            }
        }
    }

    #[test]
    fn test_rc4d_prelude_completions_have_correct_kind() {
        let items = prelude_completions();
        for item in &items {
            match item.label.as_str() {
                "true" | "false" => {
                    assert_eq!(
                        item.kind,
                        Some(CompletionItemKind::KEYWORD),
                        "'{}' should be KEYWORD",
                        item.label
                    );
                }
                _ => {
                    assert_eq!(
                        item.kind,
                        Some(CompletionItemKind::FUNCTION),
                        "'{}' should be FUNCTION",
                        item.label
                    );
                }
            }
        }
    }

    #[test]
    fn test_rc4d_builtin_mold_completions_count() {
        let items = builtin_mold_completions();
        // 10 string + 9 number + 4 type conv + 14 list + 5 core + 1 JSON + 4 JS = 47
        assert!(
            items.len() >= 40,
            "Should have at least 40 built-in mold completions, got {}",
            items.len()
        );
    }

    #[test]
    fn test_rc4d_builtin_mold_completions_all_have_docs() {
        let items = builtin_mold_completions();
        for item in &items {
            assert!(
                item.documentation.is_some(),
                "Mold '{}' should have documentation",
                item.label
            );
            assert_eq!(
                item.kind,
                Some(CompletionItemKind::CLASS),
                "Mold '{}' should be CLASS kind",
                item.label
            );
        }
    }

    #[test]
    fn test_rc4d_operator_token_completions_exactly_10() {
        // F42 sweep: this asserts the **token-level** completion list
        // returned by `operator_completions`. The semantic 10-operator
        // pin (PHILOSOPHY.md / `docs/reference/operators.md`) folds
        // `|` + `|>` into a `(| ... |>)` delimiter pair and adds `:`
        // as the 10th. See the doc-comment on `operator_completions`
        // for the rationale; the canonical pin lives in docs / doc-tests.
        let items = operator_completions();
        assert_eq!(
            items.len(),
            10,
            "operator_completions returns 10 token-level entries"
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for op in &[
            "=", "=>", "<=", ">=>", "<=<", "|==", "|", "|>", ">>>", "<<<",
        ] {
            assert!(labels.contains(op), "Should include operator '{}'", op);
        }
    }

    #[test]
    fn test_rc4d_operator_completions_have_details() {
        let items = operator_completions();
        for item in &items {
            assert!(
                item.detail.is_some(),
                "Operator '{}' should have detail",
                item.label
            );
            assert_eq!(
                item.kind,
                Some(CompletionItemKind::OPERATOR),
                "Operator '{}' should be OPERATOR kind",
                item.label
            );
        }
    }

    #[test]
    fn test_rc4d_source_completions_typed_variable() {
        let source = "count: Int <= 42";
        let items = source_completions(source);
        let count_item = items.iter().find(|i| i.label == "count");
        assert!(
            count_item.is_some(),
            "Should include typed variable 'count'"
        );
        if let Some(item) = count_item {
            assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
            if let Some(detail) = &item.detail {
                assert!(
                    detail.contains("Int"),
                    "Variable detail should contain 'Int', got '{}'",
                    detail
                );
            }
        }
    }

    #[test]
    fn test_rc4d_source_completions_function_with_return_type() {
        let source = "double x: Int = x * 2 => :Int";
        let items = source_completions(source);
        let func_item = items
            .iter()
            .find(|i| i.label == "double" && i.kind == Some(CompletionItemKind::FUNCTION));
        assert!(func_item.is_some(), "Should include function 'double'");
        if let Some(item) = func_item {
            let detail = item.detail.as_deref().unwrap_or("");
            assert!(
                detail.contains("Int"),
                "Function detail should contain return type, got '{}'",
                detail
            );
        }
    }

    #[test]
    fn test_rc4d_source_completions_type_def_with_doc() {
        let source = "///@ A user type\nUser = @(name: Str, email: Str)";
        let items = source_completions(source);
        let type_item = items.iter().find(|i| i.label == "User");
        assert!(type_item.is_some(), "Should include type 'User'");
        if let Some(item) = type_item {
            assert_eq!(item.kind, Some(CompletionItemKind::STRUCT));
            assert!(
                item.documentation.is_some(),
                "Type with doc_comments should have documentation"
            );
        }
    }

    #[test]
    fn test_rc4d_source_completions_error_type_excluded() {
        let source = "x <= 42";
        let items = source_completions(source);
        let error_type_items: Vec<_> = items
            .iter()
            .filter(|i| i.label == "Error" && i.kind == Some(CompletionItemKind::STRUCT))
            .collect();
        assert!(
            error_type_items.is_empty(),
            "Built-in 'Error' type should be excluded from source completions"
        );
    }

    #[test]
    fn test_rc4d_dot_completion_str_methods() {
        let items = fields_for_type(&crate::types::Type::Str, &TypeChecker::new());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"length"), "Str should have length");
        assert!(labels.contains(&"contains"), "Str should have contains");
        assert!(labels.contains(&"startsWith"), "Str should have startsWith");
        assert!(labels.contains(&"endsWith"), "Str should have endsWith");
        assert!(labels.contains(&"indexOfLax"), "Str should have indexOfLax");
        assert!(labels.contains(&"toString"), "Str should have toString");
        // The checker rejects `isEmpty` on a Str ([E1509]), so it is not offered.
        assert!(!labels.contains(&"isEmpty"), "Str has no isEmpty");
    }

    #[test]
    fn test_rc4d_dot_completion_list_methods() {
        let list_ty = crate::types::Type::List(Box::new(crate::types::Type::Int));
        let items = fields_for_type(&list_ty, &TypeChecker::new());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"length"), "List should have length");
        assert!(labels.contains(&"isEmpty"), "List should have isEmpty");
        assert!(labels.contains(&"contains"), "List should have contains");
        assert!(labels.contains(&"get"), "List should have get");
        assert!(labels.contains(&"first"), "List should have first");
        assert!(labels.contains(&"last"), "List should have last");
        assert!(labels.contains(&"max"), "List should have max");
        assert!(labels.contains(&"min"), "List should have min");
        assert!(labels.contains(&"any"), "List should have any");
        assert!(labels.contains(&"all"), "List should have all");
        assert!(labels.contains(&"none"), "List should have none");
        assert!(labels.contains(&"toString"), "List should have toString");
    }

    #[test]
    fn test_rc4d_dot_completion_lax_methods() {
        let lax_ty = crate::types::Type::Generic("Lax".to_string(), vec![crate::types::Type::Int]);
        let items = fields_for_type(&lax_ty, &TypeChecker::new());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"hasValue"), "Lax should have hasValue");
        assert!(labels.contains(&"map"), "Lax should have map");
        assert!(labels.contains(&"flatMap"), "Lax should have flatMap");
        assert!(labels.contains(&"toString"), "Lax should have toString");
    }

    #[test]
    fn test_rc4d_dot_completion_result_methods() {
        let result_ty =
            crate::types::Type::Generic("Result".to_string(), vec![crate::types::Type::Int]);
        let items = fields_for_type(&result_ty, &TypeChecker::new());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"isSuccess"),
            "Result should have isSuccess"
        );
        assert!(labels.contains(&"isError"), "Result should have isError");
        assert!(labels.contains(&"map"), "Result should have map");
        assert!(labels.contains(&"flatMap"), "Result should have flatMap");
        assert!(labels.contains(&"toString"), "Result should have toString");
        assert!(!labels.contains(&"hasValue"), "Result has no hasValue");
    }

    #[test]
    fn test_rc4d_dot_completion_async_methods() {
        let async_ty =
            crate::types::Type::Generic("Async".to_string(), vec![crate::types::Type::Int]);
        let items = fields_for_type(&async_ty, &TypeChecker::new());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"map"), "Async should have map");
        assert!(labels.contains(&"isPending"), "Async should have isPending");
        assert!(labels.contains(&"toString"), "Async should have toString");
    }

    #[test]
    fn test_rc4d_dot_completion_buchi_pack_fields() {
        let bp_ty = crate::types::Type::BuchiPack(vec![
            ("name".to_string(), crate::types::Type::Str),
            ("age".to_string(), crate::types::Type::Int),
        ]);
        let items = fields_for_type(&bp_ty, &TypeChecker::new());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"name"), "BuchiPack should show field name");
        assert!(labels.contains(&"age"), "BuchiPack should show field age");
    }

    #[test]
    fn test_rc4d_dot_completion_named_type_fields() {
        let source = "Person = @(name: Str, age: Int)\np <= Person(name <= \"Alice\", age <= 30)";
        let items = source_completions(source);
        // Verify Person type is registered
        let person_item = items.iter().find(|i| i.label == "Person");
        assert!(person_item.is_some(), "Should include type Person");
    }

    #[test]
    fn test_rc4d_dot_completion_int_has_tostring() {
        let items = fields_for_type(&crate::types::Type::Int, &TypeChecker::new());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"toString"), "Int should have toString");
    }

    #[test]
    fn test_rc4d_partial_source_completions_recovers_variables() {
        // Even with parse errors, partial completions should work
        let source = "x <= 42\ny <= \"hello\"\nz <= @[";
        let items = source_completions(source);
        // partial_source_completions should try to recover x and y
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"x") || labels.contains(&"y"),
            "Should recover at least some variables from partial parse. Got: {:?}",
            labels
        );
    }

    #[test]
    fn test_rc4d_no_abolished_functions_in_prelude() {
        let items = prelude_completions();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Abolished functions should NOT appear
        assert!(!labels.contains(&"print"), "print is abolished");
        assert!(!labels.contains(&"println"), "println is abolished");
        assert!(!labels.contains(&"jsonParse"), "jsonParse is abolished");
        assert!(!labels.contains(&"jsonDecode"), "jsonDecode is abolished");
        assert!(!labels.contains(&"jsonFrom"), "jsonFrom is abolished");
        assert!(!labels.contains(&"Some"), "Some is abolished");
        assert!(!labels.contains(&"None"), "None is abolished");
        assert!(!labels.contains(&"Ok"), "Ok is abolished");
        assert!(!labels.contains(&"Err"), "Err is abolished");
    }

    #[test]
    fn test_rc4d_mold_completions_no_duplicates() {
        let items = builtin_mold_completions();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        let mut seen = std::collections::HashSet::new();
        for label in &labels {
            assert!(seen.insert(label), "Duplicate mold completion: '{}'", label);
        }
    }

    #[test]
    fn test_rc4d_common_method_completions_kind() {
        let items = common_method_completions();
        for item in &items {
            assert_eq!(
                item.kind,
                Some(CompletionItemKind::METHOD),
                "Common method '{}' should be METHOD kind",
                item.label
            );
        }
    }
}

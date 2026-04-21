/// Completion provider for Taida Lang LSP.
///
/// Provides context-aware completion items:
/// - Variables and functions defined in the current document
/// - User-defined types (TypeDef, MoldDef, InheritanceDef)
/// - Built-in mold types (30+ operation molds)
/// - Prelude functions (stdout, stderr, stdin, jsonEncode, jsonPretty, etc.)
/// - Operators (10 Taida operators)
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
    let mut items = Vec::new();

    // Check if we are completing after a dot (field/method access)
    let is_dot_completion = is_dot_trigger(params, source);

    if is_dot_completion {
        // After a dot, provide field/method completions
        if let Some(src) = source {
            items.extend(dot_completions(src, params));
        }
        // Also provide common state-check methods available on most types
        items.extend(common_method_completions());
        return items;
    }

    // Source-aware completions: variables, functions, types from the document
    if let Some(src) = source {
        items.extend(source_completions(src));
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
        match stmt {
            Statement::MoldDef(md) if md.name == name => {
                let child_args = md.name_args.as_deref().unwrap_or(md.mold_args.as_slice());
                return format!(
                    "{} => {} = @({})",
                    format_named_mold_header("Mold", &md.mold_args),
                    format_named_mold_header(&md.name, child_args),
                    fields_str
                );
            }
            Statement::InheritanceDef(inh) if inh.child == name => {
                let child_args = inh
                    .child_args
                    .as_deref()
                    .or(inh.parent_args.as_deref())
                    .unwrap_or(&[]);
                let parent_header = match inh.parent_args.as_deref() {
                    Some(args) => format_named_mold_header(&inh.parent, args),
                    None => inh.parent.clone(),
                };
                return format!(
                    "{} => {} = @({})",
                    parent_header,
                    format_named_mold_header(&inh.child, child_args),
                    fields_str
                );
            }
            _ => {}
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
    // Also check if the character before cursor is a dot
    if let Some(src) = source {
        let line = params.text_document_position.position.line as usize;
        let utf16_col = params.text_document_position.position.character as usize;
        if utf16_col > 0
            && let Some(line_text) = src.lines().nth(line)
        {
            let char_col = super::utf16::utf16_offset_to_char_index(line_text, utf16_col);
            if char_col > 0
                && let Some(ch) = line_text.chars().nth(char_col - 1)
            {
                return ch == '.';
            }
        }
    }
    false
}

/// Get completions from source analysis (variables, functions, types).
fn source_completions(source: &str) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    let (program, parse_errors) = parse(source);
    if !parse_errors.is_empty() {
        // Even with parse errors, try to extract what we can from partial AST
        return partial_source_completions(&program.statements);
    }

    // Run type checker to populate scope info
    let mut checker = TypeChecker::new();
    checker.check_program(&program);

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
            Statement::TypeDef(td) => {
                items.push(CompletionItem {
                    label: td.name.clone(),
                    kind: Some(CompletionItemKind::STRUCT),
                    detail: Some(format!("type {}", td.name)),
                    documentation: format_doc_comments(&td.doc_comments),
                    ..Default::default()
                });
            }
            Statement::MoldDef(md) => {
                items.push(CompletionItem {
                    label: md.name.clone(),
                    kind: Some(CompletionItemKind::CLASS),
                    detail: Some(format!("mold {}", md.name)),
                    documentation: format_doc_comments(&md.doc_comments),
                    ..Default::default()
                });
            }
            Statement::InheritanceDef(inh) => {
                let detail = match (&inh.parent_args, &inh.child_args) {
                    (Some(parent_args), Some(child_args)) => format!(
                        "{}[{}] => {}[{}]",
                        inh.parent,
                        parent_args
                            .iter()
                            .map(format_mold_header_arg)
                            .collect::<Vec<_>>()
                            .join(", "),
                        inh.child,
                        child_args
                            .iter()
                            .map(format_mold_header_arg)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    (Some(parent_args), None) => format!(
                        "{}[{}] => {}",
                        inh.parent,
                        parent_args
                            .iter()
                            .map(format_mold_header_arg)
                            .collect::<Vec<_>>()
                            .join(", "),
                        inh.child
                    ),
                    _ => format!("{} => {}", inh.parent, inh.child),
                };
                items.push(CompletionItem {
                    label: inh.child.clone(),
                    kind: Some(CompletionItemKind::STRUCT),
                    detail: Some(detail),
                    documentation: format_doc_comments(&inh.doc_comments),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    items
}

/// Get completions for fields/methods after a dot.
fn dot_completions(source: &str, params: &CompletionParams) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    let (program, parse_errors) = parse(source);
    if !parse_errors.is_empty() {
        return items;
    }

    let mut checker = TypeChecker::new();
    checker.check_program(&program);

    // Try to find the expression before the dot and infer its type
    let line = params.text_document_position.position.line as usize;
    let utf16_col = params.text_document_position.position.character as usize;

    // Simple approach: find the identifier before the dot on the current line
    if let Some(line_text) = source.lines().nth(line) {
        // S-2: Guard on converted char index, not raw UTF-16 offset.
        let col = super::utf16::utf16_offset_to_char_index(line_text, utf16_col);
        if col <= 1 {
            return items;
        }
        let before_dot: String = line_text.chars().take(col.saturating_sub(1)).collect();
        // Get the last identifier before the dot
        let ident = before_dot
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();

        if !ident.is_empty() {
            // Look up the type of this identifier
            if let Some(ty) = checker.lookup_var(&ident) {
                items.extend(fields_for_type(&ty, &checker));
            }
        }
    }

    items
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
        crate::types::Type::Named(name) => {
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
        crate::types::Type::Int | crate::types::Type::Float | crate::types::Type::Bool => {
            items.push(CompletionItem {
                label: "toString".to_string(),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some("() => :Str -- string representation".to_string()),
                ..Default::default()
            });
        }
        crate::types::Type::Str => {
            // String state-check methods
            for (method, detail) in &[
                ("length", "Int -- string length"),
                ("isEmpty", "Bool -- true if empty"),
                ("contains", "(sub: Str) => :Bool -- substring check"),
                ("startsWith", "(prefix: Str) => :Bool"),
                ("endsWith", "(suffix: Str) => :Bool"),
                ("indexOf", "(sub: Str) => :Int"),
                ("get", "(index: Int) => :Lax[Str] -- safe character access"),
                ("toString", "() => :Str"),
            ] {
                items.push(CompletionItem {
                    label: method.to_string(),
                    kind: Some(CompletionItemKind::METHOD),
                    detail: Some(detail.to_string()),
                    ..Default::default()
                });
            }
        }
        crate::types::Type::List(_) => {
            // List state-check methods
            for (method, detail) in &[
                ("length", "Int -- list length"),
                ("isEmpty", "Bool -- true if empty"),
                ("contains", "(item) => :Bool -- element check"),
                ("get", "(index: Int) => :Lax[T] -- safe element access"),
                ("first", "() => :Lax[T] -- first element"),
                ("last", "() => :Lax[T] -- last element"),
                ("max", "() => :Lax[T] -- maximum element"),
                ("min", "() => :Lax[T] -- minimum element"),
                ("any", "(predicate) => :Bool"),
                ("all", "(predicate) => :Bool"),
                ("none", "(predicate) => :Bool"),
                ("indexOf", "(item) => :Int -- index of first match"),
                ("toString", "() => :Str"),
            ] {
                items.push(CompletionItem {
                    label: method.to_string(),
                    kind: Some(CompletionItemKind::METHOD),
                    detail: Some(detail.to_string()),
                    ..Default::default()
                });
            }
        }
        crate::types::Type::Generic(name, _) => match name.as_str() {
            "Lax" => {
                for (method, detail) in &[
                    ("hasValue", "Bool -- true if has non-default value"),
                    ("isEmpty", "Bool -- true if no value"),
                    ("map", "(fn) => :Lax[U] -- transform inner value"),
                    ("flatMap", "(fn) => :Lax[U] -- monadic bind"),
                    ("toString", "() => :Str"),
                ] {
                    items.push(CompletionItem {
                        label: method.to_string(),
                        kind: Some(CompletionItemKind::METHOD),
                        detail: Some(detail.to_string()),
                        ..Default::default()
                    });
                }
            }
            "Result" => {
                for (method, detail) in &[
                    ("hasValue", "Bool -- true if success"),
                    ("isEmpty", "Bool -- true if failure"),
                    ("map", "(fn) => :Result[U, P] -- transform inner value"),
                    ("flatMap", "(fn) => :Result[U, P] -- monadic bind"),
                    ("toString", "() => :Str"),
                ] {
                    items.push(CompletionItem {
                        label: method.to_string(),
                        kind: Some(CompletionItemKind::METHOD),
                        detail: Some(detail.to_string()),
                        ..Default::default()
                    });
                }
            }
            "Async" => {
                for (method, detail) in &[
                    ("map", "(fn) => :Async[U] -- transform async value"),
                    ("flatMap", "(fn) => :Async[U] -- monadic bind"),
                    ("toString", "() => :Str"),
                ] {
                    items.push(CompletionItem {
                        label: method.to_string(),
                        kind: Some(CompletionItemKind::METHOD),
                        detail: Some(detail.to_string()),
                        ..Default::default()
                    });
                }
            }
            "HashMap" => {
                for (method, detail) in &[
                    ("get", "(key) => :Lax[V] -- safe value lookup"),
                    ("has", "(key) => :Bool -- check if key exists"),
                    ("keys", "() => :@[K] -- all keys"),
                    ("values", "() => :@[V] -- all values"),
                    ("entries", "() => :@[@(key, value)] -- all entries"),
                    ("size", "Int -- number of entries"),
                    ("isEmpty", "Bool -- true if no entries"),
                    ("remove", "(key) => :HashMap -- remove entry by key"),
                    ("toString", "() => :Str"),
                ] {
                    items.push(CompletionItem {
                        label: method.to_string(),
                        kind: Some(CompletionItemKind::METHOD),
                        detail: Some(detail.to_string()),
                        ..Default::default()
                    });
                }
            }
            "Set" => {
                for (method, detail) in &[
                    ("has", "(item) => :Bool -- check membership"),
                    ("add", "(item) => :Set -- add item"),
                    ("remove", "(item) => :Set -- remove item"),
                    ("union", "(other: Set) => :Set -- set union"),
                    ("intersect", "(other: Set) => :Set -- set intersection"),
                    ("size", "Int -- number of items"),
                    ("isEmpty", "Bool -- true if empty"),
                    ("toString", "() => :Str"),
                ] {
                    items.push(CompletionItem {
                        label: method.to_string(),
                        kind: Some(CompletionItemKind::METHOD),
                        detail: Some(detail.to_string()),
                        ..Default::default()
                    });
                }
            }
            _ => {}
        },
        _ => {}
    }

    items
}

/// Common state-check methods available on most types.
fn common_method_completions() -> Vec<CompletionItem> {
    let methods = [
        ("toString", "() => :Str -- convert to string representation"),
        (
            "hasValue",
            "Bool -- true if has non-default value (Lax/Result)",
        ),
    ];

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
            "stdout(text: Str) -- print to stdout",
            "Print text to standard output. Returns @().",
        ),
        (
            "stderr",
            "stderr(text: Str) -- print to stderr",
            "Print text to standard error. Returns @().",
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
             / linenoise-derived). Returns Async[Lax[Str]]; use `]=>` to unmold, \
             e.g. `stdinLine(\"name: \") ]=> line`. EOF / Ctrl-C / Ctrl-D collapse \
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
            "sleep(ms: Int) => :Async[Unit] -- wait asynchronously",
            "Return a pending Async that resolves to @() after ms milliseconds.",
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
            "assert(condition: Bool, message: Str) -- throw if false",
            "Assert a condition is true. Throws an error if false.",
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
            "Get character at index. Returns Lax (out-of-range = hasValue=false, default \"\").",
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
            "Lax[T]: BuchiPack-based mold. hasValue / __value / __default / __type fields. Unmold with `]=>`.",
        ),
        (
            "Result",
            "Result[value, predicate](throw <= error) -- predicate-based operation mold",
            "Result[T, P]: Predicate-validated mold. P is a function :T => :Bool. Unmold evaluates predicate.",
        ),
        (
            "Async",
            "Async[value] -- asynchronous value container",
            "Async[T]: Wraps a value for asynchronous computation. Unmold with `]=>` to await.",
        ),
        (
            "Gorillax",
            "Gorillax[value]() -- like Lax but unmold failure = gorilla",
            "Gorillax[T]: Protected value. Unmold failure triggers gorilla exception.",
        ),
        (
            "Cage",
            "Cage[molten, fn]() -- execute fn(molten) in protected context",
            "Cage[Molten, F]: Execute F(Molten) with error protection. Returns Gorillax. First arg must be Molten.",
        ),
        // JSON mold
        (
            "JSON",
            "JSON[raw, Schema]() => :Lax[T]",
            "Parse JSON with schema. Returns Lax containing typed value matching Schema.",
        ),
        // JS interop molds (JS backend only)
        (
            "JSNew",
            "JSNew[constructor, args]() -- JS new operator",
            "JS backend only. Create new JS object.",
        ),
        (
            "JSSet",
            "JSSet[obj, field, value]() -- JS property set",
            "JS backend only. Set JS object property.",
        ),
        (
            "JSBind",
            "JSBind[fn, thisArg]() -- JS Function.bind",
            "JS backend only. Bind `this` context.",
        ),
        (
            "JSSpread",
            "JSSpread[obj]() -- JS spread operator",
            "JS backend only. Spread object properties.",
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

/// Taida operator completions (10 operators).
fn operator_completions() -> Vec<CompletionItem> {
    let operators = [
        ("=", "Type/inheritance definition"),
        ("=>", "Forward assignment / forward pipe"),
        ("<=", "Backward assignment"),
        ("]=>", "Unmold forward (extract value from mold)"),
        ("<=[", "Unmold backward"),
        ("|==", "Error ceiling (gorilla ceiling / try-catch)"),
        ("|", "Condition branch arm"),
        ("|>", "Condition branch result"),
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
fn find_type_doc_comments(statements: &[Statement], name: &str) -> Option<Documentation> {
    for stmt in statements {
        match stmt {
            Statement::TypeDef(td) if td.name == name => {
                return format_doc_comments(&td.doc_comments);
            }
            Statement::InheritanceDef(inh) if inh.child == name => {
                return format_doc_comments(&inh.doc_comments);
            }
            _ => {}
        }
    }
    None
}

/// Find doc_comments for a MoldDef by name.
fn find_mold_doc_comments(statements: &[Statement], name: &str) -> Option<Documentation> {
    for stmt in statements {
        match stmt {
            Statement::MoldDef(md) if md.name == name => {
                return format_doc_comments(&md.doc_comments);
            }
            Statement::InheritanceDef(inh) if inh.child == name => {
                return format_doc_comments(&inh.doc_comments);
            }
            _ => {}
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
    fn test_operator_completions() {
        let items = operator_completions();
        assert_eq!(items.len(), 10, "Taida has exactly 10 operators");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"=>"), "Should include =>");
        assert!(labels.contains(&"<="), "Should include <=");
        assert!(labels.contains(&"]=>"), "Should include ]=>");
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
        assert!(labels.contains(&"hasValue"), "Should include hasValue");
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
    fn test_rc4d_operator_completions_exactly_10() {
        let items = operator_completions();
        assert_eq!(items.len(), 10, "Taida has exactly 10 operators");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Verify all 10
        for op in &[
            "=", "=>", "<=", "]=>", "<=[", "|==", "|", "|>", ">>>", "<<<",
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
        assert!(labels.contains(&"isEmpty"), "Str should have isEmpty");
        assert!(labels.contains(&"contains"), "Str should have contains");
        assert!(labels.contains(&"startsWith"), "Str should have startsWith");
        assert!(labels.contains(&"endsWith"), "Str should have endsWith");
        assert!(labels.contains(&"indexOf"), "Str should have indexOf");
        assert!(labels.contains(&"toString"), "Str should have toString");
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
        assert!(labels.contains(&"hasValue"), "Result should have hasValue");
        assert!(labels.contains(&"map"), "Result should have map");
        assert!(labels.contains(&"flatMap"), "Result should have flatMap");
        assert!(labels.contains(&"toString"), "Result should have toString");
    }

    #[test]
    fn test_rc4d_dot_completion_async_methods() {
        let async_ty =
            crate::types::Type::Generic("Async".to_string(), vec![crate::types::Type::Int]);
        let items = fields_for_type(&async_ty, &TypeChecker::new());
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"map"), "Async should have map");
        assert!(labels.contains(&"flatMap"), "Async should have flatMap");
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

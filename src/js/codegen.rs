use super::runtime::RUNTIME_JS;
/// Taida AST → JavaScript コード生成
use crate::parser::*;

pub struct JsCodegen {
    output: String,
    indent: usize,
    /// If set, tail calls to these function names should be converted to TailCall returns.
    /// For self-recursion: contains only the function itself.
    /// For mutual recursion: contains all functions in the mutual recursion group.
    current_tco_funcs: std::collections::HashSet<String>,
    /// Registry of TypeDef field names for InheritanceDef parent field resolution
    type_field_registry: std::collections::HashMap<String, Vec<String>>,
    /// Registry of mold field definitions for mold-aware inheritance codegen.
    mold_field_registry: std::collections::HashMap<String, Vec<FieldDef>>,
    /// Set of function names that need trampoline wrapping (self or mutual recursion)
    trampoline_funcs: std::collections::HashSet<String>,
    /// Set of function names that contain ]=> (unmold) and need `async function` generation
    async_funcs: std::collections::HashSet<String>,
    /// Whether we are currently generating code inside an async context.
    /// true at top-level (ESM top-level await) and inside async functions.
    /// When true, ]=> generates `await __taida_unmold_async(...)`.
    /// When false, ]=> generates `__taida_unmold(...)`.
    in_async_context: bool,
    /// Source .td file path (for resolving package import paths)
    source_file: Option<std::path::PathBuf>,
    /// Project root directory (for finding .taida/deps/)
    project_root: Option<std::path::PathBuf>,
    /// Output .mjs file path (for resolving package import paths relative to the final output)
    output_file: Option<std::path::PathBuf>,
    /// Entry source root directory (entry .td file's parent) — for output placement logic
    entry_root: Option<std::path::PathBuf>,
    /// Output root directory — for output placement logic
    out_root: Option<std::path::PathBuf>,
    /// Whether the current module imports taida-lang/net (guards net builtin rewriting)
    has_net_import: bool,
    /// Net builtin names that are shadowed by a parameter/local in the current scope.
    /// When a name is in this set, call-site rewriting to __taida_net_* is suppressed.
    shadowed_net_builtins: std::collections::HashSet<String>,
}

#[derive(Debug)]
pub struct JsError {
    pub message: String,
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JS codegen error: {}", self.message)
    }
}

fn is_removed_list_method(method: &str) -> bool {
    matches!(
        method,
        "push" | "sum" | "reverse" | "concat" | "join" | "sort" | "unique" | "flatten" | "filter"
    )
}

impl Default for JsCodegen {
    fn default() -> Self {
        Self::new()
    }
}

impl JsCodegen {
    pub fn new() -> Self {
        Self {
            output: String::new(),
            indent: 0,
            current_tco_funcs: std::collections::HashSet::new(),
            type_field_registry: std::collections::HashMap::new(),
            mold_field_registry: std::collections::HashMap::new(),
            trampoline_funcs: std::collections::HashSet::new(),
            async_funcs: std::collections::HashSet::new(),
            in_async_context: true, // top-level is async (ESM top-level await)
            source_file: None,
            project_root: None,
            output_file: None,
            entry_root: None,
            out_root: None,
            has_net_import: false,
            shadowed_net_builtins: std::collections::HashSet::new(),
        }
    }

    /// Set the source file, project root, and output file for package import resolution.
    pub fn set_file_context(
        &mut self,
        source_file: &std::path::Path,
        project_root: &std::path::Path,
        output_file: &std::path::Path,
    ) {
        self.source_file = Some(source_file.to_path_buf());
        self.project_root = Some(project_root.to_path_buf());
        self.output_file = Some(output_file.to_path_buf());
    }

    /// Set the entry root (entry .td file's parent) and output root for
    /// computing correct ESM import specifiers when modules are placed
    /// in a flattened output layout.
    pub fn set_build_context(&mut self, entry_root: &std::path::Path, out_root: &std::path::Path) {
        self.entry_root = Some(entry_root.to_path_buf());
        self.out_root = Some(out_root.to_path_buf());
    }

    /// Names of taida-lang/net builtins that require scope-aware call-site rewriting.
    /// HTTP v1 (3) + HTTP v2 (1) = 4.
    /// v3 streaming APIs will be added when JS backend is ready (Phase 4).
    const NET_BUILTIN_NAMES: &'static [&'static str] = &[
        "httpServe",
        "httpParseRequestHead",
        "httpEncodeResponse",
        "readBody",
        // v3 streaming API
        "startResponse",
        "writeChunk",
        "endResponse",
        "sseEvent",
    ];

    /// Check if a net builtin name should be rewritten to its __taida_net_* form.
    /// Returns true only when the module has a net import AND the name is not
    /// shadowed by a parameter/local in the current scope.
    fn should_rewrite_net_builtin(&self, name: &str) -> bool {
        self.has_net_import
            && Self::NET_BUILTIN_NAMES.contains(&name)
            && !self.shadowed_net_builtins.contains(name)
    }

    /// Try to write a net builtin rewrite. Returns true if the name was a net
    /// builtin and was rewritten (with optional suffix appended), false otherwise.
    /// This centralizes the 4-site rewrite pattern for net builtins.
    fn try_write_net_builtin(&mut self, name: &str, suffix: &str) -> bool {
        if !self.should_rewrite_net_builtin(name) {
            return false;
        }
        match name {
            "httpServe" => {
                self.write(&format!("__taida_net_httpServe{}", suffix));
                true
            }
            "httpParseRequestHead" => {
                self.write(&format!("__taida_net_httpParseRequestHead{}", suffix));
                true
            }
            "httpEncodeResponse" => {
                self.write(&format!("__taida_net_httpEncodeResponse{}", suffix));
                true
            }
            "readBody" => {
                self.write(&format!("__taida_net_readBody{}", suffix));
                true
            }
            // v3 streaming API
            "startResponse" => {
                self.write(&format!("__taida_net_startResponse{}", suffix));
                true
            }
            "writeChunk" => {
                self.write(&format!("__taida_net_writeChunk{}", suffix));
                true
            }
            "endResponse" => {
                self.write(&format!("__taida_net_endResponse{}", suffix));
                true
            }
            "sseEvent" => {
                self.write(&format!("__taida_net_sseEvent{}", suffix));
                true
            }
            _ => false,
        }
    }

    /// Program 全体を JS に変換
    pub fn generate(&mut self, program: &Program) -> Result<String, JsError> {
        let mut result = String::new();

        // Pre-pass: detect taida-lang/net import (guards net builtin rewriting)
        self.has_net_import = program
            .statements
            .iter()
            .any(|s| matches!(s, Statement::Import(imp) if imp.path == "taida-lang/net"));

        // Pre-pass: detect mutual recursion groups and mark functions for trampolining
        self.detect_trampoline_funcs(&program.statements);

        // Pre-pass: detect functions containing ]=> (unmold) that need async generation
        self.detect_async_funcs(&program.statements);

        // ランタイム埋め込み
        result.push_str(RUNTIME_JS);
        result.push('\n');

        // プログラム本体 — ErrorCeiling aware
        let stmts = &program.statements;
        self.gen_statement_sequence(stmts, &mut result)?;

        Ok(result)
    }

    /// Detect which functions need trampoline wrapping by analyzing
    /// self-recursion and mutual recursion (tail-call graph SCCs).
    fn detect_trampoline_funcs(&mut self, stmts: &[Statement]) {
        use std::collections::{HashMap, HashSet};

        // Collect all function definitions and their tail-call targets
        let mut func_names: Vec<String> = Vec::new();
        let mut tail_call_targets: HashMap<String, HashSet<String>> = HashMap::new();

        for stmt in stmts {
            if let Statement::FuncDef(fd) = stmt {
                func_names.push(fd.name.clone());
                let mut targets = HashSet::new();
                collect_tail_call_targets(&fd.name, &fd.body, &mut targets);
                tail_call_targets.insert(fd.name.clone(), targets);
            }
        }

        // Mark self-recursive functions
        for name in &func_names {
            if let Some(targets) = tail_call_targets.get(name)
                && targets.contains(name)
            {
                self.trampoline_funcs.insert(name.clone());
            }
        }

        // Find mutual recursion groups via SCC (Tarjan's algorithm simplified)
        // Build adjacency from tail-call targets (only between known functions)
        let func_set: HashSet<&str> = func_names.iter().map(|s| s.as_str()).collect();
        let mut visited = HashSet::new();
        let mut on_stack = HashSet::new();
        let mut stack = Vec::new();

        for name in &func_names {
            if !visited.contains(name.as_str()) {
                self.find_mutual_recursion_dfs(
                    name,
                    &tail_call_targets,
                    &func_set,
                    &mut visited,
                    &mut on_stack,
                    &mut stack,
                );
            }
        }
    }

    /// DFS to find mutual recursion cycles in the tail-call graph.
    fn find_mutual_recursion_dfs<'a>(
        &mut self,
        node: &'a str,
        tail_call_targets: &'a std::collections::HashMap<String, std::collections::HashSet<String>>,
        func_set: &std::collections::HashSet<&str>,
        visited: &mut std::collections::HashSet<&'a str>,
        on_stack: &mut std::collections::HashSet<String>,
        path: &mut Vec<String>,
    ) {
        visited.insert(node);
        on_stack.insert(node.to_string());
        path.push(node.to_string());

        if let Some(targets) = tail_call_targets.get(node) {
            for target in targets {
                if !func_set.contains(target.as_str()) {
                    continue; // Not a known function
                }
                if on_stack.contains(target.as_str()) {
                    // Found a cycle! Mark all functions in the cycle for trampolining
                    // SAFETY: `on_stack` and `path` are maintained in lockstep —
                    // every element pushed to `on_stack` is also pushed to `path`,
                    // so `on_stack.contains(target)` guarantees `path` contains `target`.
                    let cycle_start = path
                        .iter()
                        .position(|n| n == target)
                        .expect("on_stack/path invariant: target must exist in path");
                    for func_in_cycle in &path[cycle_start..] {
                        self.trampoline_funcs.insert(func_in_cycle.clone());
                    }
                } else if !visited.contains(target.as_str()) {
                    self.find_mutual_recursion_dfs(
                        target,
                        tail_call_targets,
                        func_set,
                        visited,
                        on_stack,
                        path,
                    );
                }
            }
        }

        path.pop();
        on_stack.remove(node);
    }

    /// Detect functions that contain ]=> on async-related mold values.
    /// Only functions that unmold async molds (Async, AsyncReject, All, Race, Timeout)
    /// need `async function` generation. Functions that only unmold sync molds
    /// (Div, Mod, Lax, Result) remain synchronous.
    /// After initial detection, propagate async transitively: if function A calls
    /// async function B, A is also async (needed for proper await in trampolined TCO).
    fn detect_async_funcs(&mut self, stmts: &[Statement]) {
        // Phase 1: direct async detection (functions with ]=> on async molds)
        for stmt in stmts {
            if let Statement::FuncDef(fd) = stmt
                && stmts_contain_async_unmold(&fd.body)
            {
                self.async_funcs.insert(fd.name.clone());
            }
        }

        // Phase 2: transitive propagation — if a function calls an async function,
        // it must also be async so the call can be awaited.
        // Build call graph: func_name -> set of called func names
        let mut call_graph: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let func_names: std::collections::HashSet<String> = stmts
            .iter()
            .filter_map(|s| {
                if let Statement::FuncDef(fd) = s {
                    Some(fd.name.clone())
                } else {
                    None
                }
            })
            .collect();
        for stmt in stmts {
            if let Statement::FuncDef(fd) = stmt {
                let mut callees = Vec::new();
                collect_func_calls_in_stmts(&fd.body, &func_names, &mut callees);
                call_graph.insert(fd.name.clone(), callees);
            }
        }

        // Fixed-point iteration: propagate async until stable
        loop {
            let mut changed = false;
            for (caller, callees) in &call_graph {
                if self.async_funcs.contains(caller) {
                    continue;
                }
                for callee in callees {
                    if self.async_funcs.contains(callee) {
                        self.async_funcs.insert(caller.clone());
                        changed = true;
                        break;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Statement sequence with ErrorCeiling handling.
    /// When an ErrorCeiling is encountered, subsequent statements become the try body.
    fn gen_statement_sequence(
        &mut self,
        stmts: &[Statement],
        result: &mut String,
    ) -> Result<(), JsError> {
        let mut i = 0;
        while i < stmts.len() {
            if let Statement::ErrorCeiling(ec) = &stmts[i] {
                // B1: ErrorCeiling wraps all subsequent statements in try block
                self.output.clear();
                self.writeln("try {");
                result.push_str(&self.output);

                self.indent += 1;
                // Subsequent statements become the try body
                let remaining = &stmts[i + 1..];
                self.gen_statement_sequence(remaining, result)?;
                self.indent -= 1;

                // RCB-101: Extract handler type name for error type filtering
                let handler_type = match &ec.error_type {
                    TypeExpr::Named(name) => name.as_str(),
                    _ => "Error",
                };

                self.output.clear();
                self.write_indent();
                self.write("} catch (__taida_caught_err) {\n");
                result.push_str(&self.output);

                self.indent += 1;

                // RCB-101: Type filter — re-throw if type does not match
                self.output.clear();
                self.write_indent();
                self.write("const __taida_thrown_type = __taida_caught_err.type || (__taida_caught_err.__type) || 'Error';\n");
                result.push_str(&self.output);

                self.output.clear();
                self.write_indent();
                self.write(&format!(
                    "if (!__taida_is_error_subtype(__taida_thrown_type, '{}')) throw __taida_caught_err;\n",
                    handler_type
                ));
                result.push_str(&self.output);

                // Bind the error to the user's parameter name
                self.output.clear();
                self.write_indent();
                self.write(&format!("const {} = __taida_caught_err;\n", ec.error_param));
                result.push_str(&self.output);

                for stmt in &ec.handler_body {
                    self.output.clear();
                    self.gen_statement(stmt)?;
                    result.push_str(&self.output);
                }
                self.indent -= 1;

                self.output.clear();
                self.writeln("}");
                result.push_str(&self.output);

                // All remaining statements were consumed by the try block
                return Ok(());
            }

            self.output.clear();
            self.gen_statement(&stmts[i])?;
            result.push_str(&self.output);
            i += 1;
        }
        Ok(())
    }

    fn write(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
    }

    fn writeln(&mut self, s: &str) {
        self.write_indent();
        self.output.push_str(s);
        self.output.push('\n');
    }

    /// Convert a TypeExpr to a JSON schema string for __taida_registerTypeDef.
    /// Handles Named types, List types, and inline BuchiPack types recursively.
    fn type_expr_to_schema(type_annotation: &Option<crate::parser::TypeExpr>) -> String {
        match type_annotation {
            Some(crate::parser::TypeExpr::Named(n)) => format!("'{}'", n),
            Some(crate::parser::TypeExpr::List(inner)) => {
                let inner_schema = Self::type_expr_to_schema(&Some(inner.as_ref().clone()));
                format!("{{ __list: {} }}", inner_schema)
            }
            Some(crate::parser::TypeExpr::BuchiPack(fields)) => {
                // Inline buchi pack type: @(field1: Type1, field2: Type2)
                // Generate { field1: schema1, field2: schema2 }
                let mut parts = Vec::new();
                for f in fields {
                    if !f.is_method {
                        let field_schema = Self::type_expr_to_schema(&f.type_annotation);
                        parts.push(format!("{}: {}", f.name, field_schema));
                    }
                }
                format!("{{ {} }}", parts.join(", "))
            }
            _ => "'Str'".to_string(),
        }
    }

    fn gen_field_default_expr(&mut self, field: &FieldDef) -> Result<(), JsError> {
        if let Some(default_expr) = &field.default_value {
            self.gen_expr(default_expr)?;
            return Ok(());
        }
        if let Some(ty) = &field.type_annotation {
            let schema = Self::type_expr_to_schema(&Some(ty.clone()));
            self.write("__taida_defaultForSchema(");
            self.write(&schema);
            self.write(")");
            return Ok(());
        }
        self.write("__taida_defaultValue('unknown')");
        Ok(())
    }

    fn gen_param_default_expr(&mut self, param: &Param) -> Result<(), JsError> {
        if let Some(default_expr) = &param.default_value {
            self.gen_expr(default_expr)?;
            return Ok(());
        }
        if let Some(ty) = &param.type_annotation {
            let schema = Self::type_expr_to_schema(&Some(ty.clone()));
            self.write("__taida_defaultForSchema(");
            self.write(&schema);
            self.write(")");
            return Ok(());
        }
        self.write("__taida_defaultValue('unknown')");
        Ok(())
    }

    fn gen_func_param_prologue_to_buf(
        &mut self,
        func_def: &FuncDef,
        result: &mut String,
    ) -> Result<(), JsError> {
        self.output.clear();
        self.write_indent();
        self.write(&format!(
            "if (arguments.length > {}) {{\n",
            func_def.params.len()
        ));
        result.push_str(&self.output);

        self.indent += 1;
        self.output.clear();
        self.write_indent();
        self.write(&format!(
            "throw new __TaidaError('ArgumentError', `Function '{}' expected at most {} argument(s), got ${{arguments.length}}`, {{}});\n",
            func_def.name,
            func_def.params.len()
        ));
        result.push_str(&self.output);
        self.indent -= 1;

        self.output.clear();
        self.write_indent();
        self.write("}\n");
        result.push_str(&self.output);

        for (i, param) in func_def.params.iter().enumerate() {
            self.output.clear();
            self.write_indent();
            self.write(&format!("if (arguments.length <= {}) {{\n", i));
            result.push_str(&self.output);

            self.indent += 1;
            self.output.clear();
            self.write_indent();
            self.write(&format!("{} = ", param.name));
            self.gen_param_default_expr(param)?;
            self.write(";\n");
            result.push_str(&self.output);
            self.indent -= 1;

            self.output.clear();
            self.write_indent();
            self.write("}\n");
            result.push_str(&self.output);
        }

        Ok(())
    }

    fn gen_statement(&mut self, stmt: &Statement) -> Result<(), JsError> {
        match stmt {
            Statement::Expr(expr) => {
                self.write_indent();
                // In async context, await standalone calls to async functions
                // so their side effects complete before the next statement.
                if self.in_async_context
                    && let Expr::FuncCall(callee, _, _) = expr
                    && let Expr::Ident(name, _) = callee.as_ref()
                    && self.async_funcs.contains(name)
                {
                    self.write("await ");
                }
                self.gen_expr(expr)?;
                self.write(";\n");
                Ok(())
            }
            Statement::Assignment(assign) => {
                self.write_indent();
                self.write(&format!("const {} = ", assign.target));
                // In async context, await RHS calls to async functions
                if self.in_async_context
                    && let Expr::FuncCall(callee, _, _) = &assign.value
                    && let Expr::Ident(name, _) = callee.as_ref()
                    && self.async_funcs.contains(name)
                {
                    self.write("await ");
                }
                self.gen_expr(&assign.value)?;
                self.write(";\n");
                // Track local assignment shadow: if the target name matches a net
                // builtin, subsequent calls in the same scope must use the local
                // value, not the builtin rewrite.
                if self.has_net_import && Self::NET_BUILTIN_NAMES.contains(&assign.target.as_str())
                {
                    self.shadowed_net_builtins.insert(assign.target.clone());
                }
                Ok(())
            }
            Statement::FuncDef(func_def) => self.gen_func_def(func_def),
            Statement::TypeDef(type_def) => self.gen_type_def(type_def),
            Statement::InheritanceDef(inh_def) => self.gen_inheritance_def(inh_def),
            Statement::MoldDef(mold_def) => self.gen_mold_def(mold_def),
            Statement::ErrorCeiling(ec) => {
                // Standalone ErrorCeiling (when not handled by gen_statement_sequence)
                self.gen_error_ceiling(ec)
            }
            Statement::Import(import) => self.gen_import(import),
            Statement::Export(export) => self.gen_export(export),
            Statement::UnmoldForward(unmold) => {
                self.write_indent();
                let (await_prefix, unmold_fn) = if self.in_async_context {
                    ("await ", "__taida_unmold_async")
                } else {
                    ("", "__taida_unmold")
                };
                self.write(&format!(
                    "const {} = {await_prefix}{unmold_fn}(",
                    unmold.target
                ));
                self.gen_expr(&unmold.source)?;
                self.write(");\n");
                // Track local unmold-forward shadow for net builtins
                if self.has_net_import && Self::NET_BUILTIN_NAMES.contains(&unmold.target.as_str())
                {
                    self.shadowed_net_builtins.insert(unmold.target.clone());
                }
                Ok(())
            }
            Statement::UnmoldBackward(unmold) => {
                self.write_indent();
                let (await_prefix, unmold_fn) = if self.in_async_context {
                    ("await ", "__taida_unmold_async")
                } else {
                    ("", "__taida_unmold")
                };
                self.write(&format!(
                    "const {} = {await_prefix}{unmold_fn}(",
                    unmold.target
                ));
                self.gen_expr(&unmold.source)?;
                self.write(");\n");
                // Track local unmold-backward shadow for net builtins
                if self.has_net_import && Self::NET_BUILTIN_NAMES.contains(&unmold.target.as_str())
                {
                    self.shadowed_net_builtins.insert(unmold.target.clone());
                }
                Ok(())
            }
        }
    }

    fn gen_func_def(&mut self, func_def: &FuncDef) -> Result<(), JsError> {
        let needs_trampoline = self.trampoline_funcs.contains(&func_def.name);
        let is_async_fn = self.async_funcs.contains(&func_def.name);
        // Trampoline functions can be async if they call async functions
        let needs_async = is_async_fn;
        let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();

        // Save async context — set to true for async functions so ]=> generates `await`
        let prev_async_context = self.in_async_context;
        self.in_async_context = needs_async;

        // Scope-aware net builtin shadowing: snapshot before function body,
        // restore after. This covers both parameter shadows and local assignment
        // shadows (e.g. `httpServe <= add`) within the function scope.
        let prev_shadowed_net = self.shadowed_net_builtins.clone();
        for p in &func_def.params {
            if Self::NET_BUILTIN_NAMES.contains(&p.name.as_str()) {
                self.shadowed_net_builtins.insert(p.name.clone());
            }
        }

        if needs_trampoline {
            // Trampoline-based TCO: generate inner function, then trampoline wrapper
            let async_prefix = if needs_async { "async " } else { "" };
            self.write_indent();
            self.write(&format!(
                "const __inner_{} = {async_prefix}function({}) {{\n",
                func_def.name,
                params.join(", ")
            ));

            let mut result = std::mem::take(&mut self.output);

            self.indent += 1;
            // Set TCO context: all trampoline functions are potential tail-call targets
            let prev_tco = std::mem::take(&mut self.current_tco_funcs);
            for f in self.trampoline_funcs.iter() {
                self.current_tco_funcs.insert(f.clone());
            }
            self.gen_func_param_prologue_to_buf(func_def, &mut result)?;
            self.gen_func_body_to_buf(&func_def.body, &mut result)?;
            self.current_tco_funcs = prev_tco;
            self.indent -= 1;

            self.output = result;
            self.writeln("};");
            self.write_indent();
            let trampoline_fn = if needs_async {
                "__taida_trampoline_async"
            } else {
                "__taida_trampoline"
            };
            self.write(&format!(
                "const {} = {}(__inner_{});\n\n",
                func_def.name, trampoline_fn, func_def.name
            ));
        } else {
            self.write_indent();
            let async_prefix = if needs_async { "async " } else { "" };
            self.write(&format!(
                "{async_prefix}function {}({}) {{\n",
                func_def.name,
                params.join(", ")
            ));

            let mut result = std::mem::take(&mut self.output);

            self.indent += 1;
            self.gen_func_param_prologue_to_buf(func_def, &mut result)?;
            self.gen_func_body_to_buf(&func_def.body, &mut result)?;
            self.indent -= 1;

            self.output = result;
            self.writeln("}\n");
        }

        // Restore net builtin shadow set to pre-function state
        self.shadowed_net_builtins = prev_shadowed_net;

        // Restore async context
        self.in_async_context = prev_async_context;
        Ok(())
    }

    /// Function body with ErrorCeiling handling and implicit return on last expression.
    /// Writes to an external buffer, leaving self.output usage internal per statement.
    fn gen_func_body_to_buf(
        &mut self,
        stmts: &[Statement],
        result: &mut String,
    ) -> Result<(), JsError> {
        let mut i = 0;
        while i < stmts.len() {
            if let Statement::ErrorCeiling(ec) = &stmts[i] {
                // ErrorCeiling wraps all subsequent statements in try block
                self.output.clear();
                self.writeln("try {");
                result.push_str(&self.output);

                self.indent += 1;
                // Remaining statements become the try body (with implicit return)
                let remaining = &stmts[i + 1..];
                self.gen_func_body_to_buf(remaining, result)?;
                self.indent -= 1;

                // RCB-101: Extract handler type name for error type filtering
                let handler_type = match &ec.error_type {
                    TypeExpr::Named(name) => name.as_str(),
                    _ => "Error",
                };

                self.output.clear();
                self.write_indent();
                self.write("} catch (__taida_caught_err) {\n");
                result.push_str(&self.output);

                self.indent += 1;

                // RCB-101: Type filter — re-throw if type does not match
                self.output.clear();
                self.writeln("const __taida_thrown_type = __taida_caught_err.type || (__taida_caught_err.__type) || 'Error';");
                result.push_str(&self.output);

                self.output.clear();
                self.writeln(&format!(
                    "if (!__taida_is_error_subtype(__taida_thrown_type, '{}')) throw __taida_caught_err;",
                    handler_type
                ));
                result.push_str(&self.output);

                self.output.clear();
                self.writeln(&format!("const {} = __taida_caught_err;", ec.error_param));
                result.push_str(&self.output);

                for (j, handler_stmt) in ec.handler_body.iter().enumerate() {
                    self.output.clear();
                    if j == ec.handler_body.len() - 1 {
                        // Last handler statement → implicit return
                        if let Statement::Expr(expr) = handler_stmt {
                            self.write_indent();
                            self.write("return ");
                            self.gen_expr(expr)?;
                            self.write(";\n");
                        } else {
                            self.gen_statement(handler_stmt)?;
                        }
                    } else {
                        self.gen_statement(handler_stmt)?;
                    }
                    result.push_str(&self.output);
                }
                self.indent -= 1;

                self.output.clear();
                self.writeln("}");
                result.push_str(&self.output);

                // All remaining statements were consumed by the try block
                return Ok(());
            }

            let is_last = i == stmts.len() - 1;
            self.output.clear();
            if is_last {
                // Last statement: implicit return
                if let Statement::Expr(expr) = &stmts[i] {
                    self.write_indent();
                    self.write("return ");
                    self.gen_expr(expr)?;
                    self.write(";\n");
                } else {
                    self.gen_statement(&stmts[i])?;
                }
            } else {
                self.gen_statement(&stmts[i])?;
            }
            result.push_str(&self.output);
            i += 1;
        }
        Ok(())
    }

    /// Generate a JSON schema expression for the JS runtime.
    /// Converts AST schema expressions to JS schema descriptors.
    fn gen_json_schema_expr(&mut self, expr: &Expr) -> Result<(), JsError> {
        match expr {
            Expr::Ident(name, _) => {
                match name.as_str() {
                    "Int" | "Str" | "Float" | "Bool" => {
                        // Primitive type: emit as string
                        self.write(&format!("'{}'", name));
                    }
                    _ => {
                        // TypeDef name: emit as string for runtime lookup
                        self.write(&format!("'{}'", name));
                    }
                }
                Ok(())
            }
            Expr::ListLit(items, _) => {
                // @[Schema] — list type
                self.write("{ __list: ");
                if let Some(item) = items.first() {
                    self.gen_json_schema_expr(item)?;
                } else {
                    self.write("'Str'");
                }
                self.write(" }");
                Ok(())
            }
            _ => {
                self.write("'Str'");
                Ok(())
            }
        }
    }

    fn gen_type_def(&mut self, type_def: &TypeDef) -> Result<(), JsError> {
        // B3: TypeDef → factory function + methods as prototype
        let non_method_fields: Vec<&FieldDef> =
            type_def.fields.iter().filter(|f| !f.is_method).collect();
        let field_names: Vec<&str> = non_method_fields.iter().map(|f| f.name.as_str()).collect();

        let methods: Vec<&FieldDef> = type_def.fields.iter().filter(|f| f.is_method).collect();

        // TypeDef factory function and methods are sync context
        let prev_async_context = self.in_async_context;
        self.in_async_context = false;

        self.write_indent();
        self.write(&format!("function {}(fields) {{\n", type_def.name));
        self.indent += 1;
        // Extract fields as local variables so methods can access them via closure
        for field in &non_method_fields {
            self.write_indent();
            self.write(&format!(
                "const {} = __taida_ensureNotNull(fields && fields.{}, ",
                field.name, field.name
            ));
            self.gen_field_default_expr(field)?;
            self.write(");\n");
        }
        self.writeln("const obj = {");
        self.indent += 1;
        self.writeln(&format!("__type: '{}',", type_def.name));
        for name in &field_names {
            self.writeln(&format!("{name},"));
        }
        // B3: Generate methods inline
        // QF-14: gen_func_body_to_buf を使って ErrorCeiling (|==) を正しく処理する
        for method_field in &methods {
            if let Some(ref func_def) = method_field.method_def {
                let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();
                self.write_indent();
                self.write(&format!(
                    "{}({}) {{\n",
                    method_field.name,
                    params.join(", ")
                ));
                let mut result = std::mem::take(&mut self.output);
                self.indent += 1;
                self.gen_func_body_to_buf(&func_def.body, &mut result)?;
                self.indent -= 1;
                self.output = result;
                self.writeln("},");
            }
        }
        self.indent -= 1;
        self.writeln("};");
        self.writeln("return Object.freeze(obj);");
        self.indent -= 1;
        self.writeln("}");

        // Register field names for InheritanceDef parent field resolution
        self.type_field_registry.insert(
            type_def.name.clone(),
            field_names.iter().map(|s| s.to_string()).collect(),
        );

        // Register TypeDef for JSON schema resolution
        if !non_method_fields.is_empty() {
            self.write_indent();
            self.write(&format!("__taida_registerTypeDef('{}', {{ ", type_def.name));
            for (i, f) in non_method_fields.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                let schema_str = Self::type_expr_to_schema(&f.type_annotation);
                self.write(&format!("{}: {}", f.name, schema_str));
            }
            self.write(" });\n");
        }
        self.writeln("");

        // Restore async context
        self.in_async_context = prev_async_context;
        Ok(())
    }

    fn gen_inheritance_def(&mut self, inh_def: &InheritanceDef) -> Result<(), JsError> {
        if let Some(parent_mold_fields) = self.mold_field_registry.get(&inh_def.parent).cloned() {
            let merged_fields = merge_field_defs(&parent_mold_fields, &inh_def.fields);
            let header_args = inh_def
                .child_args
                .as_ref()
                .or(inh_def.parent_args.as_ref())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let prev_async_context = self.in_async_context;
            self.in_async_context = false;
            self.gen_custom_mold_factory(&inh_def.child, header_args, &merged_fields)?;
            let all_fields: Vec<String> = merged_fields
                .iter()
                .filter(|field| !field.is_method)
                .map(|field| field.name.clone())
                .collect();
            self.type_field_registry
                .insert(inh_def.child.clone(), all_fields);
            self.mold_field_registry
                .insert(inh_def.child.clone(), merged_fields);

            self.in_async_context = prev_async_context;
            return Ok(());
        }

        // B6: Inheritance with prototype chain
        let child_fields: Vec<&FieldDef> = inh_def.fields.iter().filter(|f| !f.is_method).collect();
        let field_names: Vec<&str> = child_fields.iter().map(|f| f.name.as_str()).collect();

        let methods: Vec<&FieldDef> = inh_def.fields.iter().filter(|f| f.is_method).collect();

        // Collect parent field names from registry for closure variable extraction
        let parent_field_names: Vec<String> = self
            .type_field_registry
            .get(&inh_def.parent)
            .cloned()
            .unwrap_or_default();

        // InheritanceDef factory function and methods are sync context
        let prev_async_context = self.in_async_context;
        self.in_async_context = false;

        self.write_indent();
        self.write(&format!("function {}(fields) {{\n", inh_def.child));
        self.indent += 1;
        self.writeln(&format!("const parent = {}(fields);", inh_def.parent));
        // Extract parent fields as local variables so child methods can access them via closure
        for pf in &parent_field_names {
            self.writeln(&format!("const {pf} = parent.{pf};"));
        }
        // Extract child fields as local variables
        for field in &child_fields {
            self.write_indent();
            self.write(&format!(
                "const {} = __taida_ensureNotNull(fields && fields.{}, ",
                field.name, field.name
            ));
            self.gen_field_default_expr(field)?;
            self.write(");\n");
        }
        self.writeln("const obj = {");
        self.indent += 1;
        self.writeln("...parent,");
        self.writeln(&format!("__type: '{}',", inh_def.child));
        for name in &field_names {
            self.writeln(&format!("{name},"));
        }
        // Child methods override parent methods
        // QF-14: gen_func_body_to_buf を使って ErrorCeiling (|==) を正しく処理する
        for method_field in &methods {
            if let Some(ref func_def) = method_field.method_def {
                let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();
                self.write_indent();
                self.write(&format!(
                    "{}({}) {{\n",
                    method_field.name,
                    params.join(", ")
                ));
                let mut result = std::mem::take(&mut self.output);
                self.indent += 1;
                self.gen_func_body_to_buf(&func_def.body, &mut result)?;
                self.indent -= 1;
                self.output = result;
                self.writeln("},");
            }
        }
        self.indent -= 1;
        self.writeln("};");
        self.writeln("return Object.freeze(obj);");
        self.indent -= 1;
        self.writeln("}");

        // RCB-101: Register inheritance parent for error type filtering in |==
        self.writeln(&format!(
            "__taida_type_parents['{}'] = '{}';",
            inh_def.child, inh_def.parent
        ));

        // Register child type fields (parent fields + child fields) for further inheritance
        let mut all_fields: Vec<String> = parent_field_names;
        all_fields.extend(field_names.iter().map(|s| s.to_string()));
        self.type_field_registry
            .insert(inh_def.child.clone(), all_fields);
        self.writeln("");

        // Restore async context
        self.in_async_context = prev_async_context;
        Ok(())
    }

    fn gen_mold_def(&mut self, mold_def: &MoldDef) -> Result<(), JsError> {
        // MoldDef factory function and methods are sync context
        let prev_async_context = self.in_async_context;
        self.in_async_context = false;
        let header_args = mold_def.name_args.as_ref().unwrap_or(&mold_def.mold_args);
        self.gen_custom_mold_factory(&mold_def.name, header_args, &mold_def.fields)?;
        // Register fields for later use by inheritance lookups. This is populated
        // only when `gen_mold_def()` runs, so the registry depends on definition
        // order. The parser guarantees that base mold definitions precede derived
        // ones within the same file, which is the only ordering this registry
        // relies on.
        self.mold_field_registry
            .insert(mold_def.name.clone(), mold_def.fields.clone());

        // Restore async context
        self.in_async_context = prev_async_context;
        Ok(())
    }

    fn collect_mold_type_param_names(header_args: &[MoldHeaderArg]) -> Vec<String> {
        header_args
            .iter()
            .filter_map(|arg| match arg {
                MoldHeaderArg::TypeParam(tp) => Some(tp.name.clone()),
                MoldHeaderArg::Concrete(_) => None,
            })
            .collect()
    }

    fn gen_custom_mold_factory(
        &mut self,
        name: &str,
        header_args: &[MoldHeaderArg],
        fields: &[FieldDef],
    ) -> Result<(), JsError> {
        let type_params = Self::collect_mold_type_param_names(header_args);
        let non_method_fields: Vec<&FieldDef> = fields.iter().filter(|f| !f.is_method).collect();
        let required_fields: Vec<&FieldDef> = non_method_fields
            .iter()
            .copied()
            .filter(|f| f.name != "filling" && f.default_value.is_none())
            .collect();
        let optional_fields: Vec<&FieldDef> = non_method_fields
            .iter()
            .copied()
            .filter(|f| f.name != "filling" && f.default_value.is_some())
            .collect();
        let has_declared_filling = non_method_fields.iter().any(|f| f.name == "filling");
        let positional_params: Vec<String> = std::iter::once("filling".to_string())
            .chain(required_fields.iter().map(|f| f.name.clone()))
            .collect();
        let methods: Vec<&FieldDef> = fields.iter().filter(|f| f.is_method).collect();

        self.write_indent();
        self.write(&format!(
            "function {}({}, fields) {{\n",
            name,
            positional_params.join(", ")
        ));
        self.indent += 1;
        for field in &optional_fields {
            self.write_indent();
            self.write(&format!(
                "const {} = __taida_ensureNotNull(fields && fields.{}, ",
                field.name, field.name
            ));
            self.gen_field_default_expr(field)?;
            self.write(");\n");
        }
        self.writeln("const obj = {");
        self.indent += 1;
        self.writeln(&format!("__type: '{}',", name));
        // Map type parameters to their corresponding field bindings.
        // The first type param maps to `filling`, subsequent ones map to
        // required fields in order. If a mold declares more type params
        // than there are fields + filling, the excess params fall back to
        // `"undefined"` — this is a defensive default since the type checker
        // should reject such definitions before codegen runs.
        let type_arg_bindings: Vec<String> = std::iter::once("filling".to_string())
            .chain(required_fields.iter().map(|f| f.name.clone()))
            .collect();
        for (i, _tp) in type_params.iter().enumerate() {
            let binding = type_arg_bindings
                .get(i)
                .cloned()
                .unwrap_or_else(|| "undefined".to_string());
            self.writeln(&format!("__typeArg{}: {},", i, binding));
        }
        self.writeln("__value: filling,");
        if !has_declared_filling {
            self.writeln("filling,");
        }
        for field in &non_method_fields {
            if field.name == "filling" {
                continue;
            }
            self.writeln(&format!("{},", field.name));
        }
        self.writeln("unmold() { return this.__value; },");
        for method_field in &methods {
            if let Some(ref func_def) = method_field.method_def {
                let params: Vec<String> = func_def.params.iter().map(|p| p.name.clone()).collect();
                self.write_indent();
                self.write(&format!(
                    "{}({}) {{\n",
                    method_field.name,
                    params.join(", ")
                ));
                self.indent += 1;
                for (j, stmt) in func_def.body.iter().enumerate() {
                    if j == func_def.body.len() - 1 {
                        if let Statement::Expr(expr) = stmt {
                            self.write_indent();
                            self.write("return ");
                            self.gen_expr(expr)?;
                            self.write(";\n");
                        } else {
                            self.gen_statement(stmt)?;
                        }
                    } else {
                        self.gen_statement(stmt)?;
                    }
                }
                self.indent -= 1;
                self.writeln("},");
            }
        }
        self.indent -= 1;
        self.writeln("};");
        self.writeln("return Object.freeze(obj);");
        self.indent -= 1;
        self.writeln("}\n");
        Ok(())
    }

    fn gen_error_ceiling(&mut self, ec: &ErrorCeiling) -> Result<(), JsError> {
        // RCB-101: Extract handler type name for error type filtering
        let handler_type = match &ec.error_type {
            TypeExpr::Named(name) => name.as_str(),
            _ => "Error",
        };

        // Standalone ErrorCeiling — generate try/catch with empty try
        self.writeln("try {");
        self.indent += 1;
        self.indent -= 1;
        self.write_indent();
        self.write("} catch (__taida_caught_err) {\n");
        self.indent += 1;
        // RCB-101: Type filter
        self.writeln("const __taida_thrown_type = __taida_caught_err.type || (__taida_caught_err.__type) || 'Error';");
        self.writeln(&format!(
            "if (!__taida_is_error_subtype(__taida_thrown_type, '{}')) throw __taida_caught_err;",
            handler_type
        ));
        self.writeln(&format!("const {} = __taida_caught_err;", ec.error_param));
        for stmt in &ec.handler_body {
            self.gen_statement(stmt)?;
        }
        self.indent -= 1;
        self.writeln("}");
        Ok(())
    }

    /// RCB-201: Resolve the .td source path for a local import, for export validation.
    /// Returns None if the path cannot be determined (e.g., no source_file context).
    fn resolve_import_td_path(&self, import_path: &str) -> Option<std::path::PathBuf> {
        let source_file = self.source_file.as_ref()?;
        let source_dir = source_file.parent()?;
        let td_path = source_dir.join(import_path);
        if td_path.exists() {
            Some(td_path)
        } else {
            None
        }
    }

    /// RCB-201: Validate that all imported symbols are exported by the target module.
    /// Reads and parses the target .td file, checks for explicit `<<<` declarations,
    /// and returns an error if any imported symbol is not in the export list.
    fn validate_import_symbols(&self, import: &ImportStmt) -> Result<(), JsError> {
        // Skip core-bundled and npm packages — they don't have .td export declarations
        if import.path.starts_with("npm:")
            || import.path == "taida-lang/js"
            || import.path == "taida-lang/os"
            || import.path == "taida-lang/crypto"
            || import.path == "taida-lang/net"
            || import.path == "taida-lang/pool"
        {
            return Ok(());
        }

        // Resolve the .td source path
        let td_path = if import.path.starts_with("./")
            || import.path.starts_with("../")
            || import.path.starts_with('/')
        {
            self.resolve_import_td_path(&import.path)
        } else if import.path.contains('/') {
            // Package import — resolve via .taida/deps/
            let project_root = match self.project_root.as_ref() {
                Some(r) => r,
                None => return Ok(()), // No project root — skip validation
            };
            let resolution = if let Some(ref ver) = import.version {
                crate::pkg::resolver::resolve_package_module_versioned(
                    project_root,
                    &import.path,
                    ver,
                )
            } else {
                crate::pkg::resolver::resolve_package_module(project_root, &import.path)
            };
            resolution.and_then(|r| {
                let path = match &r.submodule {
                    Some(sub) => r.pkg_dir.join(format!("{}.td", sub)),
                    None => {
                        let entry = match crate::pkg::manifest::Manifest::from_dir(&r.pkg_dir) {
                            Ok(Some(manifest)) => manifest.entry,
                            _ => "main.td".to_string(),
                        };
                        r.pkg_dir.join(entry)
                    }
                };
                if path.exists() { Some(path) } else { None }
            })
        } else {
            None
        };

        let td_path = match td_path {
            Some(p) => p,
            None => return Ok(()), // Cannot resolve — let downstream handle the error
        };

        let source = match std::fs::read_to_string(&td_path) {
            Ok(s) => s,
            Err(_) => return Ok(()), // Cannot read — let downstream handle the error
        };
        let (program, _) = crate::parser::parse(&source);

        // Collect explicit export list from <<< statements
        let mut export_symbols: Vec<String> = Vec::new();
        let mut has_export = false;
        for stmt in &program.statements {
            if let crate::parser::Statement::Export(export_stmt) = stmt {
                has_export = true;
                for sym in &export_stmt.symbols {
                    if !export_symbols.contains(sym) {
                        export_symbols.push(sym.clone());
                    }
                }
            }
        }

        // If no <<< found, all symbols are exported (backward compat)
        if !has_export {
            return Ok(());
        }

        let exports: std::collections::HashSet<String> = export_symbols.into_iter().collect();

        // Validate each imported symbol
        for sym in &import.symbols {
            if !exports.contains(&sym.name) {
                return Err(JsError {
                    message: format!(
                        "Symbol '{}' not found in module '{}'. \
                         The module exports: {}",
                        sym.name,
                        import.path,
                        if exports.is_empty() {
                            "(nothing)".to_string()
                        } else {
                            let mut sorted: Vec<&String> = exports.iter().collect();
                            sorted.sort();
                            sorted
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                });
            }
        }

        Ok(())
    }

    fn gen_import(&mut self, import: &ImportStmt) -> Result<(), JsError> {
        // RCB-201: Validate imported symbols against target module's export list
        self.validate_import_symbols(import)?;

        // taida-lang/js: JSNew is a compile-time construct, no runtime import needed
        if import.path == "taida-lang/js" {
            return Ok(());
        }
        // taida-lang/os: core-bundled, runtime functions already embedded
        if import.path == "taida-lang/os" {
            return Ok(());
        }
        // taida-lang/crypto: core-bundled, runtime sha256 already embedded
        if import.path == "taida-lang/crypto" {
            return Ok(());
        }
        // taida-lang/net: core-bundled, HTTP v1 runtime functions already embedded
        if import.path == "taida-lang/net" {
            self.has_net_import = true;
            return Ok(());
        }

        let symbols: Vec<String> = import
            .symbols
            .iter()
            .map(|s| match &s.alias {
                Some(alias) => format!("{} as {}", s.name, alias),
                None => s.name.clone(),
            })
            .collect();

        self.write_indent();
        if import.path.starts_with("npm:") {
            // npm パッケージからのインポート
            let pkg_name = &import.path[4..];
            self.write(&format!(
                "import {{ {} }} from '{}';\n",
                symbols.join(", "),
                pkg_name
            ));
        } else if !import.path.starts_with("./")
            && !import.path.starts_with("../")
            && !import.path.starts_with('/')
            && import.path.contains('/')
        {
            // Package import (e.g. "shijimic/taida-package-test")
            // Resolve via .taida/deps/ and packages.tdm entry point.
            // RC-1q: Pass version for versioned imports (>>> alice/http@b.12)
            // to resolve version-qualified directories first.
            let js_path =
                self.resolve_package_import_path(&import.path, import.version.as_deref())?;
            self.write(&format!(
                "import {{ {} }} from '{}';\n",
                symbols.join(", "),
                js_path
            ));
        } else {
            // ローカルモジュール — ESM import (.mjs)
            let js_path = self.resolve_local_import_js_path(&import.path)?;
            self.write(&format!(
                "import {{ {} }} from '{}';\n",
                symbols.join(", "),
                js_path
            ));
        }
        Ok(())
    }

    /// Resolve a local (relative) import path to the correct .mjs ESM specifier.
    ///
    /// When build context (entry_root, out_root) is available, computes the actual
    /// output location of the dependency using the same strip_prefix logic as main.rs,
    /// then produces a relative ESM import from our output to the dependency's output.
    /// This handles the case where `../shared` from `src/main.td` is flattened to
    /// `out_root/shared.mjs` alongside `out_root/main.mjs` (correct: `./shared.mjs`).
    ///
    /// Also performs RCB-303 path traversal rejection for `..` imports.
    fn resolve_local_import_js_path(&self, import_path: &str) -> Result<String, JsError> {
        use std::path::{Path, PathBuf};

        // RCB-303: Check path traversal for `..` imports
        if import_path.contains("..")
            && let (Some(project_root), Some(source_file)) = (&self.project_root, &self.source_file)
        {
            let source_dir = source_file.parent().unwrap_or(Path::new("."));
            let td_path = source_dir.join(import_path);
            let reject = if let Ok(canonical) = td_path.canonicalize() {
                if let Ok(root_canonical) = project_root.canonicalize() {
                    !canonical.starts_with(&root_canonical)
                } else {
                    false // cannot verify root — let it through, will fail at read
                }
            } else {
                false // file not found — let it through, will fail at read
            };
            if reject {
                return Err(JsError {
                    message: format!(
                        "Import path '{}' resolves outside the project root. Path traversal is not allowed.",
                        import_path
                    ),
                });
            }
        }

        // When build context is available and the import crosses directory boundaries,
        // compute the correct ESM path based on output layout.
        if let (Some(entry_root), Some(out_root), Some(source_file), Some(output_file)) = (
            &self.entry_root,
            &self.out_root,
            &self.source_file,
            &self.output_file,
        ) {
            let source_dir = source_file.parent().unwrap_or(Path::new("."));
            let dep_source = source_dir.join(import_path);
            // Canonicalize to resolve symlinks and ..
            let dep_source = dep_source.canonicalize().unwrap_or(dep_source);

            // Replicate main.rs output placement: strip_prefix chain
            let dep_rel = dep_source
                .strip_prefix(entry_root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| {
                    let entry_parent = entry_root.parent().unwrap_or(entry_root);
                    dep_source
                        .strip_prefix(entry_parent)
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|_| {
                            PathBuf::from(
                                dep_source
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("module.td"),
                            )
                        })
                });
            let dep_output = out_root.join(dep_rel.with_extension("mjs"));

            // Compute relative ESM path from our output to the dependency's output
            let our_dir = output_file.parent().unwrap_or(Path::new("."));
            if let Some(rel) = pathdiff(our_dir, &dep_output) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                // ESM requires explicit ./ prefix for same-directory imports
                let esm_path = if rel_str.starts_with("../") || rel_str.starts_with("./") {
                    rel_str.to_string()
                } else {
                    format!("./{}", rel_str)
                };
                return Ok(esm_path);
            }
        }

        // Fallback: simple string replacement (no build context).
        // Parser enforces .td extension on relative imports, so we only need
        // the .td/.tdjs → .mjs conversion.
        let js_path = import_path.replace(".tdjs", ".mjs").replace(".td", ".mjs");
        Ok(js_path)
    }

    /// Resolve a package import path to a relative .mjs path for ESM import.
    ///
    /// Given "shijimic/taida-package-test", finds `.taida/deps/shijimic/taida-package-test/`,
    /// reads packages.tdm for entry point, and returns a relative path from the JS output
    /// to the package's .mjs file (transpiled in-place in .taida/deps/).
    ///
    /// RC-1q: When `version` is provided, tries version-qualified directories first
    /// (e.g., `.taida/deps/alice/http@b.12/`) before falling back to unversioned.
    fn resolve_package_import_path(
        &self,
        import_path: &str,
        version: Option<&str>,
    ) -> Result<String, JsError> {
        let project_root = self.project_root.as_ref().ok_or_else(|| JsError {
            message: format!(
                "Could not resolve package import '{}': project root context is unavailable.",
                import_path
            ),
        })?;
        let _source_file = self.source_file.as_ref().ok_or_else(|| JsError {
            message: format!(
                "Could not resolve package import '{}': source file context is unavailable.",
                import_path
            ),
        })?;

        // Find the package directory using longest-prefix matching.
        // RCB-213: For versioned imports, use resolve_package_module_versioned
        // which does longest-prefix matching with version-qualified directories.
        // This supports submodule imports (e.g., alice/pkg/submod@b.12 resolves to
        // .taida/deps/alice/pkg@b.12/submod.td).
        let resolution = if let Some(ver) = version {
            crate::pkg::resolver::resolve_package_module_versioned(
                project_root,
                import_path,
                ver,
            )
            .ok_or_else(|| JsError {
                message: format!(
                    "Could not resolve package import '{}@{}'. Run `taida deps` and ensure the package is installed in .taida/deps/ before building JS.",
                    import_path, ver
                ),
            })?
        } else {
            crate::pkg::resolver::resolve_package_module(project_root, import_path)
                .ok_or_else(|| JsError {
                    message: format!(
                        "Could not resolve package import '{}'. Run `taida deps` and ensure the package is installed in .taida/deps/ before building JS.",
                        import_path
                    ),
                })?
        };

        // Determine the target .td file
        let td_path = match &resolution.submodule {
            Some(submodule) => resolution.pkg_dir.join(format!("{}.td", submodule)),
            None => {
                // Read packages.tdm for entry point.
                // If the manifest is missing or unreadable, fall back to "main.td"
                // which is the conventional default entry point for Taida packages.
                let entry = match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir) {
                    Ok(Some(manifest)) => manifest.entry,
                    Ok(None) => {
                        eprintln!(
                            "Warning: no packages.tdm found in '{}', using 'main.td' as entry point",
                            resolution.pkg_dir.display()
                        );
                        "main.td".to_string()
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: failed to read packages.tdm in '{}': {}, using 'main.td' as entry point",
                            resolution.pkg_dir.display(),
                            e
                        );
                        "main.td".to_string()
                    }
                };
                let entry_clean = entry.strip_prefix("./").unwrap_or(&entry);
                resolution.pkg_dir.join(entry_clean)
            }
        };

        // The dep .mjs is in-place next to the .td file in .taida/deps/
        let mjs_path = td_path.with_extension("mjs");

        let output_file = self.output_file.as_ref().ok_or_else(|| JsError {
            message: format!(
                "Could not resolve package import '{}': JS output path is unavailable.",
                import_path
            ),
        })?;
        let js_output_dir = output_file
            .parent()
            .ok_or_else(|| JsError {
                message: format!(
                    "Could not resolve package import '{}': could not determine JS output directory.",
                    import_path
                ),
            })?
            .to_path_buf();

        let rel = pathdiff(&js_output_dir, &mjs_path).ok_or_else(|| JsError {
            message: format!(
                "Could not resolve package import '{}': failed to compute relative JS import path.",
                import_path
            ),
        })?;

        // Ensure it starts with "./" for ESM
        let rel_str = rel.to_string_lossy().to_string();
        if rel_str.starts_with("./") || rel_str.starts_with("../") {
            Ok(rel_str)
        } else {
            Ok(format!("./{}", rel_str))
        }
    }

    fn gen_export(&mut self, export: &ExportStmt) -> Result<(), JsError> {
        // RCB-212: Re-export path `<<< ./path` is not supported.
        if export.path.is_some() {
            return Err(JsError {
                message: "Re-export with path (`<<< ./path`) is not yet supported. \
                         Use explicit import and re-export instead."
                    .to_string(),
            });
        }
        // ESM named export
        self.write_indent();
        self.write("export { ");
        self.write(&export.symbols.join(", "));
        self.write(" };\n");
        Ok(())
    }

    fn gen_todo_default_expr(&mut self, arg: &Expr) -> Result<(), JsError> {
        match arg {
            Expr::Ident(name, _) => match name.as_str() {
                "Int" | "Num" => self.write("0"),
                "Float" => self.write("0.0"),
                "Str" => self.write("\"\""),
                "Bool" => self.write("false"),
                "Molten" => self.write("__taida_molten()"),
                _ => self.write("Object.freeze({})"),
            },
            Expr::MoldInst(name, type_args, _, _) if name == "Stub" => {
                if type_args.len() != 1 {
                    return Err(JsError {
                        message: "Stub requires exactly 1 message argument: Stub[\"msg\"]"
                            .to_string(),
                    });
                }
                self.write("__taida_molten()");
            }
            _ => self.write("Object.freeze({})"),
        }
        Ok(())
    }

    fn gen_expr(&mut self, expr: &Expr) -> Result<(), JsError> {
        match expr {
            Expr::IntLit(val, _) => {
                self.write(&val.to_string());
                Ok(())
            }
            Expr::FloatLit(val, _) => {
                self.write(&val.to_string());
                Ok(())
            }
            Expr::StringLit(val, _) => {
                let escaped = val
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t");
                self.write(&format!("\"{}\"", escaped));
                Ok(())
            }
            Expr::TemplateLit(val, _) => {
                // テンプレートリテラル → JS テンプレートリテラル
                // Taida uses ${var} syntax, same as JS — pass through directly
                // But @[...] list literals inside ${} need conversion to JS arrays
                let converted = Self::convert_template_list_literals(val);
                self.write(&format!("`{}`", converted));
                Ok(())
            }
            Expr::BoolLit(val, _) => {
                self.write(if *val { "true" } else { "false" });
                Ok(())
            }
            Expr::Gorilla(_) => {
                self.write("process.exit(1)");
                Ok(())
            }
            Expr::Ident(name, _) => {
                self.write(name);
                Ok(())
            }
            Expr::Placeholder(_) => {
                self.write("_");
                Ok(())
            }
            Expr::Hole(_) => {
                // Hole should not appear outside of FuncCall partial application context
                self.write("undefined");
                Ok(())
            }
            Expr::BuchiPack(fields, _) => {
                // QF-16: Placeholder 値のフィールドをスキップ（=> :Type が Placeholder として
                // パースされるため、BuchiPack 内ラムダの戻り値型注釈が不正なフィールドになる）
                let real_fields: Vec<_> = fields
                    .iter()
                    .filter(|f| !matches!(f.value, Expr::Placeholder(_)))
                    .collect();
                self.write("Object.freeze({ ");
                for (i, field) in real_fields.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(&format!("{}: ", field.name));
                    self.gen_expr(&field.value)?;
                }
                self.write(" })");
                Ok(())
            }
            Expr::ListLit(items, _) => {
                self.write("Object.freeze([");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.gen_expr(item)?;
                }
                self.write("])");
                Ok(())
            }
            Expr::BinaryOp(lhs, op, rhs, _) => {
                // Eq/NotEq use __taida_equals for structural comparison
                match op {
                    BinOp::Eq => {
                        self.write("__taida_equals(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    BinOp::NotEq => {
                        self.write("!__taida_equals(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    BinOp::Add => {
                        self.write("__taida_add(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    BinOp::Sub => {
                        self.write("__taida_sub(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    BinOp::Mul => {
                        self.write("__taida_mul(");
                        self.gen_expr(lhs)?;
                        self.write(", ");
                        self.gen_expr(rhs)?;
                        self.write(")");
                        return Ok(());
                    }
                    _ => {}
                }
                self.write("(");
                self.gen_expr(lhs)?;
                let op_str = match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul => unreachable!(),
                    // BinOp::Div and BinOp::Mod removed — use Div[x, y]() and Mod[x, y]() molds
                    BinOp::Eq | BinOp::NotEq => unreachable!(),
                    BinOp::Lt => " < ",
                    BinOp::Gt => " > ",
                    BinOp::GtEq => " >= ",
                    BinOp::And => " && ",
                    BinOp::Or => " || ",
                    BinOp::Concat => " + ",
                };
                self.write(op_str);
                self.gen_expr(rhs)?;
                self.write(")");
                Ok(())
            }
            Expr::UnaryOp(op, operand, _) => {
                let op_str = match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "!",
                };
                self.write(op_str);
                self.gen_expr(operand)?;
                Ok(())
            }
            Expr::FuncCall(callee, args, _) => {
                // TCO: if calling a function in the current TCO group, emit TailCall
                if !self.current_tco_funcs.is_empty()
                    && let Expr::Ident(name, _) = callee.as_ref()
                    && self.current_tco_funcs.contains(name)
                {
                    self.write(&format!("new __TaidaTailCall(__inner_{}, [", name));
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.gen_expr(arg)?;
                    }
                    self.write("])");
                    return Ok(());
                }

                // Empty-slot partial application: if any arg is Hole (empty slot), emit a closure.
                // Note: Old `_` (Placeholder) partial application is rejected by checker
                // (E1502) before reaching codegen. Only Hole-based syntax `f(5, )` is handled.
                let has_hole = args.iter().any(|a| matches!(a, Expr::Hole(_)));
                if has_hole {
                    // Count holes and generate parameter names
                    let placeholder_count =
                        args.iter().filter(|a| matches!(a, Expr::Hole(_))).count();
                    let params: Vec<String> = (0..placeholder_count)
                        .map(|i| format!("__pa_{}", i))
                        .collect();
                    self.write(&format!("(({}) => ", params.join(", ")));

                    // Generate the function call with placeholders replaced
                    if let Expr::Ident(name, _) = callee.as_ref() {
                        match name.as_str() {
                            "debug" => self.write("__taida_debug"),
                            "typeof" => self.write("__taida_typeof"),
                            "assert" => self.write("__taida_assert"),
                            "stdout" => self.write("__taida_stdout"),
                            "stderr" => self.write("__taida_stderr"),
                            "stdin" => self.write("__taida_stdin"),
                            "jsonEncode" => self.write("__taida_jsonEncode"),
                            "jsonPretty" => self.write("__taida_jsonPretty"),
                            "nowMs" => self.write("__taida_nowMs"),
                            "sleep" => self.write("__taida_sleep"),
                            "readBytes" => self.write("__taida_os_readBytes"),
                            "writeFile" => self.write("__taida_os_writeFile"),
                            "writeBytes" => self.write("__taida_os_writeBytes"),
                            "appendFile" => self.write("__taida_os_appendFile"),
                            "remove" => self.write("__taida_os_remove"),
                            "createDir" => self.write("__taida_os_createDir"),
                            "rename" => self.write("__taida_os_rename"),
                            "run" => self.write("__taida_os_run"),
                            "execShell" => self.write("__taida_os_execShell"),
                            "allEnv" => self.write("__taida_os_allEnv"),
                            "argv" => self.write("__taida_os_argv"),
                            "tcpConnect" => self.write("__taida_os_tcpConnect"),
                            "tcpListen" => self.write("__taida_os_tcpListen"),
                            "tcpAccept" => self.write("__taida_os_tcpAccept"),
                            "socketSend" => self.write("__taida_os_socketSend"),
                            "socketSendAll" => self.write("__taida_os_socketSendAll"),
                            "socketRecv" => self.write("__taida_os_socketRecv"),
                            "socketSendBytes" => self.write("__taida_os_socketSendBytes"),
                            "socketRecvBytes" => self.write("__taida_os_socketRecvBytes"),
                            "socketClose" => self.write("__taida_os_socketClose"),
                            "listenerClose" => self.write("__taida_os_listenerClose"),
                            "udpBind" => self.write("__taida_os_udpBind"),
                            "udpSendTo" => self.write("__taida_os_udpSendTo"),
                            "udpRecvFrom" => self.write("__taida_os_udpRecvFrom"),
                            "udpClose" => self.write("__taida_os_udpClose"),
                            "socketRecvExact" => self.write("__taida_os_socketRecvExact"),
                            "dnsResolve" => self.write("__taida_os_dnsResolve"),
                            "poolCreate" => self.write("__taida_os_poolCreate"),
                            "poolAcquire" => self.write("__taida_os_poolAcquire"),
                            "poolRelease" => self.write("__taida_os_poolRelease"),
                            "poolClose" => self.write("__taida_os_poolClose"),
                            "poolHealth" => self.write("__taida_os_poolHealth"),
                            // taida-lang/net HTTP v1 (only when imported)
                            _ if self.try_write_net_builtin(name, "") => {}
                            _ => self.gen_expr(callee)?,
                        }
                    } else {
                        self.gen_expr(callee)?;
                    }
                    self.write("(");
                    let mut ph_idx = 0;
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        if matches!(arg, Expr::Hole(_)) {
                            self.write(&format!("__pa_{}", ph_idx));
                            ph_idx += 1;
                        } else {
                            self.gen_expr(arg)?;
                        }
                    }
                    self.write("))");
                    return Ok(());
                }

                if let Expr::Ident(name, _) = callee.as_ref() {
                    match name.as_str() {
                        "debug" => self.write("__taida_debug"),
                        "typeof" => self.write("__taida_typeof"),
                        "assert" => self.write("__taida_assert"),
                        "stdout" => self.write("__taida_stdout"),
                        "stderr" => self.write("__taida_stderr"),
                        "stdin" => self.write("__taida_stdin"),
                        "jsonEncode" => self.write("__taida_jsonEncode"),
                        "jsonPretty" => self.write("__taida_jsonPretty"),
                        "nowMs" => self.write("__taida_nowMs"),
                        "sleep" => self.write("__taida_sleep"),
                        "readBytes" => self.write("__taida_os_readBytes"),
                        "writeFile" => self.write("__taida_os_writeFile"),
                        "writeBytes" => self.write("__taida_os_writeBytes"),
                        "appendFile" => self.write("__taida_os_appendFile"),
                        "remove" => self.write("__taida_os_remove"),
                        "createDir" => self.write("__taida_os_createDir"),
                        "rename" => self.write("__taida_os_rename"),
                        "run" => self.write("__taida_os_run"),
                        "execShell" => self.write("__taida_os_execShell"),
                        "allEnv" => self.write("__taida_os_allEnv"),
                        "argv" => self.write("__taida_os_argv"),
                        "tcpConnect" => self.write("__taida_os_tcpConnect"),
                        "tcpListen" => self.write("__taida_os_tcpListen"),
                        "tcpAccept" => self.write("__taida_os_tcpAccept"),
                        "socketSend" => self.write("__taida_os_socketSend"),
                        "socketSendAll" => self.write("__taida_os_socketSendAll"),
                        "socketRecv" => self.write("__taida_os_socketRecv"),
                        "socketSendBytes" => self.write("__taida_os_socketSendBytes"),
                        "socketRecvBytes" => self.write("__taida_os_socketRecvBytes"),
                        "socketClose" => self.write("__taida_os_socketClose"),
                        "listenerClose" => self.write("__taida_os_listenerClose"),
                        "udpBind" => self.write("__taida_os_udpBind"),
                        "udpSendTo" => self.write("__taida_os_udpSendTo"),
                        "udpRecvFrom" => self.write("__taida_os_udpRecvFrom"),
                        "udpClose" => self.write("__taida_os_udpClose"),
                        "socketRecvExact" => self.write("__taida_os_socketRecvExact"),
                        "dnsResolve" => self.write("__taida_os_dnsResolve"),
                        "poolCreate" => self.write("__taida_os_poolCreate"),
                        "poolAcquire" => self.write("__taida_os_poolAcquire"),
                        "poolRelease" => self.write("__taida_os_poolRelease"),
                        "poolClose" => self.write("__taida_os_poolClose"),
                        "poolHealth" => self.write("__taida_os_poolHealth"),
                        // taida-lang/net HTTP v1 (only when imported)
                        _ if self.try_write_net_builtin(name, "") => {}
                        _ => self.gen_expr(callee)?,
                    }
                } else {
                    self.gen_expr(callee)?;
                }
                self.write("(");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.gen_expr(arg)?;
                }
                self.write(")");
                Ok(())
            }
            Expr::MethodCall(obj, method, args, _) => {
                if method == "throw" {
                    // .throw() is emitted as a standalone function call to avoid
                    // polluting Object.prototype.
                    self.write("__taida_throw(");
                    self.gen_expr(obj)?;
                    self.write(")");
                    return Ok(());
                }
                if is_removed_list_method(method) {
                    self.write("__taida_list_method_removed(");
                    self.write(&format!("{:?}", method));
                    self.write(")");
                    return Ok(());
                }
                // hasValue() is a method call on Lax — emit as method call
                // (In new design, hasValue() is always a function, not a property)
                self.gen_expr(obj)?;
                // Taida .length() is a method call, but JS .length is a property.
                // Use .length_() which is patched in the runtime.
                // Only state-check methods remain as prototype methods.
                // Operation methods are now standalone mold functions.
                let js_method = match method.as_str() {
                    "length" => "length_",
                    other => other,
                };
                self.write(&format!(".{}(", js_method));
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.gen_expr(arg)?;
                }
                self.write(")");
                Ok(())
            }
            Expr::FieldAccess(obj, field, _) => {
                self.gen_expr(obj)?;
                // F-59 fix: Lax/Gorillax hasValue is a callable function in JS runtime.
                // When accessed as a property (field access), emit as function call
                // so that it returns the boolean value instead of a function reference.
                if field == "hasValue" {
                    self.write(".hasValue()");
                } else {
                    self.write(&format!(".{}", field));
                }
                Ok(())
            }
            // IndexAccess removed in v0.5.0 — use .get(i) instead
            Expr::CondBranch(arms, _) => self.gen_cond_branch(arms),
            Expr::Pipeline(exprs, _) => self.gen_pipeline(exprs),
            Expr::TypeInst(name, fields, _) => {
                self.write(&format!("{}({{ ", name));
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.write(&format!("{}: ", field.name));
                    self.gen_expr(&field.value)?;
                }
                self.write(" })");
                Ok(())
            }
            Expr::MoldInst(name, type_args, fields, _) => {
                // B5: MoldInst → function call with type args

                // JSNew[ClassName](...) → new ClassName(...)
                if name == "JSNew" {
                    if type_args.is_empty() {
                        return Err(JsError {
                            message: "JSNew requires a type argument: JSNew[ClassName](...)"
                                .to_string(),
                        });
                    }
                    // Extract class name from first type arg (must be an identifier)
                    let class_name = match &type_args[0] {
                        Expr::Ident(n, _) => n.clone(),
                        _ => {
                            return Err(JsError {
                                message: "JSNew type argument must be an identifier (class name)"
                                    .to_string(),
                            });
                        }
                    };
                    self.write(&format!("new {}(", class_name));
                    // Emit constructor arguments from fields (positional args)
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.gen_expr(&field.value)?;
                    }
                    self.write(")");
                    return Ok(());
                }

                // JSSet[obj, key, value]() → ((o) => { o[key] = value; return o; })(obj)
                if name == "JSSet" {
                    if type_args.len() < 3 {
                        return Err(JsError {
                            message: "JSSet requires 3 type arguments: JSSet[obj, key, value]()"
                                .to_string(),
                        });
                    }
                    self.write("((__o) => { __o[");
                    self.gen_expr(&type_args[1])?;
                    self.write("] = ");
                    self.gen_expr(&type_args[2])?;
                    self.write("; return __o; })(");
                    self.gen_expr(&type_args[0])?;
                    self.write(")");
                    return Ok(());
                }

                // JSBind[obj, method]() → obj[method].bind(obj)
                if name == "JSBind" {
                    if type_args.len() < 2 {
                        return Err(JsError {
                            message: "JSBind requires 2 type arguments: JSBind[obj, method]()"
                                .to_string(),
                        });
                    }
                    self.write("((__o) => __o[");
                    self.gen_expr(&type_args[1])?;
                    self.write("].bind(__o))(");
                    self.gen_expr(&type_args[0])?;
                    self.write(")");
                    return Ok(());
                }

                // JSSpread[target, source]() → __taida_js_spread(target, source)
                if name == "JSSpread" {
                    if type_args.len() < 2 {
                        return Err(JsError {
                            message:
                                "JSSpread requires 2 type arguments: JSSpread[target, source]()"
                                    .to_string(),
                        });
                    }
                    self.write("__taida_js_spread(");
                    self.gen_expr(&type_args[0])?;
                    self.write(", ");
                    self.gen_expr(&type_args[1])?;
                    self.write(")");
                    return Ok(());
                }

                // taida-lang/os input molds: Read, ListDir, Stat, Exists, EnvVar
                if name == "Read"
                    || name == "ListDir"
                    || name == "Stat"
                    || name == "Exists"
                    || name == "EnvVar"
                {
                    let func_name = if name == "Read" {
                        "__taida_os_read"
                    } else if name == "ListDir" {
                        "__taida_os_listdir"
                    } else if name == "Stat" {
                        "__taida_os_stat"
                    } else if name == "Exists" {
                        "__taida_os_exists"
                    } else {
                        "__taida_os_envvar"
                    };
                    self.write(func_name);
                    self.write("(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }

                // taida-lang/os async input molds: ReadAsync, HttpGet, HttpPost, HttpRequest
                if name == "ReadAsync" {
                    self.write("__taida_os_readAsync(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                if name == "HttpGet" {
                    self.write("__taida_os_httpGet(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                if name == "HttpPost" {
                    self.write("__taida_os_httpPost(");
                    if type_args.len() >= 2 {
                        self.gen_expr(&type_args[0])?;
                        self.write(", ");
                        self.gen_expr(&type_args[1])?;
                    } else if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                        self.write(", ''");
                    }
                    self.write(")");
                    return Ok(());
                }
                if name == "HttpRequest" {
                    self.write("__taida_os_httpRequest(");
                    if type_args.len() >= 2 {
                        self.gen_expr(&type_args[0])?;
                        self.write(", ");
                        self.gen_expr(&type_args[1])?;
                    }
                    // Pass headers and body from optional fields
                    let mut has_headers = false;
                    let mut has_body = false;
                    for field in fields {
                        if field.name == "headers" {
                            self.write(", ");
                            self.gen_expr(&field.value)?;
                            has_headers = true;
                        }
                    }
                    if !has_headers {
                        self.write(", null");
                    }
                    for field in fields {
                        if field.name == "body" {
                            self.write(", ");
                            self.gen_expr(&field.value)?;
                            has_body = true;
                        }
                    }
                    if !has_body {
                        self.write(", null");
                    }
                    self.write(")");
                    return Ok(());
                }

                if name == "JSON" {
                    // JSON[raw, Schema]() — pass raw and schema name
                    self.write("JSON_mold(");
                    if type_args.len() >= 2 {
                        self.gen_expr(&type_args[0])?;
                        self.write(", ");
                        // Schema is a type name — emit as string for runtime lookup
                        self.gen_json_schema_expr(&type_args[1])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // Str[x](), Int[x](base?), Float[x](), Bool[x](), Bytes[x](), UInt8[x](),
                // Char[x](), CodePoint[x](), Utf8Encode[x](), Utf8Decode[x]() conversion molds
                if (name == "Str"
                    || name == "Int"
                    || name == "Float"
                    || name == "Bool"
                    || name == "Bytes"
                    || name == "UInt8"
                    || name == "Char"
                    || name == "CodePoint"
                    || name == "Utf8Encode"
                    || name == "Utf8Decode"
                    || name == "U16BE"
                    || name == "U16LE"
                    || name == "U32BE"
                    || name == "U32LE"
                    || name == "U16BEDecode"
                    || name == "U16LEDecode"
                    || name == "U32BEDecode"
                    || name == "U32LEDecode"
                    || name == "BytesCursor"
                    || name == "BytesCursorU8"
                    || name == "Cancel")
                    && !type_args.is_empty()
                {
                    self.write(&format!("{}_mold(", name));
                    self.gen_expr(&type_args[0])?;
                    if name == "Int" && type_args.len() >= 2 {
                        self.write(", ");
                        self.gen_expr(&type_args[1])?;
                    } else if name == "Bytes" && !fields.is_empty() {
                        self.write(", { ");
                        for (i, field) in fields.iter().enumerate() {
                            if i > 0 {
                                self.write(", ");
                            }
                            self.write(&format!("{}: ", field.name));
                            self.gen_expr(&field.value)?;
                        }
                        self.write(" }");
                    }
                    self.write(")");
                    return Ok(());
                }
                // BytesCursorTake[cursor, size]() — 2 type args
                if name == "BytesCursorTake" && type_args.len() >= 2 {
                    self.write("BytesCursorTake_mold(");
                    self.gen_expr(&type_args[0])?;
                    self.write(", ");
                    self.gen_expr(&type_args[1])?;
                    self.write(")");
                    return Ok(());
                }
                // BytesCursorRemaining[cursor]() — 1 type arg, returns Int (not Lax)
                if name == "BytesCursorRemaining" && !type_args.is_empty() {
                    self.write("BytesCursorRemaining_mold(");
                    self.gen_expr(&type_args[0])?;
                    self.write(")");
                    return Ok(());
                }
                // Cage[value, fn]() → Cage_mold(value, fn)
                if name == "Cage" {
                    self.write("Cage_mold(");
                    for (i, arg) in type_args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.gen_expr(arg)?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // Gorillax[value]() → Gorillax(value)
                if name == "Gorillax" {
                    self.write("Gorillax(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // Stub["msg"]() -> __taida_stub("msg")
                if name == "Stub" {
                    if !fields.is_empty() {
                        return Err(JsError {
                            message: "Stub does not take `()` fields. Use Stub[\"msg\"]"
                                .to_string(),
                        });
                    }
                    if type_args.len() != 1 {
                        return Err(JsError {
                            message: "Stub requires exactly 1 message argument: Stub[\"msg\"]"
                                .to_string(),
                        });
                    }
                    self.write("__taida_stub(");
                    self.gen_expr(&type_args[0])?;
                    self.write(")");
                    return Ok(());
                }
                // TODO[T](id <= ..., task <= ..., sol <= ..., unm <= ...)
                // The runtime uses `__type: 'TODO'` as the discriminant marker,
                // matching the mold name in source. This is consistent with how
                // other mold types (Lax, Result, etc.) use their name as `__type`.
                if name == "TODO" {
                    self.write("__taida_todo_mold(");
                    if let Some(arg0) = type_args.first() {
                        self.gen_todo_default_expr(arg0)?;
                    } else {
                        self.write("Object.freeze({})");
                    }
                    self.write(", { ");
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.write(&format!("{}: ", field.name));
                        self.gen_expr(&field.value)?;
                    }
                    self.write(" })");
                    return Ok(());
                }
                // Molten[]() → __taida_molten()
                if name == "Molten" {
                    if !type_args.is_empty() {
                        return Err(JsError {
                            message: "Molten takes no type arguments: Molten[]()".to_string(),
                        });
                    }
                    self.write("__taida_molten()");
                    return Ok(());
                }
                // Stream[value]() → Stream_mold(value)
                if name == "Stream" {
                    self.write("Stream_mold(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // StreamFrom[list]() → StreamFrom(list)
                if name == "StreamFrom" {
                    self.write("StreamFrom(");
                    if !type_args.is_empty() {
                        self.gen_expr(&type_args[0])?;
                    }
                    self.write(")");
                    return Ok(());
                }
                // Div[x, y]() and Mod[x, y]() molds → Div_mold(x, y, opts)
                if name == "Div" || name == "Mod" {
                    self.write(&format!("{}_mold(", name));
                    for (i, arg) in type_args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.gen_expr(arg)?;
                    }
                    // Check if any type arg is a FloatLit — JS Number.isInteger(2.0) is true,
                    // so we pass a __floatHint flag to preserve Taida's float semantics.
                    let has_float_arg = type_args.iter().any(|a| matches!(a, Expr::FloatLit(..)));
                    if !fields.is_empty() || has_float_arg {
                        self.write(", { ");
                        let mut wrote = false;
                        for (i, field) in fields.iter().enumerate() {
                            if i > 0 {
                                self.write(", ");
                            }
                            self.write(&format!("{}: ", field.name));
                            self.gen_expr(&field.value)?;
                            wrote = true;
                        }
                        if has_float_arg {
                            if wrote {
                                self.write(", ");
                            }
                            self.write("__floatHint: true");
                        }
                        self.write(" }");
                    }
                    self.write(")");
                    return Ok(());
                }
                self.write("__taida_solidify(");
                self.write(&format!("{}(", name));
                for (i, arg) in type_args.iter().enumerate() {
                    if i > 0 {
                        self.write(", ");
                    }
                    self.gen_expr(arg)?;
                }
                if !fields.is_empty() {
                    if !type_args.is_empty() {
                        self.write(", ");
                    }
                    self.write("{ ");
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        self.write(&format!("{}: ", field.name));
                        self.gen_expr(&field.value)?;
                    }
                    self.write(" }");
                } else if type_args.is_empty() {
                    // No args at all
                }
                self.write(")");
                self.write(")");
                Ok(())
            }
            Expr::Unmold(inner, _) => {
                let (await_prefix, unmold_fn) = if self.in_async_context {
                    ("await ", "__taida_unmold_async")
                } else {
                    ("", "__taida_unmold")
                };
                self.write(&format!("{await_prefix}{unmold_fn}("));
                self.gen_expr(inner)?;
                self.write(")");
                Ok(())
            }
            Expr::Lambda(params, body, _) => {
                let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                // Scope-aware net builtin shadowing: snapshot/restore for lambda scope
                let prev_shadowed_net = self.shadowed_net_builtins.clone();
                for p in params {
                    if Self::NET_BUILTIN_NAMES.contains(&p.name.as_str()) {
                        self.shadowed_net_builtins.insert(p.name.clone());
                    }
                }
                self.write(&format!("(({}) => ", param_names.join(", ")));
                self.gen_expr(body)?;
                self.write(")");
                self.shadowed_net_builtins = prev_shadowed_net;
                Ok(())
            }
            Expr::Throw(inner, _) => {
                self.write("(() => { throw ");
                self.gen_expr(inner)?;
                self.write("; })()");
                Ok(())
            }
        }
    }

    fn gen_cond_branch(&mut self, arms: &[crate::parser::CondArm]) -> Result<(), JsError> {
        self.write("(() => {\n");
        self.indent += 1;

        for (i, arm) in arms.iter().enumerate() {
            match &arm.condition {
                Some(cond) => {
                    self.write_indent();
                    if i == 0 {
                        self.write("if (");
                    } else {
                        self.write("else if (");
                    }
                    self.gen_expr(cond)?;
                    self.write(") {\n");
                    self.indent += 1;
                    self.gen_cond_arm_body(&arm.body)?;
                    self.indent -= 1;
                    self.write_indent();
                    self.write("}\n");
                }
                None => {
                    self.write_indent();
                    if i > 0 {
                        self.write("else {\n");
                    } else {
                        self.write("{\n");
                    }
                    self.indent += 1;
                    self.gen_cond_arm_body(&arm.body)?;
                    self.indent -= 1;
                    self.write_indent();
                    self.write("}\n");
                }
            }
        }

        self.indent -= 1;
        self.write_indent();
        self.write("})()");
        Ok(())
    }

    /// Generate the body of a condition arm.
    /// For multi-statement bodies, generates all statements with `return` on the last expression.
    fn gen_cond_arm_body(&mut self, body: &[crate::parser::Statement]) -> Result<(), JsError> {
        use crate::parser::Statement;
        if body.is_empty() {
            self.write_indent();
            self.write("return undefined;\n");
            return Ok(());
        }
        for (i, stmt) in body.iter().enumerate() {
            let is_last = i == body.len() - 1;
            if is_last {
                if let Statement::Expr(expr) = stmt {
                    self.write_indent();
                    self.write("return ");
                    self.gen_expr(expr)?;
                    self.write(";\n");
                } else {
                    self.gen_statement(stmt)?;
                }
            } else {
                self.gen_statement(stmt)?;
            }
        }
        Ok(())
    }

    /// テンプレートリテラル内の Taida 構文を JS 構文に変換
    ///
    /// Handles two categories of conversion:
    /// 1. **Text segments** (outside `${...}`): escape `\`, `` ` ``, and convert
    ///    `@[` to `[` and `.length()` to `.length_()`.
    /// 2. **Interpolation segments** (`${...}`): convert `@[` and `.length()` only,
    ///    without adding escape sequences.
    ///
    /// Limitation: nested `${...}` and complex expressions inside interpolation
    /// blocks are handled by simple brace matching (first `}` closes the block).
    /// Deeply nested braces (e.g. `${fn(@(a <= 1))}`) would be mis-split. In
    /// practice, Taida's template interpolation is single-expression, so this
    /// suffices for all current use cases.
    fn convert_template_list_literals(template: &str) -> String {
        // Template literals: convert Taida syntax to JS.
        // Segment the template so that escaping is only applied to text outside ${...}
        // interpolation blocks. Inside ${...}, the expression is passed through as-is.
        //
        // Escaping applied to text segments:
        //   - `\` → `\\`  (backslash)
        //   - `` ` `` → `\``  (backtick)
        //   - `@[` → `[`  (list literal)
        //   - `.length()` → `.length_()`  (avoid JS property collision)
        let mut result = String::new();
        let mut rest = template;
        while let Some(start) = rest.find("${") {
            // Escape the text before ${
            result.push_str(&Self::escape_template_text(&rest[..start]));
            if let Some(end) = rest[start..].find('}') {
                // Apply expression-level transforms inside ${...}
                let expr_content = &rest[start..start + end + 1];
                let expr_converted = expr_content
                    .replace("@[", "[")
                    .replace(".length()", ".length_()");
                result.push_str(&expr_converted);
                rest = &rest[start + end + 1..];
            } else {
                break;
            }
        }
        result.push_str(&Self::escape_template_text(rest));
        result
    }

    /// テンプレートリテラルのテキスト部分（${...} の外側）にエスケープを適用
    fn escape_template_text(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('`', "\\`")
            .replace("@[", "[")
            .replace(".length()", ".length_()")
    }

    fn gen_pipeline(&mut self, exprs: &[Expr]) -> Result<(), JsError> {
        if exprs.is_empty() {
            return Ok(());
        }

        // Pipeline is wrapped in an IIFE `(() => { ... })()`, so the `__p`
        // accumulator variable is scoped to the IIFE and cannot collide with
        // user-defined variables in the surrounding scope. The `__` prefix is
        // a defensive convention to avoid shadowing within the pipeline body.
        self.write("(() => {\n");
        self.indent += 1;

        self.write_indent();
        self.write("let __p = ");
        self.gen_expr(&exprs[0])?;
        self.write(";\n");

        for expr in &exprs[1..] {
            self.write_indent();
            self.write("__p = ");
            match expr {
                Expr::FuncCall(callee, args, _) => {
                    if let Expr::Ident(name, _) = callee.as_ref() {
                        match name.as_str() {
                            "debug" => self.write("__taida_debug"),
                            "typeof" => self.write("__taida_typeof"),
                            "assert" => self.write("__taida_assert"),
                            "stdout" => self.write("__taida_stdout"),
                            "stderr" => self.write("__taida_stderr"),
                            "stdin" => self.write("__taida_stdin"),
                            "jsonEncode" => self.write("__taida_jsonEncode"),
                            "jsonPretty" => self.write("__taida_jsonPretty"),
                            "nowMs" => self.write("__taida_nowMs"),
                            "sleep" => self.write("__taida_sleep"),
                            "readBytes" => self.write("__taida_os_readBytes"),
                            "writeFile" => self.write("__taida_os_writeFile"),
                            "writeBytes" => self.write("__taida_os_writeBytes"),
                            "appendFile" => self.write("__taida_os_appendFile"),
                            "remove" => self.write("__taida_os_remove"),
                            "createDir" => self.write("__taida_os_createDir"),
                            "rename" => self.write("__taida_os_rename"),
                            "run" => self.write("__taida_os_run"),
                            "execShell" => self.write("__taida_os_execShell"),
                            "allEnv" => self.write("__taida_os_allEnv"),
                            "argv" => self.write("__taida_os_argv"),
                            "tcpConnect" => self.write("__taida_os_tcpConnect"),
                            "tcpListen" => self.write("__taida_os_tcpListen"),
                            "tcpAccept" => self.write("__taida_os_tcpAccept"),
                            "socketSend" => self.write("__taida_os_socketSend"),
                            "socketSendAll" => self.write("__taida_os_socketSendAll"),
                            "socketRecv" => self.write("__taida_os_socketRecv"),
                            "socketSendBytes" => self.write("__taida_os_socketSendBytes"),
                            "socketRecvBytes" => self.write("__taida_os_socketRecvBytes"),
                            "socketClose" => self.write("__taida_os_socketClose"),
                            "listenerClose" => self.write("__taida_os_listenerClose"),
                            "udpBind" => self.write("__taida_os_udpBind"),
                            "udpSendTo" => self.write("__taida_os_udpSendTo"),
                            "udpRecvFrom" => self.write("__taida_os_udpRecvFrom"),
                            "udpClose" => self.write("__taida_os_udpClose"),
                            "socketRecvExact" => self.write("__taida_os_socketRecvExact"),
                            "dnsResolve" => self.write("__taida_os_dnsResolve"),
                            "poolCreate" => self.write("__taida_os_poolCreate"),
                            "poolAcquire" => self.write("__taida_os_poolAcquire"),
                            "poolRelease" => self.write("__taida_os_poolRelease"),
                            "poolClose" => self.write("__taida_os_poolClose"),
                            "poolHealth" => self.write("__taida_os_poolHealth"),
                            // taida-lang/net HTTP v1 (only when imported)
                            _ if self.try_write_net_builtin(name, "") => {}
                            _ => self.write(name),
                        }
                    } else {
                        self.gen_expr(callee)?;
                    }
                    self.write("(");
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.write(", ");
                        }
                        if matches!(arg, Expr::Placeholder(_)) {
                            self.write("__p");
                        } else {
                            self.gen_expr(arg)?;
                        }
                    }
                    self.write(")");
                }
                Expr::MethodCall(obj, method, args, _) => {
                    // Pipeline method call: replace _ placeholder in obj with __p
                    {
                        if is_removed_list_method(method) {
                            self.write("__taida_list_method_removed(");
                            self.write(&format!("{:?}", method));
                            self.write(")");
                            return Ok(());
                        }
                        let js_method = match method.as_str() {
                            "length" => "length_",
                            other => other,
                        };
                        if matches!(obj.as_ref(), Expr::Placeholder(_)) {
                            self.write(&format!("__p.{}(", js_method));
                        } else {
                            self.gen_expr(obj)?;
                            self.write(&format!(".{}(", js_method));
                        }
                        for (i, arg) in args.iter().enumerate() {
                            if i > 0 {
                                self.write(", ");
                            }
                            if matches!(arg, Expr::Placeholder(_)) {
                                self.write("__p");
                            } else {
                                self.gen_expr(arg)?;
                            }
                        }
                        self.write(")");
                    }
                }
                Expr::MoldInst(name, type_args, fields, _) => {
                    // JSNew in pipeline: JSNew[ClassName](__p, ...) or JSNew[ClassName](...)
                    if name == "JSNew" {
                        if let Some(Expr::Ident(class_name, _)) = type_args.first() {
                            self.write(&format!("new {}(", class_name));
                            // Pipeline value __p as first arg, followed by fields
                            let has_placeholder = fields
                                .iter()
                                .any(|f| matches!(f.value, Expr::Placeholder(_)));
                            if has_placeholder {
                                for (i, field) in fields.iter().enumerate() {
                                    if i > 0 {
                                        self.write(", ");
                                    }
                                    if matches!(field.value, Expr::Placeholder(_)) {
                                        self.write("__p");
                                    } else {
                                        self.gen_expr(&field.value)?;
                                    }
                                }
                            } else if fields.is_empty() {
                                self.write("__p");
                            } else {
                                self.write("__p");
                                for field in fields {
                                    self.write(", ");
                                    self.gen_expr(&field.value)?;
                                }
                            }
                            self.write(")");
                        }
                    } else {
                        // Pipeline MoldInst: replace _ placeholders in type_args with __p
                        // OS molds need to be mapped to runtime function names
                        let js_name = match name.as_str() {
                            "Read" => "__taida_os_read",
                            "ListDir" => "__taida_os_listdir",
                            "Stat" => "__taida_os_stat",
                            "Exists" => "__taida_os_exists",
                            "EnvVar" => "__taida_os_envvar",
                            _ => name.as_str(),
                        };
                        self.write("__taida_solidify(");
                        self.write(&format!("{}(", js_name));
                        let has_placeholder =
                            type_args.iter().any(|a| matches!(a, Expr::Placeholder(_)));
                        if has_placeholder {
                            for (i, arg) in type_args.iter().enumerate() {
                                if i > 0 {
                                    self.write(", ");
                                }
                                if matches!(arg, Expr::Placeholder(_)) {
                                    self.write("__p");
                                } else {
                                    self.gen_expr(arg)?;
                                }
                            }
                        } else {
                            // No placeholder — insert __p as first type arg
                            self.write("__p");
                            for arg in type_args {
                                self.write(", ");
                                self.gen_expr(arg)?;
                            }
                        }
                        if !fields.is_empty() {
                            self.write(", { ");
                            for (i, field) in fields.iter().enumerate() {
                                if i > 0 {
                                    self.write(", ");
                                }
                                self.write(&format!("{}: ", field.name));
                                self.gen_expr(&field.value)?;
                            }
                            self.write(" }");
                        }
                        self.write(")");
                        self.write(")");
                    }
                }
                Expr::Ident(name, _) => match name.as_str() {
                    "debug" => self.write("__taida_debug(__p)"),
                    "typeof" => self.write("__taida_typeof(__p)"),
                    "assert" => self.write("__taida_assert(__p)"),
                    "stdout" => self.write("__taida_stdout(__p)"),
                    "stderr" => self.write("__taida_stderr(__p)"),
                    "stdin" => self.write("__taida_stdin(__p)"),
                    "jsonEncode" => self.write("__taida_jsonEncode(__p)"),
                    "jsonPretty" => self.write("__taida_jsonPretty(__p)"),
                    "nowMs" => self.write("__taida_nowMs()"),
                    "sleep" => self.write("__taida_sleep(__p)"),
                    "readBytes" => self.write("__taida_os_readBytes(__p)"),
                    "writeFile" => self.write("__taida_os_writeFile(__p)"),
                    "writeBytes" => self.write("__taida_os_writeBytes(__p)"),
                    "appendFile" => self.write("__taida_os_appendFile(__p)"),
                    "remove" => self.write("__taida_os_remove(__p)"),
                    "createDir" => self.write("__taida_os_createDir(__p)"),
                    "rename" => self.write("__taida_os_rename(__p)"),
                    "run" => self.write("__taida_os_run(__p)"),
                    "execShell" => self.write("__taida_os_execShell(__p)"),
                    "allEnv" => self.write("__taida_os_allEnv(__p)"),
                    "argv" => self.write("__taida_os_argv()"),
                    "tcpConnect" => self.write("__taida_os_tcpConnect(__p)"),
                    "tcpListen" => self.write("__taida_os_tcpListen(__p)"),
                    "tcpAccept" => self.write("__taida_os_tcpAccept(__p)"),
                    "socketSend" => self.write("__taida_os_socketSend(__p)"),
                    "socketSendAll" => self.write("__taida_os_socketSendAll(__p)"),
                    "socketRecv" => self.write("__taida_os_socketRecv(__p)"),
                    "socketSendBytes" => self.write("__taida_os_socketSendBytes(__p)"),
                    "socketRecvBytes" => self.write("__taida_os_socketRecvBytes(__p)"),
                    "socketClose" => self.write("__taida_os_socketClose(__p)"),
                    "listenerClose" => self.write("__taida_os_listenerClose(__p)"),
                    "udpBind" => self.write("__taida_os_udpBind(__p)"),
                    "udpSendTo" => self.write("__taida_os_udpSendTo(__p)"),
                    "udpRecvFrom" => self.write("__taida_os_udpRecvFrom(__p)"),
                    "udpClose" => self.write("__taida_os_udpClose(__p)"),
                    "socketRecvExact" => self.write("__taida_os_socketRecvExact(__p)"),
                    "dnsResolve" => self.write("__taida_os_dnsResolve(__p)"),
                    "poolCreate" => self.write("__taida_os_poolCreate(__p)"),
                    "poolAcquire" => self.write("__taida_os_poolAcquire(__p)"),
                    "poolRelease" => self.write("__taida_os_poolRelease(__p)"),
                    "poolClose" => self.write("__taida_os_poolClose(__p)"),
                    "poolHealth" => self.write("__taida_os_poolHealth(__p)"),
                    // taida-lang/net HTTP v1 (only when imported)
                    _ if self.try_write_net_builtin(name, "(__p)") => {}
                    _ => self.write(&format!("{}(__p)", name)),
                },
                _ => {
                    self.gen_expr(expr)?;
                }
            }
            self.write(";\n");
        }

        self.write_indent();
        self.write("return __p;\n");
        self.indent -= 1;
        self.write_indent();
        self.write("})()");
        Ok(())
    }
}

fn merge_field_defs(parent: &[FieldDef], child: &[FieldDef]) -> Vec<FieldDef> {
    let mut merged = parent.to_vec();
    for child_field in child {
        if let Some(existing) = merged
            .iter_mut()
            .find(|field| field.name == child_field.name)
        {
            *existing = child_field.clone();
        } else {
            merged.push(child_field.clone());
        }
    }
    merged
}

/// Collect function names that are called in tail position from the given function body.
/// This is used to build the tail-call graph for mutual recursion detection.
fn collect_tail_call_targets(
    _self_name: &str,
    body: &[Statement],
    targets: &mut std::collections::HashSet<String>,
) {
    // Find the last expression in the body
    let last_expr = body.iter().rev().find_map(|s| match s {
        Statement::Expr(e) => Some(e),
        _ => None,
    });
    if let Some(expr) = last_expr {
        collect_tail_targets_from_expr(expr, targets);
    }
}

fn collect_tail_targets_from_expr(expr: &Expr, targets: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::FuncCall(callee, _, _) => {
            if let Expr::Ident(name, _) = callee.as_ref() {
                targets.insert(name.clone());
            }
        }
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(expr) = arm.last_expr() {
                    collect_tail_targets_from_expr(expr, targets);
                }
            }
        }
        _ => {}
    }
}

/// OS API mold names that are always async sources when unmolded.
/// These require `async function` generation when `]=>` appears inside a function.
const OS_ASYNC_MOLDS: &[&str] = &[
    "Read",
    "ListDir",
    "Stat",
    "Exists",
    "EnvVar",
    "ReadAsync",
    "HttpGet",
    "HttpPost",
    "HttpRequest",
];

/// OS/API/prelude function names that can yield pending async sources.
/// These are runtime functions (not molds).
const OS_ASYNC_FUNCS: &[&str] = &[
    "sleep",
    "tcpConnect",
    "tcpListen",
    "tcpAccept",
    "socketSend",
    "socketSendAll",
    "socketRecv",
    "socketSendBytes",
    "socketRecvBytes",
    "udpBind",
    "udpSendTo",
    "udpRecvFrom",
    "socketClose",
    "listenerClose",
    "udpClose",
    "socketRecvExact",
    "dnsResolve",
    "poolAcquire",
    "poolClose",
    // taida-lang/net HTTP v1 — call-site rewriting is guarded by has_net_import,
    // but async detection here is intentionally left unguarded to avoid threading
    // has_net_import through all recursive free functions. Over-inclusion only
    // affects the unlikely case of a user-defined `httpServe` without net import.
    "httpServe",
];

fn callee_is_os_async_func(callee: &Expr) -> bool {
    match callee {
        Expr::Ident(name, _) => OS_ASYNC_FUNCS.contains(&name.as_str()),
        _ => false,
    }
}

fn mold_propagates_async_from_args(name: &str) -> bool {
    matches!(name, "All" | "Race" | "Timeout")
}

/// Check if an unmold source expression involves an OS async mold/function (true Promise).
fn is_os_async_unmold_source(
    source: &Expr,
    os_async_vars: &std::collections::HashSet<String>,
) -> bool {
    match source {
        Expr::MoldInst(name, type_args, fields, _) => {
            OS_ASYNC_MOLDS.contains(&name.as_str())
                || (mold_propagates_async_from_args(name)
                    && (type_args
                        .iter()
                        .any(|a| is_os_async_unmold_source(a, os_async_vars))
                        || fields
                            .iter()
                            .any(|f| is_os_async_unmold_source(&f.value, os_async_vars))))
        }
        Expr::FuncCall(callee, _, _) => {
            callee_is_os_async_func(callee) || is_os_async_unmold_source(callee, os_async_vars)
        }
        Expr::Ident(name, _) => os_async_vars.contains(name),
        Expr::MethodCall(receiver, _, _, _) => is_os_async_unmold_source(receiver, os_async_vars),
        Expr::FieldAccess(receiver, _, _) => is_os_async_unmold_source(receiver, os_async_vars),
        Expr::BinaryOp(left, _, right, _) => {
            is_os_async_unmold_source(left, os_async_vars)
                || is_os_async_unmold_source(right, os_async_vars)
        }
        Expr::UnaryOp(_, inner, _) => is_os_async_unmold_source(inner, os_async_vars),
        Expr::Pipeline(exprs, _) => exprs
            .iter()
            .any(|e| is_os_async_unmold_source(e, os_async_vars)),
        Expr::BuchiPack(fields, _) => fields
            .iter()
            .any(|f| is_os_async_unmold_source(&f.value, os_async_vars)),
        Expr::TypeInst(_, fields, _) => fields
            .iter()
            .any(|f| is_os_async_unmold_source(&f.value, os_async_vars)),
        Expr::ListLit(items, _) => items
            .iter()
            .any(|e| is_os_async_unmold_source(e, os_async_vars)),
        Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
            arm.condition
                .as_ref()
                .is_some_and(|c| is_os_async_unmold_source(c, os_async_vars))
                || arm.body.iter().any(|stmt| {
                    if let crate::parser::Statement::Expr(e) = stmt {
                        is_os_async_unmold_source(e, os_async_vars)
                    } else {
                        false
                    }
                })
        }),
        _ => false,
    }
}

/// Check if an expression tree contains Expr::Unmold whose inner expression
/// involves an OS async mold. Recurses into sub-expressions but NOT lambdas.
fn expr_contains_os_async_unmold(
    expr: &Expr,
    os_async_vars: &std::collections::HashSet<String>,
) -> bool {
    match expr {
        Expr::Unmold(inner, _) => is_os_async_unmold_source(inner, os_async_vars),
        Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
            arm.condition
                .as_ref()
                .is_some_and(|c| expr_contains_os_async_unmold(c, os_async_vars))
                || arm.body.iter().any(|stmt| {
                    if let crate::parser::Statement::Expr(e) = stmt {
                        expr_contains_os_async_unmold(e, os_async_vars)
                    } else {
                        false
                    }
                })
        }),
        Expr::FuncCall(callee, args, _) => {
            expr_contains_os_async_unmold(callee, os_async_vars)
                || args
                    .iter()
                    .any(|a| expr_contains_os_async_unmold(a, os_async_vars))
        }
        Expr::MethodCall(obj, _, args, _) => {
            expr_contains_os_async_unmold(obj, os_async_vars)
                || args
                    .iter()
                    .any(|a| expr_contains_os_async_unmold(a, os_async_vars))
        }
        Expr::FieldAccess(obj, _, _) => expr_contains_os_async_unmold(obj, os_async_vars),
        Expr::BinaryOp(l, _, r, _) => {
            expr_contains_os_async_unmold(l, os_async_vars)
                || expr_contains_os_async_unmold(r, os_async_vars)
        }
        Expr::UnaryOp(_, inner, _) => expr_contains_os_async_unmold(inner, os_async_vars),
        Expr::Pipeline(exprs, _) => exprs
            .iter()
            .any(|e| expr_contains_os_async_unmold(e, os_async_vars)),
        Expr::MoldInst(_, type_args, fields, _) => {
            type_args
                .iter()
                .any(|a| expr_contains_os_async_unmold(a, os_async_vars))
                || fields
                    .iter()
                    .any(|f| expr_contains_os_async_unmold(&f.value, os_async_vars))
        }
        Expr::TypeInst(_, fields, _) => fields
            .iter()
            .any(|f| expr_contains_os_async_unmold(&f.value, os_async_vars)),
        Expr::BuchiPack(fields, _) => fields
            .iter()
            .any(|f| expr_contains_os_async_unmold(&f.value, os_async_vars)),
        Expr::ListLit(items, _) => items
            .iter()
            .any(|e| expr_contains_os_async_unmold(e, os_async_vars)),
        Expr::Lambda(_, _, _) => false, // Lambdas get their own async detection
        _ => false,
    }
}

/// Check if statements contain ]=> that unmolds an OS async (true Promise) value.
/// Only OS API molds that return real Promises trigger async function generation.
/// Standard Taida molds (Async, Div, Mod, etc.) use sync __TaidaAsync thenables
/// and do NOT require async functions.
/// Also checks for Expr::Unmold within expressions (e.g. inside CondBranch).
/// Collect names of user-defined functions called in a statement list (non-recursive into nested FuncDefs).
fn collect_func_calls_in_stmts(
    stmts: &[Statement],
    func_names: &std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Expr(expr) => collect_func_calls_in_expr(expr, func_names, out),
            Statement::Assignment(assign) => {
                collect_func_calls_in_expr(&assign.value, func_names, out)
            }
            Statement::UnmoldForward(u) => collect_func_calls_in_expr(&u.source, func_names, out),
            Statement::UnmoldBackward(u) => collect_func_calls_in_expr(&u.source, func_names, out),
            Statement::ErrorCeiling(ec) => {
                collect_func_calls_in_stmts(&ec.handler_body, func_names, out);
            }
            _ => {}
        }
    }
}

fn collect_func_calls_in_expr(
    expr: &Expr,
    func_names: &std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    match expr {
        Expr::FuncCall(callee, args, _) => {
            if let Expr::Ident(name, _) = callee.as_ref()
                && func_names.contains(name)
                && !out.contains(name)
            {
                out.push(name.clone());
            }
            collect_func_calls_in_expr(callee, func_names, out);
            for arg in args {
                collect_func_calls_in_expr(arg, func_names, out);
            }
        }
        Expr::MethodCall(obj, _, args, _) => {
            collect_func_calls_in_expr(obj, func_names, out);
            for arg in args {
                collect_func_calls_in_expr(arg, func_names, out);
            }
        }
        Expr::BinaryOp(l, _, r, _) => {
            collect_func_calls_in_expr(l, func_names, out);
            collect_func_calls_in_expr(r, func_names, out);
        }
        Expr::UnaryOp(_, inner, _) => collect_func_calls_in_expr(inner, func_names, out),
        Expr::FieldAccess(obj, _, _) => collect_func_calls_in_expr(obj, func_names, out),
        Expr::Pipeline(exprs, _) => {
            for e in exprs {
                collect_func_calls_in_expr(e, func_names, out);
            }
        }
        Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
            for f in fields {
                collect_func_calls_in_expr(&f.value, func_names, out);
            }
        }
        Expr::ListLit(exprs, _) => {
            for e in exprs {
                collect_func_calls_in_expr(e, func_names, out);
            }
        }
        Expr::MoldInst(_, type_args, fields, _) => {
            for a in type_args {
                collect_func_calls_in_expr(a, func_names, out);
            }
            for f in fields {
                collect_func_calls_in_expr(&f.value, func_names, out);
            }
        }
        Expr::Lambda(_, body, _) => collect_func_calls_in_expr(body, func_names, out),
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(cond) = &arm.condition {
                    collect_func_calls_in_expr(cond, func_names, out);
                }
                for stmt in &arm.body {
                    if let crate::parser::Statement::Expr(e) = stmt {
                        collect_func_calls_in_expr(e, func_names, out);
                    }
                }
            }
        }
        _ => {}
    }
}

fn stmts_contain_async_unmold(stmts: &[Statement]) -> bool {
    // Collect variable names assigned from async sources in this scope.
    // Fixed-point is needed for transitive cases like:
    //   s <= sleep(0)
    //   t <= Timeout[s, 100]()
    // where `t` should also be considered async.
    let mut os_async_vars = std::collections::HashSet::new();
    loop {
        let mut changed = false;
        for stmt in stmts {
            if let Statement::Assignment(assign) = stmt
                && is_os_async_unmold_source(&assign.value, &os_async_vars)
            {
                changed |= os_async_vars.insert(assign.target.clone());
            }
        }
        if !changed {
            break;
        }
    }

    for stmt in stmts {
        match stmt {
            Statement::UnmoldForward(unmold) => {
                if is_os_async_unmold_source(&unmold.source, &os_async_vars) {
                    return true;
                }
            }
            Statement::UnmoldBackward(unmold) => {
                if is_os_async_unmold_source(&unmold.source, &os_async_vars) {
                    return true;
                }
            }
            Statement::FuncDef(_) => {
                // Don't recurse into nested function defs — they get their own async detection
            }
            Statement::ErrorCeiling(ec) => {
                if stmts_contain_async_unmold(&ec.handler_body) {
                    return true;
                }
            }
            Statement::Expr(expr) => {
                if expr_contains_os_async_unmold(expr, &os_async_vars) {
                    return true;
                }
            }
            Statement::Assignment(assign) => {
                if expr_contains_os_async_unmold(&assign.value, &os_async_vars) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Compute relative path from `base` directory to `target` file.
fn pathdiff(base: &std::path::Path, target: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    let base = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(base)
    };
    let target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(target)
    };

    let base_comps: Vec<_> = base.components().collect();
    let target_comps: Vec<_> = target.components().collect();

    // Find common prefix length
    let common = base_comps
        .iter()
        .zip(target_comps.iter())
        .take_while(|(b, t)| b == t)
        .count();

    let mut result = PathBuf::new();
    // Go up from base
    for _ in common..base_comps.len() {
        result.push("..");
    }
    // Go down to target
    for comp in &target_comps[common..] {
        result.push(comp);
    }

    if result.as_os_str().is_empty() {
        None
    } else {
        Some(result)
    }
}

/// .td ファイルを JS にトランスパイル (with file context for package import resolution)
pub fn transpile_with_context(
    program: &Program,
    source_file: &std::path::Path,
    project_root: &std::path::Path,
    output_file: &std::path::Path,
) -> Result<String, JsError> {
    let mut codegen = JsCodegen::new();
    codegen.set_file_context(source_file, project_root, output_file);
    codegen.generate(program)
}

/// .td ファイルを JS にトランスパイル (with build context for output-layout-aware imports)
pub fn transpile_with_build_context(
    program: &Program,
    source_file: &std::path::Path,
    project_root: Option<&std::path::Path>,
    output_file: &std::path::Path,
    entry_root: &std::path::Path,
    out_root: &std::path::Path,
) -> Result<String, JsError> {
    let mut codegen = JsCodegen::new();
    codegen.source_file = Some(source_file.to_path_buf());
    codegen.output_file = Some(output_file.to_path_buf());
    if let Some(root) = project_root {
        codegen.project_root = Some(root.to_path_buf());
    }
    codegen.set_build_context(entry_root, out_root);
    codegen.generate(program)
}

/// .td ファイルを JS にトランスパイル
pub fn transpile(source: &str) -> Result<String, JsError> {
    let (program, parse_errors) = crate::parser::parse(source);
    if !parse_errors.is_empty() {
        let msgs: Vec<String> = parse_errors.iter().map(|e| format!("{}", e)).collect();
        return Err(JsError {
            message: format!("parse errors:\n{}", msgs.join("\n")),
        });
    }

    let mut codegen = JsCodegen::new();
    codegen.generate(&program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn js_contains(source: &str, needle: &str) -> bool {
        let js = transpile(source).expect("transpile failed");
        js.contains(needle)
    }

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    // ── Optional — ABOLISHED (v0.8.0) ──
    // Optional tests removed. Optional has been replaced by Lax[T].

    // ── Result methods in runtime ──

    #[test]
    fn test_js_runtime_result_flatmap() {
        let js = transpile("x = 1\nx").unwrap();
        // Result function should contain flatMap
        assert!(
            js.contains("flatMap(fn)"),
            "Result should have flatMap method"
        );
    }

    #[test]
    fn test_js_runtime_result_maperror() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("mapError(fn)"),
            "Result should have mapError method"
        );
    }

    #[test]
    fn test_js_runtime_result_getorthrow() {
        let js = transpile("x = 1\nx").unwrap();
        // Both Result and ResultErr should have getOrThrow
        assert!(
            js.contains("ResultError"),
            "ResultErr getOrThrow should throw ResultError"
        );
    }

    #[test]
    fn test_js_runtime_result_tostring() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("'Result('"),
            "Result toString should produce Result(...)"
        );
        assert!(
            js.contains("'Result(throw <= '"),
            "Result toString should produce Result(throw <= ...) for errors"
        );
    }

    #[test]
    fn test_molten_rejects_type_args_codegen() {
        let err = transpile("m <= Molten[1]()").expect_err("Molten with type args should fail");
        assert!(
            err.message.contains("Molten takes no type arguments"),
            "Unexpected error: {}",
            err.message
        );
    }

    // ── Prelude utility functions in runtime ──

    #[test]
    fn test_js_runtime_hashmap() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function hashMap(entries)"),
            "Runtime should contain hashMap function"
        );
    }

    #[test]
    fn test_js_runtime_setof() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function setOf(items)"),
            "Runtime should contain setOf function"
        );
    }

    #[test]
    fn test_js_runtime_range() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function range(start, end)"),
            "Runtime should contain range function"
        );
    }

    #[test]
    fn test_js_runtime_enumerate() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function enumerate(list)"),
            "Runtime should contain enumerate function"
        );
    }

    #[test]
    fn test_js_runtime_zip() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function zip(a, b)"),
            "Runtime should contain zip function"
        );
    }

    #[test]
    fn test_js_runtime_assert() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function __taida_assert(cond, msg)"),
            "Runtime should contain __taida_assert function"
        );
    }

    #[test]
    fn test_js_runtime_typeof() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function __taida_typeof(x)"),
            "Runtime should contain __taida_typeof function"
        );
    }

    // ── Codegen mapping: typeof → __taida_typeof, assert → __taida_assert ──

    #[test]
    fn test_js_codegen_typeof_mapping() {
        assert!(
            js_contains("x = typeof(42)\nx", "__taida_typeof(42)"),
            "typeof(42) should be mapped to __taida_typeof(42)"
        );
    }

    #[test]
    fn test_js_codegen_assert_mapping() {
        assert!(
            js_contains("assert(true, \"ok\")\n", "__taida_assert(true, \"ok\")"),
            "assert(true, \"ok\") should be mapped to __taida_assert(true, \"ok\")"
        );
    }

    // ── Partial application: empty slot → closure ──

    #[test]
    fn test_js_codegen_partial_application_single() {
        // add(5, ) should generate a closure
        assert!(
            js_contains(
                "add x y = x + y => :Int\nadd5 = add(5, )\nadd5",
                "((__pa_0) => add(5, __pa_0))"
            ),
            "add(5, ) should generate ((__pa_0) => add(5, __pa_0))"
        );
    }

    #[test]
    fn test_js_codegen_partial_application_first_arg() {
        // multiply(, 2) should generate a closure
        assert!(
            js_contains(
                "mul x y = x * y => :Int\ndouble = mul(, 2)\ndouble",
                "((__pa_0) => mul(__pa_0, 2))"
            ),
            "mul(, 2) should generate ((__pa_0) => mul(__pa_0, 2))"
        );
    }

    #[test]
    fn test_js_codegen_partial_application_multiple() {
        // func(, 1, ) should generate closure with two params
        assert!(
            js_contains(
                "f x y z = x + y + z => :Int\ng = f(, 1, )\ng",
                "((__pa_0, __pa_1) => f(__pa_0, 1, __pa_1))"
            ),
            "f(, 1, ) should generate ((__pa_0, __pa_1) => f(__pa_0, 1, __pa_1))"
        );
    }

    // ── Str methods in runtime ──

    #[test]
    fn test_js_runtime_str_patches() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("__taida_str_patched"),
            "Runtime should contain string patches"
        );
        assert!(
            js.contains("function Reverse("),
            "Runtime should contain Reverse mold function"
        );
    }

    #[test]
    fn test_js_runtime_operation_molds() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function Upper("),
            "Runtime should contain Upper mold function"
        );
        assert!(
            js.contains("function Lower("),
            "Runtime should contain Lower mold function"
        );
        assert!(
            js.contains("function Sort("),
            "Runtime should contain Sort mold function"
        );
        assert!(
            js.contains("function Abs("),
            "Runtime should contain Abs mold function"
        );
        assert!(
            js.contains("function Find("),
            "Runtime should contain Find mold function"
        );
    }

    #[test]
    fn test_js_runtime_safe_unmold() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function __taida_unmold("),
            "Runtime should contain __taida_unmold helper"
        );
    }

    #[test]
    fn test_js_codegen_str_trimstart() {
        assert!(
            js_contains("s = \"  hi  \"\ns.trimStart()", ".trimStart()"),
            "str.trimStart() should pass through as-is"
        );
    }

    // ── Number methods in runtime ──

    #[test]
    fn test_js_runtime_number_methods() {
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("isNaN"),
            "Runtime should contain isNaN on Number.prototype"
        );
        assert!(
            js.contains("isInfinite"),
            "Runtime should contain isInfinite on Number.prototype"
        );
        assert!(
            js.contains("isFinite"),
            "Runtime should contain isFinite on Number.prototype"
        );
        assert!(
            js.contains("isPositive"),
            "Runtime should contain isPositive on Number.prototype"
        );
        assert!(
            js.contains("isNegative"),
            "Runtime should contain isNegative on Number.prototype"
        );
        assert!(
            js.contains("isZero"),
            "Runtime should contain isZero on Number.prototype"
        );
    }

    #[test]
    fn test_js_codegen_number_isnan() {
        assert!(
            js_contains("x = 42\nx.isNaN()", ".isNaN()"),
            "x.isNaN() should pass through as-is"
        );
    }

    #[test]
    fn test_js_codegen_number_ispositive() {
        assert!(
            js_contains("x = 42\nx.isPositive()", ".isPositive()"),
            "x.isPositive() should pass through as-is"
        );
    }

    // ── JSNew mold (taida-lang/js) ──

    #[test]
    fn test_js_codegen_jsnew_no_args() {
        // JSNew[Hono]() ]=> app  →  const app = new Hono();
        assert!(
            js_contains("JSNew[Hono]() ]=> app", "new Hono()"),
            "JSNew[Hono]() should generate new Hono()"
        );
    }

    #[test]
    fn test_js_codegen_jsnew_with_args() {
        // JSNew[Server](8080) ]=> server  →  const server = new Server(8080);
        assert!(
            js_contains("JSNew[Server](8080) ]=> server", "new Server(8080)"),
            "JSNew[Server](8080) should generate new Server(8080)"
        );
    }

    #[test]
    fn test_js_codegen_jsnew_with_multiple_args() {
        // JSNew[Uint8Array](16) ]=> buf  →  const buf = new Uint8Array(16);
        assert!(
            js_contains("JSNew[Uint8Array](16) ]=> buf", "new Uint8Array(16)"),
            "JSNew[Uint8Array](16) should generate new Uint8Array(16)"
        );
    }

    #[test]
    fn test_js_codegen_jsnew_unmold_forward() {
        let js = transpile("JSNew[Hono]() ]=> app\napp").unwrap();
        assert!(
            js.contains("const app = await __taida_unmold_async(new Hono())"),
            "JSNew unmold forward should wrap with await __taida_unmold_async: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsnew_unmold_backward() {
        let js = transpile("app <=[ JSNew[Hono]()\napp").unwrap();
        assert!(
            js.contains("const app = await __taida_unmold_async(new Hono())"),
            "JSNew unmold backward should wrap with await __taida_unmold_async: got {}",
            js
        );
    }

    #[test]
    fn test_os_async_function_call_marks_function_async() {
        let src = r#"
fetchUdp p =
  s <= udpBind("127.0.0.1", 0)
  s ]=> v
  v
"#;
        let js = transpile(src).expect("transpile should succeed");
        assert!(
            js.contains("async function fetchUdp("),
            "OS async function call unmold should mark function async: got {}",
            js
        );
        assert!(
            js.contains("await __taida_unmold_async(s)"),
            "OS async function call unmold should emit await unmold: got {}",
            js
        );
    }

    #[test]
    fn test_sleep_all_inside_function_marks_function_async() {
        let src = r#"
waitBoth p =
  all <= All[@[sleep(0), sleep(0)]]()
  all ]=> vals
  vals.length()
=> :Int
"#;
        let js = transpile(src).expect("transpile should succeed");
        assert!(
            js.contains("async function waitBoth("),
            "All+sleep unmold should mark function async: got {}",
            js
        );
        assert!(
            js.contains("await __taida_unmold_async(all)"),
            "All+sleep unmold should emit await unmold: got {}",
            js
        );
    }

    #[test]
    fn test_sleep_timeout_direct_unmold_marks_function_async() {
        let src = r#"
waitWithTimeout p =
  Timeout[sleep(0), 100]() ]=> _done
  1
=> :Int
"#;
        let js = transpile(src).expect("transpile should succeed");
        assert!(
            js.contains("async function waitWithTimeout("),
            "Timeout+sleep unmold should mark function async: got {}",
            js
        );
        assert!(
            js.contains("await __taida_unmold_async(__taida_solidify(Timeout("),
            "Timeout+sleep direct unmold should emit await unmold: got {}",
            js
        );
    }

    #[test]
    fn test_sleep_timeout_via_assigned_var_marks_function_async() {
        let src = r#"
waitWithTimeout p =
  s <= sleep(0)
  t <= Timeout[s, 100]()
  t ]=> _done
  1
=> :Int
"#;
        let js = transpile(src).expect("transpile should succeed");
        assert!(
            js.contains("async function waitWithTimeout("),
            "Timeout+sleep via var unmold should mark function async: got {}",
            js
        );
        assert!(
            js.contains("await __taida_unmold_async(t)"),
            "Timeout+sleep via var unmold should emit await unmold: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsnew_import_skipped() {
        // taida-lang/js import should not generate any ESM import statement
        let js = transpile(">>> taida-lang/js => @(JSNew)\nJSNew[Hono]() ]=> app\napp").unwrap();
        assert!(
            !js.contains("import {"),
            "taida-lang/js import should be skipped in JS output (no ESM import): got {}",
            js
        );
        assert!(
            js.contains("new Hono()"),
            "JSNew should still generate new: got {}",
            js
        );
    }

    #[test]
    fn test_package_import_resolution_failure_is_codegen_error() {
        let dir = unique_temp_dir("taida_js_missing_pkg");
        let main = dir.join("main.td");
        std::fs::write(&main, ">>> alice/missing => @(run)\nstdout(\"ok\")\n")
            .expect("write main.td");

        let source = std::fs::read_to_string(&main).expect("read main.td");
        let (program, parse_errors) = crate::parser::parse(&source);
        assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

        let err = transpile_with_context(&program, &main, &dir, &dir.join("out.mjs"))
            .expect_err("unresolved package import should fail codegen");
        assert!(
            err.message
                .contains("Could not resolve package import 'alice/missing'"),
            "unexpected error: {}",
            err.message
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── JSSet tests ──

    #[test]
    fn test_js_codegen_jsset_basic() {
        // JSSet[obj, "key", "value"]() → IIFE that sets property and returns obj
        let js = transpile("obj = 1\nJSSet[obj, \"key\", \"value\"]()\nobj").unwrap();
        assert!(
            js.contains("__o[\"key\"] = \"value\""),
            "JSSet should generate property assignment: got {}",
            js
        );
        assert!(
            js.contains("return __o;"),
            "JSSet should return the object: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsset_unmold() {
        let js = transpile("obj = 1\nJSSet[obj, \"x\", 42]() ]=> result\nresult").unwrap();
        assert!(
            js.contains("__taida_unmold"),
            "JSSet unmold should wrap with __taida_unmold: got {}",
            js
        );
        assert!(
            js.contains("__o[\"x\"] = 42"),
            "JSSet should set property: got {}",
            js
        );
    }

    // ── JSBind tests ──

    #[test]
    fn test_js_codegen_jsbind_basic() {
        // JSBind[obj, "method"]() → obj["method"].bind(obj)
        let js = transpile("obj = 1\nJSBind[obj, \"method\"]()\nobj").unwrap();
        assert!(
            js.contains(".bind("),
            "JSBind should generate .bind(): got {}",
            js
        );
        assert!(
            js.contains("[\"method\"]"),
            "JSBind should access method by name: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsbind_unmold() {
        let js = transpile("obj = 1\nJSBind[obj, \"fetch\"]() ]=> bound\nbound").unwrap();
        assert!(
            js.contains("__taida_unmold"),
            "JSBind unmold should wrap with __taida_unmold: got {}",
            js
        );
        assert!(
            js.contains(".bind("),
            "JSBind should generate .bind(): got {}",
            js
        );
    }

    // ── JSSpread tests ──

    #[test]
    fn test_js_codegen_jsspread_basic() {
        // JSSpread[target, source]() → __taida_js_spread(target, source)
        let js = transpile("a = 1\nb = 2\nJSSpread[a, b]()\na").unwrap();
        assert!(
            js.contains("__taida_js_spread("),
            "JSSpread should call __taida_js_spread: got {}",
            js
        );
    }

    #[test]
    fn test_js_codegen_jsspread_unmold() {
        let js = transpile("a = 1\nb = 2\nJSSpread[a, b]() ]=> merged\nmerged").unwrap();
        assert!(
            js.contains("__taida_unmold"),
            "JSSpread unmold should wrap with __taida_unmold: got {}",
            js
        );
        assert!(
            js.contains("__taida_js_spread("),
            "JSSpread should call __taida_js_spread: got {}",
            js
        );
    }

    #[test]
    fn test_js_runtime_jsspread_helper() {
        // Verify __taida_js_spread is present in the runtime
        let js = transpile("x = 1\nx").unwrap();
        assert!(
            js.contains("function __taida_js_spread("),
            "Runtime should include __taida_js_spread helper: got {}",
            js
        );
    }
}

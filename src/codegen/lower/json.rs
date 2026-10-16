use super::{HIDDEN, LowerError, Lowering, simple_hash};
/// JSON schema resolution for the Taida native backend.
///
/// Contains `resolve_json_schema_descriptor`, `typedef_to_schema_descriptor`,
/// and `type_expr_to_schema_descriptor`.
///
/// These are `impl Lowering` methods split from lower.rs for maintainability.
use crate::codegen::ir::{IrFunction, IrInst, IrVar};
use crate::parser::*;
use crate::types::Type;

impl Lowering {
    // -- JSON Schema Resolution (compile-time) --

    /// Resolve a JSON schema from an AST expression into a descriptor string
    /// for the C runtime. The schema expression is interpreted as a type descriptor:
    /// - Ident("Int"/"Str"/"Float"/"Bool") -> primitive
    /// - Ident("User") -> lookup TypeDef by name
    /// - ListLit([Ident("Pilot")]) -> list schema
    ///
    /// Descriptor format:
    /// "i" = Int, "f" = Float, "s" = Str, "b" = Bool, "y" = Bytes
    /// "Q" = WebRequest, "R" = WebResponse, "a" = untyped fallback
    /// "T{TypeName|field1:desc,field2:desc,...}" = TypeDef
    /// "N{TypeName}" = the enclosing TypeDef of that name (self-reference)
    /// "L{desc}" = List of elements
    /// "E{EnumName|Variant1,Variant2,...}" = Enum.
    ///
    /// Taida の命名規則（英数字 + `_`、`docs/reference/naming_conventions.md`）により
    /// Variant 名に `,` `|` `{` `}` は出現しないため、エスケープ不要。
    /// D1 懸念の最終決定: escape なし。parser 側で名前が汚染された場合は別 ticket。
    pub(crate) fn resolve_json_schema_descriptor(&self, expr: &Expr) -> Result<String, LowerError> {
        match expr {
            Expr::Ident(name, _) => match name.as_str() {
                "Int" => Ok("i".to_string()),
                "Str" => Ok("s".to_string()),
                "Float" => Ok("f".to_string()),
                "Bool" => Ok("b".to_string()),
                "Bytes" => Ok("y".to_string()),
                "WebRequest" => Ok("Q".to_string()),
                "WebResponse" => Ok("R".to_string()),
                type_name => {
                    // C16: TypeDef wins over Enum when both exist (precedence
                    // mirrors the interpreter; collisions are forbidden today
                    // but explicit ordering keeps the runtime predictable).
                    if let Some(field_types) = self.type_field_types.get(type_name) {
                        self.typedef_to_schema_descriptor(type_name, field_types, &mut Vec::new())
                    } else if let Some(variants) = self.enum_defs.get(type_name) {
                        Ok(Self::enum_to_schema_descriptor(type_name, variants))
                    } else {
                        Err(LowerError {
                            message: format!(
                                "Unknown schema type '{}' for JSON casting. Define it as a TypeDef first.",
                                type_name
                            ),
                        })
                    }
                }
            },
            // @[Schema] -- list type
            Expr::ListLit(items, _) => {
                if items.len() == 1 {
                    let elem_desc = self.resolve_json_schema_descriptor(&items[0])?;
                    Ok(format!("L{{{}}}", elem_desc))
                } else {
                    Err(LowerError {
                        message:
                            "List schema @[...] must have exactly one element type: @[TypeName]"
                                .to_string(),
                    })
                }
            }
            _ => Err(LowerError {
                message:
                    "JSON schema must be a type name (e.g., User, Int) or list type (e.g., @[User])"
                        .to_string(),
            }),
        }
    }

    /// Convert a TypeDef's field types to a schema descriptor string.
    ///
    /// `expanding` holds the TypeDefs being expanded around this one. A
    /// field that names one of them is written as `N{Name}` (a reference
    /// back to the enclosing `T{Name|...}`), so a self-referential type
    /// yields a finite descriptor.
    fn typedef_to_schema_descriptor(
        &self,
        type_name: &str,
        field_types: &[(String, Option<crate::parser::TypeExpr>)],
        expanding: &mut Vec<String>,
    ) -> Result<String, LowerError> {
        expanding.push(type_name.to_string());
        let mut parts = Vec::new();
        for (name, type_ann) in field_types {
            let type_desc = match type_ann {
                Some(te) => self.type_expr_to_schema_descriptor(te, expanding),
                None => Ok("s".to_string()), // default to Str
            };
            match type_desc {
                Ok(desc) => parts.push(format!("{}:{}", name, desc)),
                Err(e) => {
                    expanding.pop();
                    return Err(e);
                }
            }
        }
        expanding.pop();
        Ok(format!("T{{{}|{}}}", type_name, parts.join(",")))
    }

    /// Descriptor for a named TypeDef reached from inside another
    /// descriptor: `N{Name}` when it is already being expanded.
    fn named_typedef_descriptor(
        &self,
        type_name: &str,
        field_types: &[(String, Option<crate::parser::TypeExpr>)],
        expanding: &mut Vec<String>,
    ) -> Result<String, LowerError> {
        if expanding.iter().any(|n| n == type_name) {
            return Ok(format!("N{{{}}}", type_name));
        }
        self.typedef_to_schema_descriptor(type_name, field_types, expanding)
    }

    /// Format an Enum schema descriptor.
    ///
    /// Shape: `E{<EnumName>|<Variant1>,<Variant2>,...}`
    /// Empty variant list becomes `E{<EnumName>|}` and is treated as
    /// "no match possible" by the C runtime (always yields Lax[Enum]).
    fn enum_to_schema_descriptor(enum_name: &str, variants: &[String]) -> String {
        format!("E{{{}|{}}}", enum_name, variants.join(","))
    }

    /// Convert a TypeExpr to a schema descriptor string.
    fn type_expr_to_schema_descriptor(
        &self,
        type_expr: &crate::parser::TypeExpr,
        expanding: &mut Vec<String>,
    ) -> Result<String, LowerError> {
        match type_expr {
            crate::parser::TypeExpr::Named(name) => match name.as_str() {
                "Int" => Ok("i".to_string()),
                "Str" => Ok("s".to_string()),
                "Float" => Ok("f".to_string()),
                "Bool" => Ok("b".to_string()),
                "Bytes" => Ok("y".to_string()),
                "WebRequest" => Ok("Q".to_string()),
                "WebResponse" => Ok("R".to_string()),
                other => {
                    if let Some(field_types) = self.type_field_types.get(other) {
                        self.named_typedef_descriptor(other, field_types, expanding)
                    } else if let Some(variants) = self.enum_defs.get(other) {
                        Ok(Self::enum_to_schema_descriptor(other, variants))
                    } else {
                        Ok("s".to_string()) // Unknown type defaults to Str
                    }
                }
            },
            crate::parser::TypeExpr::List(inner) => {
                let elem_desc = self.type_expr_to_schema_descriptor(inner, expanding)?;
                Ok(format!("L{{{}}}", elem_desc))
            }
            crate::parser::TypeExpr::BuchiPack(fields) => {
                // Inline buchi pack type @(field: Type, ...)
                let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> = fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .map(|f| (f.name.clone(), f.type_annotation.clone()))
                    .collect();
                self.typedef_to_schema_descriptor("BuchiPack", &field_types, expanding)
            }
            _ => Ok("s".to_string()), // fallback to Str
        }
    }

    /// Resolve the wire schema for a runtime value expression. This is used
    /// for `HostStep` positional argument lists where Bytes and ABI web types
    /// must follow the boundary JSON shape instead of generic list/pack JSON.
    pub(crate) fn resolve_wire_schema_descriptor(&self, expr: &Expr) -> Result<String, LowerError> {
        if matches!(expr, Expr::ListLit(items, _) if items.is_empty()) {
            return Ok("L{a}".to_string());
        }
        match self.typed_expr_table.lookup(expr) {
            Some(ty) => self.type_to_schema_descriptor(ty, &mut Vec::new()),
            None => Ok("a".to_string()),
        }
    }

    fn type_to_schema_descriptor(
        &self,
        ty: &Type,
        expanding: &mut Vec<String>,
    ) -> Result<String, LowerError> {
        match ty {
            Type::Int => Ok("i".to_string()),
            Type::Float | Type::Num => Ok("f".to_string()),
            Type::Str => Ok("s".to_string()),
            Type::Bool => Ok("b".to_string()),
            Type::Bytes => Ok("y".to_string()),
            Type::List(inner) => {
                let elem_desc = self.type_to_schema_descriptor(inner, expanding)?;
                Ok(format!("L{{{}}}", elem_desc))
            }
            Type::BuchiPack(fields) => {
                let mut parts = Vec::new();
                for (name, field_ty) in fields {
                    parts.push(format!(
                        "{}:{}",
                        name,
                        self.type_to_schema_descriptor(field_ty, expanding)?
                    ));
                }
                Ok(format!("T{{BuchiPack|{}}}", parts.join(",")))
            }
            Type::Named(name) if name == "WebRequest" => Ok("Q".to_string()),
            Type::Named(name) if name == "WebResponse" => Ok("R".to_string()),
            Type::Named(name) => {
                if let Some(field_types) = self.type_field_types.get(name) {
                    self.named_typedef_descriptor(name, field_types, expanding)
                } else if let Some(variants) = self.enum_defs.get(name) {
                    Ok(Self::enum_to_schema_descriptor(name, variants))
                } else {
                    Ok("a".to_string())
                }
            }
            Type::Generic(name, args) if name == "HostCapability" && args.len() == 2 => {
                Ok("T{HostCapability|name:s,kind:s}".to_string())
            }
            Type::Any | Type::Unknown => Ok("a".to_string()),
            _ => Ok("a".to_string()),
        }
    }

    /// The descriptor of the checked type of `jsonEncode` / `jsonPretty`'s
    /// argument, in the JSON reader's format with `a` wherever the kind is
    /// not known statically. `None` when nothing about it is known.
    pub(crate) fn json_encode_descriptor(&self, expr: &Expr) -> Option<String> {
        let ty = self.typed_expr_table.lookup(expr)?;
        let desc = self.type_to_encode_descriptor(ty, &mut Vec::new());
        (desc != "a").then_some(desc)
    }

    fn type_to_encode_descriptor(&self, ty: &Type, expanding: &mut Vec<String>) -> String {
        match ty {
            Type::Int => "i".to_string(),
            Type::Float => "f".to_string(),
            Type::Str => "s".to_string(),
            Type::Bool => "b".to_string(),
            Type::Bytes => "y".to_string(),
            Type::List(inner) => {
                format!("L{{{}}}", self.type_to_encode_descriptor(inner, expanding))
            }
            Type::BuchiPack(fields) => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(name, field_ty)| {
                        format!(
                            "{}:{}",
                            name,
                            self.type_to_encode_descriptor(field_ty, expanding)
                        )
                    })
                    .collect();
                format!("T{{BuchiPack|{}}}", parts.join(","))
            }
            Type::Named(name) => self.named_encode_descriptor(name, expanding),
            // The payload of a Lax / Result / Gorillax is written with its
            // type; the other fields keep their own rendering.
            Type::Generic(name, args) if matches!(name.as_str(), "Lax" | "Result" | "Gorillax") => {
                let inner = args
                    .first()
                    .map(|arg| self.type_to_encode_descriptor(arg, expanding))
                    .unwrap_or_else(|| "a".to_string());
                if inner == "a" {
                    return inner;
                }
                if name == "Lax" {
                    format!("T{{Lax|__value:{inner},__default:{inner}}}")
                } else {
                    format!("T{{{name}|__value:{inner}}}")
                }
            }
            _ => "a".to_string(),
        }
    }

    fn type_expr_to_encode_descriptor(
        &self,
        type_expr: &crate::parser::TypeExpr,
        expanding: &mut Vec<String>,
    ) -> String {
        match type_expr {
            crate::parser::TypeExpr::Named(name) => match name.as_str() {
                "Int" => "i".to_string(),
                "Float" => "f".to_string(),
                "Str" => "s".to_string(),
                "Bool" => "b".to_string(),
                "Bytes" => "y".to_string(),
                other => self.named_encode_descriptor(other, expanding),
            },
            crate::parser::TypeExpr::List(inner) => {
                format!(
                    "L{{{}}}",
                    self.type_expr_to_encode_descriptor(inner, expanding)
                )
            }
            crate::parser::TypeExpr::BuchiPack(fields) => {
                let parts: Vec<String> = fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .map(|f| {
                        let desc = match &f.type_annotation {
                            Some(te) => self.type_expr_to_encode_descriptor(te, expanding),
                            None => "a".to_string(),
                        };
                        format!("{}:{}", f.name, desc)
                    })
                    .collect();
                format!("T{{BuchiPack|{}}}", parts.join(","))
            }
            _ => "a".to_string(),
        }
    }

    /// A TypeDef (`N{Name}` inside its own expansion) or an Enum; `a` for
    /// any other name.
    fn named_encode_descriptor(&self, name: &str, expanding: &mut Vec<String>) -> String {
        if let Some(field_types) = self.type_field_types.get(name) {
            if expanding.iter().any(|n| n == name) {
                return format!("N{{{}}}", name);
            }
            expanding.push(name.to_string());
            let parts: Vec<String> = field_types
                .iter()
                .map(|(field, annotation)| {
                    let desc = match annotation {
                        Some(te) => self.type_expr_to_encode_descriptor(te, expanding),
                        None => "a".to_string(),
                    };
                    format!("{}:{}", field, desc)
                })
                .collect();
            expanding.pop();
            format!("T{{{}|{}}}", name, parts.join(","))
        } else if let Some(variants) = self.enum_defs.get(name) {
            Self::enum_to_schema_descriptor(name, variants)
        } else {
            "a".to_string()
        }
    }

    /// Adds the field names of a schema descriptor to the names registered
    /// at startup. A pack the JSON reader builds for an inline pack type
    /// (`m: @(g: Float)`) otherwise shows and encodes without the fields
    /// whose names appear nowhere else in the program.
    pub(crate) fn note_json_schema_field_names(&mut self, desc: &str) {
        let bytes = desc.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'T' && bytes[i + 1] == b'{' {
                // T{Name|field:desc,...}: skip to the '|' after the name.
                i += 2;
                while i < bytes.len() && bytes[i] != b'|' && bytes[i] != b'}' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'|' {
                    i += 1;
                    self.note_json_field_names_in_body(desc, &mut i);
                }
            } else {
                i += 1;
            }
        }
    }

    /// Reads `field:desc,field:desc,...}` at `*i`, noting each field name
    /// and descending into nested TypeDefs.
    fn note_json_field_names_in_body(&mut self, desc: &str, i: &mut usize) {
        let bytes = desc.as_bytes();
        while *i < bytes.len() && bytes[*i] != b'}' {
            let start = *i;
            while *i < bytes.len() && bytes[*i] != b':' && bytes[*i] != b'}' {
                *i += 1;
            }
            if *i > start {
                self.field_names.insert(desc[start..*i].to_string());
            }
            if *i < bytes.len() && bytes[*i] == b':' {
                *i += 1;
            }
            // The field's descriptor runs to the next ',' / '}' at depth 0.
            let mut depth = 0usize;
            while *i < bytes.len() {
                match bytes[*i] {
                    b'{' => depth += 1,
                    b'}' if depth == 0 => break,
                    b'}' => depth -= 1,
                    b',' if depth == 0 => break,
                    b'T' if *i + 1 < bytes.len() && bytes[*i + 1] == b'{' => {
                        // Nested TypeDef: note its fields too.
                        *i += 2;
                        while *i < bytes.len() && bytes[*i] != b'|' && bytes[*i] != b'}' {
                            *i += 1;
                        }
                        if *i < bytes.len() && bytes[*i] == b'|' {
                            *i += 1;
                            self.note_json_field_names_in_body(desc, i);
                        }
                        // `*i` is at the nested TypeDef's closing '}'.
                    }
                    _ => {}
                }
                *i += 1;
            }
            if *i < bytes.len() && bytes[*i] == b',' {
                *i += 1;
            }
        }
    }

    /// TypeDefs with methods that `JSON[raw, schema]()` can build. The JSON
    /// reader only fills in the data fields, so each of these gets a
    /// rebuild function (see `json_rebuild_function`).
    fn json_schema_method_types(&self, schema: &Expr) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut found = Vec::new();
        self.collect_json_method_types_in_expr(schema, &mut seen, &mut found);
        found
    }

    fn collect_json_method_types_in_expr(
        &self,
        schema: &Expr,
        seen: &mut std::collections::HashSet<String>,
        found: &mut Vec<String>,
    ) {
        match schema {
            Expr::Ident(name, _) => self.collect_json_method_types_in_type(name, seen, found),
            Expr::ListLit(items, _) if items.len() == 1 => {
                self.collect_json_method_types_in_expr(&items[0], seen, found)
            }
            _ => {}
        }
    }

    fn collect_json_method_types_in_type(
        &self,
        type_name: &str,
        seen: &mut std::collections::HashSet<String>,
        found: &mut Vec<String>,
    ) {
        if !seen.insert(type_name.to_string()) {
            return;
        }
        let Some(field_types) = self.type_field_types.get(type_name) else {
            return;
        };
        if self.type_method_defs.contains_key(type_name) {
            found.push(type_name.to_string());
        }
        for (_, type_ann) in field_types {
            if let Some(te) = type_ann {
                self.collect_json_method_types_in_type_expr(te, seen, found);
            }
        }
    }

    fn collect_json_method_types_in_type_expr(
        &self,
        type_expr: &crate::parser::TypeExpr,
        seen: &mut std::collections::HashSet<String>,
        found: &mut Vec<String>,
    ) {
        match type_expr {
            crate::parser::TypeExpr::Named(name) => {
                self.collect_json_method_types_in_type(name, seen, found)
            }
            crate::parser::TypeExpr::List(inner) => {
                self.collect_json_method_types_in_type_expr(inner, seen, found)
            }
            crate::parser::TypeExpr::BuchiPack(fields) => {
                for field in fields.iter().filter(|f| !f.is_method) {
                    if let Some(te) = &field.type_annotation {
                        self.collect_json_method_types_in_type_expr(te, seen, found);
                    }
                }
            }
            _ => {}
        }
    }

    /// The rebuild hooks for `JSON[raw, schema]()`: a list of
    /// `[TypeName, rebuild function, ...]` for every TypeDef with methods
    /// the schema can build, or `None` when there is none.
    pub(crate) fn lower_json_rebuild_hooks(
        &mut self,
        func: &mut IrFunction,
        schema: &Expr,
    ) -> Result<Option<IrVar>, LowerError> {
        let types = self.json_schema_method_types(schema);
        if types.is_empty() {
            return Ok(None);
        }
        let mut list = func.alloc_var();
        func.push(IrInst::Call(list, "taida_list_new".to_string(), vec![]));
        for type_name in types {
            let symbol = self.json_rebuild_function(&type_name)?;
            self.publish_json_rebuild_globals(func, &type_name);
            let name_var = func.alloc_var();
            func.push(IrInst::ConstStr(name_var, type_name));
            let with_name = func.alloc_var();
            func.push(IrInst::Call(
                with_name,
                "taida_list_push".to_string(),
                vec![list, name_var],
            ));
            let fn_var = func.alloc_var();
            func.push(IrInst::FuncAddr(fn_var, symbol));
            let with_fn = func.alloc_var();
            func.push(IrInst::Call(
                with_fn,
                "taida_list_push".to_string(),
                vec![with_name, fn_var],
            ));
            list = with_fn;
        }
        Ok(Some(list))
    }

    /// The rebuild function reads the top-level values the methods use
    /// from the global table. A top-level binding is only written there
    /// when a function is known to read it, which a binding lowered before
    /// this JSON call could not know, so a top-level call site writes the
    /// current values itself.
    fn publish_json_rebuild_globals(&mut self, func: &mut IrFunction, type_name: &str) {
        if self.current_func_name.is_some() || self.is_library_module {
            return;
        }
        let data_field_names: Vec<String> = self
            .type_field_defs
            .get(type_name)
            .map(|defs| {
                defs.iter()
                    .filter(|f| !f.is_method)
                    .map(|f| f.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let methods = self
            .type_method_defs
            .get(type_name)
            .cloned()
            .unwrap_or_default();
        let defined = Self::collect_defvar_names(&func.body);
        let mut published: Vec<String> = Vec::new();
        for (_, method) in &methods {
            for name in self.type_method_extra_captures(method, &data_field_names) {
                if published.contains(&name)
                    || !defined.contains(&name)
                    || self.imported_value_names.contains(&name)
                {
                    continue;
                }
                let value = func.alloc_var();
                func.push(IrInst::UseVar(value, name.clone()));
                let hash = self.global_var_hash(&name);
                func.push(IrInst::GlobalSet(hash, value));
                published.push(name);
            }
        }
    }

    /// A function that turns the data-only pack the JSON reader built for
    /// `type_name` into the value `TypeName(field <= ..., ...)` builds from
    /// the same field values, so its methods are callable. Generated once
    /// per module.
    fn json_rebuild_function(&mut self, type_name: &str) -> Result<String, LowerError> {
        if let Some(symbol) = self.json_rebuild_fns.get(type_name) {
            return Ok(symbol.clone());
        }
        let symbol = self.next_lambda_symbol(&format!("json_rebuild_{}", type_name));
        self.json_rebuild_fns
            .insert(type_name.to_string(), symbol.clone());

        let mut rebuild =
            IrFunction::new_with_params(symbol.clone(), vec!["__json_data".to_string()]);
        let data_var: IrVar = 0; // the only parameter
        let span =
            crate::lexer::Span::new(0, 0, 0, 0).with_node_id(crate::parser::SYNTHETIC_NODE_ID_BASE);
        let field_defs = self
            .type_field_defs
            .get(type_name)
            .cloned()
            .unwrap_or_default();
        let mut fields = Vec::new();
        for (i, field_def) in field_defs.iter().filter(|f| !f.is_method).enumerate() {
            let hash_var = rebuild.alloc_var();
            rebuild.push(IrInst::ConstInt(
                hash_var,
                simple_hash(&field_def.name) as i64,
            ));
            let value = rebuild.alloc_var();
            rebuild.push(IrInst::Call(
                value,
                "taida_pack_get".to_string(),
                vec![data_var, hash_var],
            ));
            let local = format!("{HIDDEN}json_field_{i}");
            rebuild.push(IrInst::DefVar(local.clone(), value));
            fields.push(BuchiField {
                name: field_def.name.clone(),
                value: Expr::Ident(local, span.clone()),
                span: span.clone(),
            });
        }

        // Lower like a function body: top-level values the methods use are
        // read through the global table.
        let prev_heap = std::mem::take(&mut self.current_heap_vars);
        let prev_func_name = self.current_func_name.replace(symbol.clone());
        let built = self.lower_type_inst(&mut rebuild, type_name, &fields);
        self.current_func_name = prev_func_name;
        self.current_heap_vars = prev_heap;
        let pack = built?;
        rebuild.push(IrInst::Return(pack));

        self.user_funcs.insert(symbol.clone());
        self.lambda_funcs.push(rebuild);
        Ok(symbol)
    }
}

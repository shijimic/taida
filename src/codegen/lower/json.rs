use super::{LowerError, Lowering};
/// JSON schema resolution for the Taida native backend.
///
/// Contains `resolve_json_schema_descriptor`, `typedef_to_schema_descriptor`,
/// and `type_expr_to_schema_descriptor`.
///
/// These are `impl Lowering` methods split from lower.rs for maintainability.
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
                        self.typedef_to_schema_descriptor(type_name, field_types)
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
    fn typedef_to_schema_descriptor(
        &self,
        type_name: &str,
        field_types: &[(String, Option<crate::parser::TypeExpr>)],
    ) -> Result<String, LowerError> {
        let mut parts = Vec::new();
        for (name, type_ann) in field_types {
            let type_desc = match type_ann {
                Some(te) => self.type_expr_to_schema_descriptor(te)?,
                None => "s".to_string(), // default to Str
            };
            parts.push(format!("{}:{}", name, type_desc));
        }
        Ok(format!("T{{{}|{}}}", type_name, parts.join(",")))
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
                        self.typedef_to_schema_descriptor(other, field_types)
                    } else if let Some(variants) = self.enum_defs.get(other) {
                        Ok(Self::enum_to_schema_descriptor(other, variants))
                    } else {
                        Ok("s".to_string()) // Unknown type defaults to Str
                    }
                }
            },
            crate::parser::TypeExpr::List(inner) => {
                let elem_desc = self.type_expr_to_schema_descriptor(inner)?;
                Ok(format!("L{{{}}}", elem_desc))
            }
            crate::parser::TypeExpr::BuchiPack(fields) => {
                // Inline buchi pack type @(field: Type, ...)
                let field_types: Vec<(String, Option<crate::parser::TypeExpr>)> = fields
                    .iter()
                    .filter(|f| !f.is_method)
                    .map(|f| (f.name.clone(), f.type_annotation.clone()))
                    .collect();
                self.typedef_to_schema_descriptor("BuchiPack", &field_types)
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
            Some(ty) => self.type_to_schema_descriptor(ty),
            None => Ok("a".to_string()),
        }
    }

    fn type_to_schema_descriptor(&self, ty: &Type) -> Result<String, LowerError> {
        match ty {
            Type::Int => Ok("i".to_string()),
            Type::Float | Type::Num => Ok("f".to_string()),
            Type::Str => Ok("s".to_string()),
            Type::Bool => Ok("b".to_string()),
            Type::Bytes => Ok("y".to_string()),
            Type::List(inner) => {
                let elem_desc = self.type_to_schema_descriptor(inner)?;
                Ok(format!("L{{{}}}", elem_desc))
            }
            Type::BuchiPack(fields) => {
                let mut parts = Vec::new();
                for (name, field_ty) in fields {
                    parts.push(format!(
                        "{}:{}",
                        name,
                        self.type_to_schema_descriptor(field_ty)?
                    ));
                }
                Ok(format!("T{{BuchiPack|{}}}", parts.join(",")))
            }
            Type::Named(name) if name == "WebRequest" => Ok("Q".to_string()),
            Type::Named(name) if name == "WebResponse" => Ok("R".to_string()),
            Type::Named(name) => {
                if let Some(field_types) = self.type_field_types.get(name) {
                    self.typedef_to_schema_descriptor(name, field_types)
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
}

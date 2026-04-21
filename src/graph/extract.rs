//! AST-to-graph extraction for all 5 graph views.
//!
//! Because Taida has only 10 operators with unambiguous semantics,
//! graph extraction is deterministic — no type inference or control
//! flow analysis required.

use std::collections::HashMap;

use super::model::*;
use crate::parser::*;

/// Extractor state.
pub struct GraphExtractor {
    file: String,
    /// Counter for generating unique anonymous function IDs.
    lambda_counter: usize,
}

impl GraphExtractor {
    pub fn new(file: &str) -> Self {
        Self {
            file: file.to_string(),
            lambda_counter: 0,
        }
    }

    // ── Dataflow Graph ──────────────────────────────────

    /// Extract a dataflow graph from a program.
    pub fn extract_dataflow(&mut self, program: &Program) -> Graph {
        let mut graph = Graph::new(GraphView::Dataflow);
        graph.source_files.push(self.file.clone());

        for stmt in &program.statements {
            self.extract_dataflow_stmt(&mut graph, stmt);
        }
        graph
    }

    fn extract_dataflow_stmt(&mut self, graph: &mut Graph, stmt: &Statement) {
        match stmt {
            Statement::Assignment(assign) => {
                let span = &assign.span;
                let var_id = self.add_variable_node(graph, &assign.target, span.line, span.column);
                self.extract_dataflow_expr_to(
                    graph,
                    &assign.value,
                    &var_id,
                    EdgeKind::PipeBackward,
                );
            }

            Statement::FuncDef(fd) => {
                // Extract dataflow within function body
                for body_stmt in &fd.body {
                    self.extract_dataflow_stmt(graph, body_stmt);
                }
            }

            Statement::UnmoldForward(uf) => {
                let span = &uf.span;
                let target_id = self.add_variable_node(graph, &uf.target, span.line, span.column);
                let source_id = self.extract_dataflow_expr(graph, &uf.source);
                if let Some(src) = source_id {
                    graph.add_edge(GraphEdge {
                        source: src,
                        target: target_id,
                        kind: EdgeKind::UnmoldForward,
                        label: "]=>".to_string(),
                        metadata: HashMap::new(),
                    });
                }
            }

            Statement::UnmoldBackward(ub) => {
                let span = &ub.span;
                let target_id = self.add_variable_node(graph, &ub.target, span.line, span.column);
                let source_id = self.extract_dataflow_expr(graph, &ub.source);
                if let Some(src) = source_id {
                    graph.add_edge(GraphEdge {
                        source: src,
                        target: target_id,
                        kind: EdgeKind::UnmoldBackward,
                        label: "<=[".to_string(),
                        metadata: HashMap::new(),
                    });
                }
            }

            Statement::Expr(expr) => {
                self.extract_dataflow_expr(graph, expr);
            }

            Statement::ErrorCeiling(ec) => {
                for body_stmt in &ec.handler_body {
                    self.extract_dataflow_stmt(graph, body_stmt);
                }
            }

            _ => {}
        }
    }

    /// Extract dataflow from an expression. Returns the node ID of the expression.
    fn extract_dataflow_expr(&mut self, graph: &mut Graph, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident(name, span) => {
                Some(self.add_variable_node(graph, name, span.line, span.column))
            }

            Expr::EnumVariant(enum_name, variant_name, span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Literal);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Literal,
                    label: format!("{}:{}()", enum_name, variant_name),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::IntLit(n, span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Literal);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Literal,
                    label: n.to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::FloatLit(n, span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Literal);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Literal,
                    label: n.to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::StringLit(s, span) | Expr::TemplateLit(s, span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Literal);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Literal,
                    label: format!("\"{}\"", s),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::BoolLit(b, span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Literal);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Literal,
                    label: b.to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::FuncCall(callee, args, span) => {
                let label = if let Expr::Ident(name, _) = callee.as_ref() {
                    format!("{}(...)", name)
                } else {
                    "call(...)".to_string()
                };
                let id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::FunctionCall);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::FunctionCall,
                    label,
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });

                // Argument edges
                for arg in args {
                    if let Some(arg_id) = self.extract_dataflow_expr(graph, arg) {
                        graph.add_edge(GraphEdge {
                            source: arg_id,
                            target: id.clone(),
                            kind: EdgeKind::Argument,
                            label: "arg".to_string(),
                            metadata: HashMap::new(),
                        });
                    }
                }
                Some(id)
            }

            Expr::MethodCall(obj, method, args, span) => {
                let id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::FunctionCall);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::FunctionCall,
                    label: format!(".{}(...)", method),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });

                if let Some(obj_id) = self.extract_dataflow_expr(graph, obj) {
                    graph.add_edge(GraphEdge {
                        source: obj_id,
                        target: id.clone(),
                        kind: EdgeKind::Argument,
                        label: "self".to_string(),
                        metadata: HashMap::new(),
                    });
                }

                for arg in args {
                    if let Some(arg_id) = self.extract_dataflow_expr(graph, arg) {
                        graph.add_edge(GraphEdge {
                            source: arg_id,
                            target: id.clone(),
                            kind: EdgeKind::Argument,
                            label: "arg".to_string(),
                            metadata: HashMap::new(),
                        });
                    }
                }
                Some(id)
            }

            Expr::Pipeline(exprs, span) => {
                let mut prev_id: Option<String> = None;
                for (i, e) in exprs.iter().enumerate() {
                    let cur_id = self.extract_dataflow_expr(graph, e);
                    if i > 0
                        && let (Some(prev), Some(cur)) = (&prev_id, &cur_id)
                    {
                        graph.add_edge(GraphEdge {
                            source: prev.clone(),
                            target: cur.clone(),
                            kind: EdgeKind::PipeForward,
                            label: "=>".to_string(),
                            metadata: HashMap::new(),
                        });
                    }
                    prev_id = cur_id;
                }
                // Return the ID of the last in the pipeline
                if prev_id.is_some() {
                    prev_id
                } else {
                    let id =
                        Graph::make_id(&self.file, span.line, span.column, &NodeKind::Variable);
                    Some(id)
                }
            }

            Expr::BuchiPack(fields, span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::BuchiPack);
                let field_names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::BuchiPack,
                    label: format!("@({})", field_names.join(", ")),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::ListLit(_, span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Literal);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Literal,
                    label: "@[...]".to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::MoldInst(name, _, _, span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Unmold);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Unmold,
                    label: format!("{}[...]", name),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::CondBranch(arms, span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Condition);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Condition,
                    label: "| ... |>".to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });

                for arm in arms {
                    let body_id = arm
                        .last_expr()
                        .and_then(|e| self.extract_dataflow_expr(graph, e));
                    if let Some(bid) = body_id {
                        let edge_kind = if arm.condition.is_some() {
                            EdgeKind::ConditionTrue
                        } else {
                            EdgeKind::ConditionFalse
                        };
                        graph.add_edge(GraphEdge {
                            source: id.clone(),
                            target: bid,
                            kind: edge_kind,
                            label: "|>".to_string(),
                            metadata: HashMap::new(),
                        });
                    }
                }
                Some(id)
            }

            Expr::BinaryOp(left, _, right, span) => {
                let left_id = self.extract_dataflow_expr(graph, left);
                let right_id = self.extract_dataflow_expr(graph, right);
                // Return the span-based ID for this operation
                let id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::FunctionCall);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::FunctionCall,
                    label: "op".to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                if let Some(lid) = left_id {
                    graph.add_edge(GraphEdge {
                        source: lid,
                        target: id.clone(),
                        kind: EdgeKind::Argument,
                        label: "left".to_string(),
                        metadata: HashMap::new(),
                    });
                }
                if let Some(rid) = right_id {
                    graph.add_edge(GraphEdge {
                        source: rid,
                        target: id.clone(),
                        kind: EdgeKind::Argument,
                        label: "right".to_string(),
                        metadata: HashMap::new(),
                    });
                }
                Some(id)
            }

            Expr::Gorilla(span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Literal);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Literal,
                    label: "><".to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::Placeholder(span) => {
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Placeholder);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Placeholder,
                    label: "_".to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                Some(id)
            }

            Expr::UnaryOp(op, inner, span) => {
                let inner_id = self.extract_dataflow_expr(graph, inner);
                let id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::FunctionCall);
                let label = match op {
                    UnaryOp::Not => "unary_not",
                    UnaryOp::Neg => "unary_neg",
                };
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::FunctionCall,
                    label: label.to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                if let Some(iid) = inner_id {
                    graph.add_edge(GraphEdge {
                        source: iid,
                        target: id.clone(),
                        kind: EdgeKind::Argument,
                        label: "operand".to_string(),
                        metadata: HashMap::new(),
                    });
                }
                Some(id)
            }

            Expr::FieldAccess(obj, field, span) => {
                let obj_id = self.extract_dataflow_expr(graph, obj);
                let id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::FunctionCall);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::FunctionCall,
                    label: format!(".{}", field),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                if let Some(oid) = obj_id {
                    graph.add_edge(GraphEdge {
                        source: oid,
                        target: id.clone(),
                        kind: EdgeKind::Argument,
                        label: "self".to_string(),
                        metadata: HashMap::new(),
                    });
                }
                Some(id)
            }

            Expr::Unmold(inner, span) => {
                let inner_id = self.extract_dataflow_expr(graph, inner);
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Unmold);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Unmold,
                    label: "]=>".to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                if let Some(iid) = inner_id {
                    graph.add_edge(GraphEdge {
                        source: iid,
                        target: id.clone(),
                        kind: EdgeKind::UnmoldForward,
                        label: "]=>".to_string(),
                        metadata: HashMap::new(),
                    });
                }
                Some(id)
            }

            Expr::Lambda(params, body, span) => {
                self.lambda_counter += 1;
                let base_id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::AnonymousFn);
                let id = format!("{}:{}", base_id, self.lambda_counter);
                let param_names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::AnonymousFn,
                    label: format!("_ {} = ...", param_names.join(" ")),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                // Recurse into the lambda body
                self.extract_dataflow_expr(graph, body);
                Some(id)
            }

            Expr::TypeInst(name, fields, span) => {
                let id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::FunctionCall);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::FunctionCall,
                    label: format!("{}(...)", name),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                for field in fields {
                    if let Some(fid) = self.extract_dataflow_expr(graph, &field.value) {
                        graph.add_edge(GraphEdge {
                            source: fid,
                            target: id.clone(),
                            kind: EdgeKind::Argument,
                            label: field.name.clone(),
                            metadata: HashMap::new(),
                        });
                    }
                }
                Some(id)
            }

            Expr::Throw(inner, span) => {
                let inner_id = self.extract_dataflow_expr(graph, inner);
                let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::ThrowSite);
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::ThrowSite,
                    label: ".throw()".to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                if let Some(iid) = inner_id {
                    graph.add_edge(GraphEdge {
                        source: iid,
                        target: id.clone(),
                        kind: EdgeKind::Argument,
                        label: "error".to_string(),
                        metadata: HashMap::new(),
                    });
                }
                Some(id)
            }

            // Hole is a partial application marker — not a dataflow node
            Expr::Hole(_) => None,

            // B11-6a: TypeLiteral is a compile-time construct, not a dataflow node
            Expr::TypeLiteral(_, _, _) => None,
        }
    }

    /// Add a PipeBackward edge from an expression to a target node.
    fn extract_dataflow_expr_to(
        &mut self,
        graph: &mut Graph,
        expr: &Expr,
        target_id: &str,
        edge_kind: EdgeKind,
    ) {
        if let Some(source_id) = self.extract_dataflow_expr(graph, expr) {
            graph.add_edge(GraphEdge {
                source: source_id,
                target: target_id.to_string(),
                kind: edge_kind,
                label: "<=".to_string(),
                metadata: HashMap::new(),
            });
        }
    }

    fn add_variable_node(&self, graph: &mut Graph, name: &str, line: usize, col: usize) -> String {
        let id = Graph::make_id(&self.file, line, col, &NodeKind::Variable);
        graph.add_node(GraphNode {
            id: id.clone(),
            kind: NodeKind::Variable,
            label: name.to_string(),
            location: Location {
                file: self.file.clone(),
                line,
                column: col,
            },
            metadata: HashMap::new(),
        });
        id
    }

    // ── Module Dependency Graph ─────────────────────────

    /// Extract a module dependency graph from a program.
    pub fn extract_module(&mut self, program: &Program) -> Graph {
        let mut graph = Graph::new(GraphView::Module);
        graph.source_files.push(self.file.clone());

        // Add current module node
        let module_id = Graph::make_id(&self.file, 0, 0, &NodeKind::Module);
        graph.add_node(GraphNode {
            id: module_id.clone(),
            kind: NodeKind::Module,
            label: self.file.clone(),
            location: Location {
                file: self.file.clone(),
                line: 0,
                column: 0,
            },
            metadata: HashMap::new(),
        });

        for stmt in &program.statements {
            match stmt {
                Statement::Import(import) => {
                    let span = &import.span;
                    let _import_id = Graph::make_id(&import.path, 0, 0, &NodeKind::Module);
                    let is_external = !import.path.starts_with("./")
                        && !import.path.starts_with("../")
                        && !import.path.starts_with("~/")
                        && !import.path.starts_with('/');

                    let kind = if is_external {
                        NodeKind::ExternalPackage
                    } else {
                        NodeKind::Module
                    };
                    let import_node_id = Graph::make_id(&import.path, 0, 0, &kind);

                    graph.add_node(GraphNode {
                        id: import_node_id.clone(),
                        kind,
                        label: import.path.clone(),
                        location: Location {
                            file: self.file.clone(),
                            line: span.line,
                            column: span.column,
                        },
                        metadata: HashMap::new(),
                    });

                    let sym_names: Vec<String> =
                        import.symbols.iter().map(|s| s.name.clone()).collect();
                    let mut edge_meta = HashMap::new();
                    edge_meta.insert("symbols".to_string(), sym_names.join(", "));

                    graph.add_edge(GraphEdge {
                        source: module_id.clone(),
                        target: import_node_id.clone(),
                        kind: EdgeKind::Imports,
                        label: format!(">>> {}", import.path),
                        metadata: edge_meta,
                    });

                    // Add symbol nodes
                    for sym in &import.symbols {
                        let sym_id = format!("{}:symbol:{}", import.path, sym.name);
                        graph.add_node(GraphNode {
                            id: sym_id.clone(),
                            kind: NodeKind::Symbol,
                            label: sym.name.clone(),
                            location: Location {
                                file: self.file.clone(),
                                line: span.line,
                                column: span.column,
                            },
                            metadata: HashMap::new(),
                        });
                    }
                }

                Statement::Export(export) => {
                    let span = &export.span;
                    for sym_name in &export.symbols {
                        let sym_id = format!("{}:export:{}", self.file, sym_name);
                        graph.add_node(GraphNode {
                            id: sym_id.clone(),
                            kind: NodeKind::Symbol,
                            label: sym_name.clone(),
                            location: Location {
                                file: self.file.clone(),
                                line: span.line,
                                column: span.column,
                            },
                            metadata: HashMap::new(),
                        });
                        graph.add_edge(GraphEdge {
                            source: module_id.clone(),
                            target: sym_id,
                            kind: EdgeKind::Exports,
                            label: format!("<<< {}", sym_name),
                            metadata: HashMap::new(),
                        });
                    }
                }

                _ => {}
            }
        }

        graph
    }

    // ── Type Hierarchy Graph ────────────────────────────

    /// Extract a type hierarchy graph from a program.
    pub fn extract_type_hierarchy(&mut self, program: &Program) -> Graph {
        let mut graph = Graph::new(GraphView::TypeHierarchy);
        graph.source_files.push(self.file.clone());

        for stmt in &program.statements {
            match stmt {
                Statement::TypeDef(td) => {
                    let span = &td.span;
                    let id = Graph::make_id(
                        &self.file,
                        span.line,
                        span.column,
                        &NodeKind::BuchiPackType,
                    );
                    graph.add_node(GraphNode {
                        id,
                        kind: NodeKind::BuchiPackType,
                        label: td.name.clone(),
                        location: Location {
                            file: self.file.clone(),
                            line: span.line,
                            column: span.column,
                        },
                        metadata: HashMap::new(),
                    });
                }

                Statement::MoldDef(md) => {
                    let span = &md.span;
                    let id =
                        Graph::make_id(&self.file, span.line, span.column, &NodeKind::MoldType);
                    graph.add_node(GraphNode {
                        id: id.clone(),
                        kind: NodeKind::MoldType,
                        label: md.name.clone(),
                        location: Location {
                            file: self.file.clone(),
                            line: span.line,
                            column: span.column,
                        },
                        metadata: HashMap::new(),
                    });

                    // MoldInheritance edge from Mold[T] to this type
                    let mold_base_id = "builtin:Mold".to_string();
                    graph.add_node(GraphNode {
                        id: mold_base_id.clone(),
                        kind: NodeKind::MoldType,
                        label: "Mold[T]".to_string(),
                        location: Location {
                            file: "builtin".to_string(),
                            line: 0,
                            column: 0,
                        },
                        metadata: HashMap::new(),
                    });
                    graph.add_edge(GraphEdge {
                        source: mold_base_id,
                        target: id,
                        kind: EdgeKind::MoldInheritance,
                        label: "Mold[T] =>".to_string(),
                        metadata: HashMap::new(),
                    });
                }

                Statement::InheritanceDef(inh) => {
                    let span = &inh.span;

                    // Check if this is Error inheritance
                    let (parent_kind, child_kind, edge_kind) = if inh.parent == "Error" {
                        (
                            NodeKind::ErrorType,
                            NodeKind::ErrorType,
                            EdgeKind::ErrorInheritance,
                        )
                    } else {
                        (
                            NodeKind::BuchiPackType,
                            NodeKind::BuchiPackType,
                            EdgeKind::StructuralSubtype,
                        )
                    };

                    let parent_id = format!("type:{}", inh.parent);
                    graph.add_node(GraphNode {
                        id: parent_id.clone(),
                        kind: parent_kind,
                        label: inh.parent.clone(),
                        location: Location {
                            file: self.file.clone(),
                            line: span.line,
                            column: span.column,
                        },
                        metadata: HashMap::new(),
                    });

                    let child_id = Graph::make_id(&self.file, span.line, span.column, &child_kind);
                    graph.add_node(GraphNode {
                        id: child_id.clone(),
                        kind: child_kind,
                        label: inh.child.clone(),
                        location: Location {
                            file: self.file.clone(),
                            line: span.line,
                            column: span.column,
                        },
                        metadata: HashMap::new(),
                    });

                    graph.add_edge(GraphEdge {
                        source: parent_id,
                        target: child_id,
                        kind: edge_kind,
                        label: format!("{} => {}", inh.parent, inh.child),
                        metadata: HashMap::new(),
                    });
                }

                _ => {}
            }
        }

        graph
    }

    // ── Error Boundary Graph ────────────────────────────

    /// Extract an error boundary graph from a program.
    pub fn extract_error(&mut self, program: &Program) -> Graph {
        let mut graph = Graph::new(GraphView::Error);
        graph.source_files.push(self.file.clone());

        // Collect function names defined in this program (for cross-function propagation)
        let func_names: std::collections::HashSet<String> = program
            .statements
            .iter()
            .filter_map(|stmt| {
                if let Statement::FuncDef(fd) = stmt {
                    Some(fd.name.clone())
                } else {
                    None
                }
            })
            .collect();

        // Pass 1: Extract error nodes and edges (existing behavior)
        self.extract_error_stmts(&mut graph, &program.statements, None, &func_names);

        graph
    }

    /// Extract error-related nodes and edges from a list of statements.
    ///
    /// `current_ceiling`: the active error ceiling ID (if any) covering this scope.
    /// `current_func`: the Function node ID of the enclosing function (if any).
    /// `func_names`: set of all function names defined in the program (for Propagates edges).
    fn extract_error_stmts(
        &mut self,
        graph: &mut Graph,
        stmts: &[Statement],
        current_ceiling: Option<&str>,
        func_names: &std::collections::HashSet<String>,
    ) {
        self.extract_error_stmts_inner(graph, stmts, current_ceiling, None, func_names);
    }

    fn extract_error_stmts_inner(
        &mut self,
        graph: &mut Graph,
        stmts: &[Statement],
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
        func_names: &std::collections::HashSet<String>,
    ) {
        let mut active_ceiling: Option<String> = current_ceiling.map(|s| s.to_string());

        for stmt in stmts {
            match stmt {
                Statement::ErrorCeiling(ec) => {
                    let span = &ec.span;
                    let ceiling_id =
                        Graph::make_id(&self.file, span.line, span.column, &NodeKind::ErrorCeiling);
                    let type_label = match &ec.error_type {
                        TypeExpr::Named(n) => n.clone(),
                        _ => "Error".to_string(),
                    };
                    graph.add_node(GraphNode {
                        id: ceiling_id.clone(),
                        kind: NodeKind::ErrorCeiling,
                        label: format!("|== {}: {}", ec.error_param, type_label),
                        location: Location {
                            file: self.file.clone(),
                            line: span.line,
                            column: span.column,
                        },
                        metadata: HashMap::new(),
                    });

                    // Handler body
                    self.extract_error_stmts_inner(
                        graph,
                        &ec.handler_body,
                        Some(&ceiling_id),
                        current_func,
                        func_names,
                    );

                    active_ceiling = Some(ceiling_id);
                }

                Statement::FuncDef(fd) => {
                    let span = &fd.span;
                    let fn_id =
                        Graph::make_id(&self.file, span.line, span.column, &NodeKind::Function);
                    graph.add_node(GraphNode {
                        id: fn_id.clone(),
                        kind: NodeKind::Function,
                        label: fd.name.clone(),
                        location: Location {
                            file: self.file.clone(),
                            line: span.line,
                            column: span.column,
                        },
                        metadata: HashMap::new(),
                    });

                    // Recurse into function body: ceiling resets to None, current_func becomes this function
                    self.extract_error_stmts_inner(graph, &fd.body, None, Some(&fn_id), func_names);
                }

                Statement::Expr(expr) => {
                    self.extract_error_expr(
                        graph,
                        expr,
                        active_ceiling.as_deref(),
                        current_func,
                        func_names,
                    );
                }

                Statement::UnmoldForward(uf) => {
                    self.extract_error_expr(
                        graph,
                        &uf.source,
                        active_ceiling.as_deref(),
                        current_func,
                        func_names,
                    );
                }

                Statement::UnmoldBackward(ub) => {
                    self.extract_error_expr(
                        graph,
                        &ub.source,
                        active_ceiling.as_deref(),
                        current_func,
                        func_names,
                    );
                }

                Statement::Assignment(assign) => {
                    self.extract_error_expr(
                        graph,
                        &assign.value,
                        active_ceiling.as_deref(),
                        current_func,
                        func_names,
                    );
                }

                _ => {}
            }
        }
    }

    /// Extract error-related nodes and edges from an expression.
    ///
    /// `current_ceiling`: the active error ceiling ID (if any) covering this scope.
    /// `current_func`: the Function node ID of the enclosing function (if any).
    /// `func_names`: set of all function names defined in the program (for Propagates edges).
    ///
    /// Propagates edge semantics:
    /// - `ErrorCeiling -> Function`: the ceiling covers the callee's throws (cross-function coverage)
    /// - `Function(caller) -> Function(callee)`: callee's throws propagate to caller
    ///   (allows transitive coverage: if caller is covered by a ceiling, callee is too)
    fn extract_error_expr(
        &mut self,
        graph: &mut Graph,
        expr: &Expr,
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
        func_names: &std::collections::HashSet<String>,
    ) {
        match expr {
            Expr::Throw(inner, span) => {
                let throw_label = if let Expr::FuncCall(callee, _, _) = inner.as_ref() {
                    if let Expr::Ident(name, _) = callee.as_ref() {
                        name.clone()
                    } else {
                        "unknown".to_string()
                    }
                } else {
                    "throw".to_string()
                };

                let throw_id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::ThrowSite);
                let mut metadata = HashMap::new();
                if let Some(func_id) = current_func {
                    metadata.insert("enclosing_function".to_string(), func_id.to_string());
                }
                graph.add_node(GraphNode {
                    id: throw_id.clone(),
                    kind: NodeKind::ThrowSite,
                    label: format!("{}.throw()", throw_label),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata,
                });

                if let Some(ceiling_id) = current_ceiling {
                    graph.add_edge(GraphEdge {
                        source: throw_id,
                        target: ceiling_id.to_string(),
                        kind: EdgeKind::ThrowsTo,
                        label: "throws to".to_string(),
                        metadata: HashMap::new(),
                    });
                }
                // If no ceiling, it propagates to gorilla ceiling (uncovered)
            }

            Expr::MethodCall(obj, method, _, span) if method == "throw" => {
                let throw_id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::ThrowSite);
                let mut metadata = HashMap::new();
                if let Some(func_id) = current_func {
                    metadata.insert("enclosing_function".to_string(), func_id.to_string());
                }
                graph.add_node(GraphNode {
                    id: throw_id.clone(),
                    kind: NodeKind::ThrowSite,
                    label: ".throw()".to_string(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata,
                });

                if let Some(ceiling_id) = current_ceiling {
                    graph.add_edge(GraphEdge {
                        source: throw_id,
                        target: ceiling_id.to_string(),
                        kind: EdgeKind::ThrowsTo,
                        label: "throws to".to_string(),
                        metadata: HashMap::new(),
                    });
                }
            }

            Expr::FuncCall(callee, args, _) => {
                // Generate Propagates edges for cross-function error propagation.
                if let Expr::Ident(name, _) = callee.as_ref()
                    && func_names.contains(name)
                {
                    let callee_fn_id = graph
                        .nodes
                        .iter()
                        .find(|n| n.kind == NodeKind::Function && n.label == *name)
                        .map(|n| n.id.clone());

                    if let Some(callee_id) = callee_fn_id {
                        if let Some(ceiling_id) = current_ceiling {
                            // Call site is under a ceiling: ceiling covers callee's throws
                            graph.add_edge(GraphEdge {
                                source: ceiling_id.to_string(),
                                target: callee_id,
                                kind: EdgeKind::Propagates,
                                label: format!("covers {}", name),
                                metadata: HashMap::new(),
                            });
                        } else if let Some(caller_id) = current_func {
                            // Call site is inside a function but without ceiling:
                            // callee's throws propagate to caller
                            graph.add_edge(GraphEdge {
                                source: caller_id.to_string(),
                                target: callee_id,
                                kind: EdgeKind::Propagates,
                                label: format!("propagates from {}", name),
                                metadata: HashMap::new(),
                            });
                        }
                        // Top-level call without ceiling: callee's uncovered throws
                        // remain uncovered (no edge needed).
                    }
                }

                for arg in args {
                    self.extract_error_expr(graph, arg, current_ceiling, current_func, func_names);
                }
            }

            Expr::MethodCall(obj, _, args, _) => {
                self.extract_error_expr(graph, obj, current_ceiling, current_func, func_names);
                for arg in args {
                    self.extract_error_expr(graph, arg, current_ceiling, current_func, func_names);
                }
            }

            Expr::BinaryOp(left, _, right, _) => {
                self.extract_error_expr(graph, left, current_ceiling, current_func, func_names);
                self.extract_error_expr(graph, right, current_ceiling, current_func, func_names);
            }

            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.extract_error_expr(
                            graph,
                            cond,
                            current_ceiling,
                            current_func,
                            func_names,
                        );
                    }
                    for stmt in &arm.body {
                        if let Statement::Expr(e) = stmt {
                            self.extract_error_expr(
                                graph,
                                e,
                                current_ceiling,
                                current_func,
                                func_names,
                            );
                        }
                    }
                }
            }

            _ => {}
        }
    }

    // ── Call Graph ──────────────────────────────────────

    /// Extract a call graph from a program.
    pub fn extract_call(&mut self, program: &Program) -> Graph {
        let mut graph = Graph::new(GraphView::Call);
        graph.source_files.push(self.file.clone());

        // Add entrypoint
        let entry_id = format!("{}:entrypoint", self.file);
        graph.add_node(GraphNode {
            id: entry_id.clone(),
            kind: NodeKind::Entrypoint,
            label: self.file.clone(),
            location: Location {
                file: self.file.clone(),
                line: 0,
                column: 0,
            },
            metadata: HashMap::new(),
        });

        // Collect exported symbol names for later use
        let exported_symbols: std::collections::HashSet<String> = program
            .statements
            .iter()
            .filter_map(|stmt| {
                if let Statement::Export(export) = stmt {
                    Some(export.symbols.clone())
                } else {
                    None
                }
            })
            .flatten()
            .collect();

        // First pass: collect all function definitions
        let mut func_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for stmt in &program.statements {
            if let Statement::FuncDef(fd) = stmt {
                let span = &fd.span;
                let fn_id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Function);
                graph.add_node(GraphNode {
                    id: fn_id.clone(),
                    kind: NodeKind::Function,
                    label: fd.name.clone(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });
                func_names.insert(fd.name.clone());

                // If this function is exported, add a call edge from entrypoint
                if exported_symbols.contains(&fd.name) {
                    graph.add_edge(GraphEdge {
                        source: entry_id.clone(),
                        target: fn_id,
                        kind: EdgeKind::Calls,
                        label: format!("exports {}", fd.name),
                        metadata: HashMap::new(),
                    });
                }
            }
        }

        // Second pass: extract call edges
        for stmt in &program.statements {
            match stmt {
                Statement::FuncDef(fd) => {
                    let span = &fd.span;
                    let caller_id =
                        Graph::make_id(&self.file, span.line, span.column, &NodeKind::Function);
                    let body_len = fd.body.len();
                    for (i, body_stmt) in fd.body.iter().enumerate() {
                        let is_last = i == body_len - 1;
                        self.extract_calls_stmt(
                            graph.clone(),
                            &mut graph,
                            &caller_id,
                            body_stmt,
                            &func_names,
                            is_last,
                        );
                    }
                }

                Statement::Expr(expr) => {
                    self.extract_calls_expr(&graph.clone(), &mut graph, &entry_id, expr, false);
                    // Also track function references in top-level expressions
                    // (e.g., passing a function as an argument to another function)
                    self.extract_func_ref_from_expr(
                        &graph.clone(),
                        &mut graph,
                        &entry_id,
                        expr,
                        &func_names,
                    );
                }

                Statement::Assignment(assign) => {
                    self.extract_calls_expr(
                        &graph.clone(),
                        &mut graph,
                        &entry_id,
                        &assign.value,
                        false,
                    );
                    // If the RHS is a function name reference (variable assigned to a function),
                    // treat it as a usage of that function from the entrypoint.
                    self.extract_func_ref_from_expr(
                        &graph.clone(),
                        &mut graph,
                        &entry_id,
                        &assign.value,
                        &func_names,
                    );
                }

                Statement::UnmoldForward(uf) => {
                    self.extract_calls_expr(
                        &graph.clone(),
                        &mut graph,
                        &entry_id,
                        &uf.source,
                        false,
                    );
                }

                _ => {}
            }
        }

        graph
    }

    fn extract_calls_stmt(
        &mut self,
        graph_snapshot: Graph,
        graph: &mut Graph,
        caller_id: &str,
        stmt: &Statement,
        func_names: &std::collections::HashSet<String>,
        is_tail: bool,
    ) {
        match stmt {
            Statement::Expr(expr) => {
                self.extract_calls_expr(&graph_snapshot, graph, caller_id, expr, is_tail);
                // Track function references in expression statements within function bodies
                self.extract_func_ref_from_expr(
                    &graph_snapshot,
                    graph,
                    caller_id,
                    expr,
                    func_names,
                );
            }
            Statement::Assignment(assign) => {
                // Assignments are never in tail position (they bind a value, not return it)
                self.extract_calls_expr(&graph_snapshot, graph, caller_id, &assign.value, false);
                // Track function references assigned to variables within function bodies
                self.extract_func_ref_from_expr(
                    &graph_snapshot,
                    graph,
                    caller_id,
                    &assign.value,
                    func_names,
                );
            }
            Statement::UnmoldForward(uf) => {
                self.extract_calls_expr(&graph_snapshot, graph, caller_id, &uf.source, false);
            }
            Statement::UnmoldBackward(ub) => {
                self.extract_calls_expr(&graph_snapshot, graph, caller_id, &ub.source, false);
            }
            Statement::ErrorCeiling(ec) => {
                for body_stmt in &ec.handler_body {
                    self.extract_calls_stmt(
                        graph_snapshot.clone(),
                        graph,
                        caller_id,
                        body_stmt,
                        func_names,
                        false,
                    );
                }
            }
            _ => {}
        }
    }

    fn extract_calls_expr(
        &mut self,
        graph_snapshot: &Graph,
        graph: &mut Graph,
        caller_id: &str,
        expr: &Expr,
        is_tail: bool,
    ) {
        match expr {
            Expr::FuncCall(callee, args, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    // Find the function node by label
                    let callee_id = graph_snapshot
                        .nodes
                        .iter()
                        .find(|n| n.label == *name && matches!(n.kind, NodeKind::Function))
                        .map(|n| n.id.clone());

                    if let Some(cid) = callee_id {
                        let edge_kind = if is_tail {
                            EdgeKind::TailCalls
                        } else {
                            EdgeKind::Calls
                        };
                        let label_prefix = if is_tail { "tail calls" } else { "calls" };
                        graph.add_edge(GraphEdge {
                            source: caller_id.to_string(),
                            target: cid,
                            kind: edge_kind,
                            label: format!("{} {}", label_prefix, name),
                            metadata: HashMap::new(),
                        });
                    }
                }

                // Recurse into arguments
                for arg in args {
                    self.extract_calls_expr(graph_snapshot, graph, caller_id, arg, false);
                }
            }

            Expr::MethodCall(obj, _, args, _) => {
                self.extract_calls_expr(graph_snapshot, graph, caller_id, obj, false);
                for arg in args {
                    self.extract_calls_expr(graph_snapshot, graph, caller_id, arg, false);
                }
            }

            Expr::Lambda(_, body, span) => {
                self.lambda_counter += 1;
                let lambda_id = format!(
                    "{}:{}:{}:AnonymousFn:{}",
                    self.file, span.line, span.column, self.lambda_counter
                );
                graph.add_node(GraphNode {
                    id: lambda_id.clone(),
                    kind: NodeKind::AnonymousFn,
                    label: format!("_lambda_{}", self.lambda_counter),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata: HashMap::new(),
                });

                graph.add_edge(GraphEdge {
                    source: caller_id.to_string(),
                    target: lambda_id.clone(),
                    kind: EdgeKind::CallsLambda,
                    label: "calls lambda".to_string(),
                    metadata: HashMap::new(),
                });

                // Recurse into lambda body
                self.extract_calls_expr(graph_snapshot, graph, &lambda_id, body, false);
            }

            Expr::BinaryOp(left, _, right, _) => {
                self.extract_calls_expr(graph_snapshot, graph, caller_id, left, false);
                self.extract_calls_expr(graph_snapshot, graph, caller_id, right, false);
            }

            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.extract_calls_expr(graph_snapshot, graph, caller_id, cond, false);
                    }
                    // Propagate tail position into each arm body (last expr only)
                    for (si, stmt) in arm.body.iter().enumerate() {
                        if let Statement::Expr(e) = stmt {
                            let tail = is_tail && si == arm.body.len() - 1;
                            self.extract_calls_expr(graph_snapshot, graph, caller_id, e, tail);
                        }
                    }
                }
            }

            Expr::MoldInst(_, type_args, _, _) => {
                for arg in type_args {
                    self.extract_calls_expr(graph_snapshot, graph, caller_id, arg, false);
                }
            }

            Expr::Pipeline(stages, _) => {
                for stage in stages {
                    self.extract_calls_expr(graph_snapshot, graph, caller_id, stage, false);
                    // If a pipeline stage is just a function name (Ident), it's being used
                    // as a function reference: `data => myFunc => result`
                    if let Expr::Ident(name, _) = stage {
                        let callee_id = graph_snapshot
                            .nodes
                            .iter()
                            .find(|n| n.label == *name && matches!(n.kind, NodeKind::Function))
                            .map(|n| n.id.clone());
                        if let Some(cid) = callee_id {
                            graph.add_edge(GraphEdge {
                                source: caller_id.to_string(),
                                target: cid,
                                kind: EdgeKind::Calls,
                                label: format!("pipes to {}", name),
                                metadata: HashMap::new(),
                            });
                        }
                    }
                }
            }

            Expr::ListLit(items, _) => {
                for item in items {
                    self.extract_calls_expr(graph_snapshot, graph, caller_id, item, false);
                }
            }

            _ => {}
        }
    }

    /// Detect function name references in expressions (e.g., assigned to a variable,
    /// passed as an argument). This helps avoid false positive dead-code warnings
    /// for functions that are referenced but not directly called.
    fn extract_func_ref_from_expr(
        &self,
        graph_snapshot: &Graph,
        graph: &mut Graph,
        caller_id: &str,
        expr: &Expr,
        func_names: &std::collections::HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name, _) if func_names.contains(name) => {
                let callee_id = graph_snapshot
                    .nodes
                    .iter()
                    .find(|n| n.label == *name && matches!(n.kind, NodeKind::Function))
                    .map(|n| n.id.clone());
                if let Some(cid) = callee_id {
                    graph.add_edge(GraphEdge {
                        source: caller_id.to_string(),
                        target: cid,
                        kind: EdgeKind::Calls,
                        label: format!("references {}", name),
                        metadata: HashMap::new(),
                    });
                }
            }
            Expr::Pipeline(stages, _) => {
                for stage in stages {
                    self.extract_func_ref_from_expr(
                        graph_snapshot,
                        graph,
                        caller_id,
                        stage,
                        func_names,
                    );
                }
            }
            Expr::FuncCall(callee, args, _) => {
                self.extract_func_ref_from_expr(
                    graph_snapshot,
                    graph,
                    caller_id,
                    callee,
                    func_names,
                );
                for arg in args {
                    self.extract_func_ref_from_expr(
                        graph_snapshot,
                        graph,
                        caller_id,
                        arg,
                        func_names,
                    );
                }
            }
            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.extract_func_ref_from_expr(
                            graph_snapshot,
                            graph,
                            caller_id,
                            cond,
                            func_names,
                        );
                    }
                    for stmt in &arm.body {
                        if let Statement::Expr(e) = stmt {
                            self.extract_func_ref_from_expr(
                                graph_snapshot,
                                graph,
                                caller_id,
                                e,
                                func_names,
                            );
                        }
                    }
                }
            }
            Expr::MoldInst(_, type_args, _, _) => {
                for arg in type_args {
                    self.extract_func_ref_from_expr(
                        graph_snapshot,
                        graph,
                        caller_id,
                        arg,
                        func_names,
                    );
                }
            }
            _ => {}
        }
    }

    // ── High-level: extract all views ───────────────────

    /// Extract a specific graph view from a program.
    pub fn extract(&mut self, program: &Program, view: GraphView) -> Graph {
        match view {
            GraphView::Dataflow => self.extract_dataflow(program),
            GraphView::Module => self.extract_module(program),
            GraphView::TypeHierarchy => self.extract_type_hierarchy(program),
            GraphView::Error => self.extract_error(program),
            GraphView::Call => self.extract_call(program),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_extract(source: &str, view: GraphView) -> Graph {
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut extractor = GraphExtractor::new("test.td");
        extractor.extract(&program, view)
    }

    // ── Dataflow extraction ──

    #[test]
    fn test_dataflow_assignment() {
        let graph = parse_and_extract("x <= 42", GraphView::Dataflow);
        assert!(!graph.nodes.is_empty());
        assert!(graph.nodes.iter().any(|n| n.label == "x"));
        assert!(graph.nodes.iter().any(|n| n.label == "42"));
        assert!(graph.edges.iter().any(|e| e.kind == EdgeKind::PipeBackward));
    }

    #[test]
    fn test_dataflow_unmold_forward() {
        let source =
            "numbers <= @[1, 2, 3]\ndoubleFn x =\n  x * 2\nMap[numbers, doubleFn]() ]=> doubled";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(graph.nodes.iter().any(|n| n.label == "doubled"));
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::UnmoldForward)
        );
    }

    #[test]
    fn test_dataflow_function_call() {
        let source = "add x y =\n  x + y\nresult <= add(1, 2)";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(graph.nodes.iter().any(|n| n.kind == NodeKind::FunctionCall));
        assert!(graph.edges.iter().any(|e| e.kind == EdgeKind::Argument));
    }

    #[test]
    fn test_dataflow_condition_branch() {
        // C20-1 (ROOT-5): wrap multi-line rhs guard in parens.
        let source = "x <= 10\ny <= (\n  | x > 5 |> \"big\"\n  | _ |> \"small\"\n)";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(graph.nodes.iter().any(|n| n.kind == NodeKind::Condition));
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::ConditionTrue || e.kind == EdgeKind::ConditionFalse)
        );
    }

    // ── Module extraction ──

    #[test]
    fn test_module_import() {
        let source = ">>> ./utils.td => @(helper, format)";
        let graph = parse_and_extract(source, GraphView::Module);
        assert!(
            graph
                .nodes
                .iter()
                .any(|n| n.kind == NodeKind::Module && n.label == "test.td")
        );
        assert!(graph.nodes.iter().any(|n| n.label == "./utils.td"));
        assert!(graph.edges.iter().any(|e| e.kind == EdgeKind::Imports));
    }

    #[test]
    fn test_module_export() {
        let source = "x <= 42\n<<< @(x)";
        let graph = parse_and_extract(source, GraphView::Module);
        assert!(graph.edges.iter().any(|e| e.kind == EdgeKind::Exports));
    }

    // ── Type hierarchy extraction ──

    #[test]
    fn test_type_hierarchy_typedef() {
        let source = "Person = @(name: Str, age: Int)";
        let graph = parse_and_extract(source, GraphView::TypeHierarchy);
        assert!(
            graph
                .nodes
                .iter()
                .any(|n| n.label == "Person" && n.kind == NodeKind::BuchiPackType)
        );
    }

    #[test]
    fn test_type_hierarchy_error_inheritance() {
        let source = "Error => ValidationError = @(field: Str)";
        let graph = parse_and_extract(source, GraphView::TypeHierarchy);
        assert!(graph.nodes.iter().any(|n| n.label == "Error"));
        assert!(graph.nodes.iter().any(|n| n.label == "ValidationError"));
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::ErrorInheritance)
        );
    }

    // ── Error boundary extraction ──

    #[test]
    fn test_error_ceiling_extraction() {
        let source = "processData input =\n  |== error: Error =\n    \"default\"\n  => :Str\n  input\n=> :Str";
        let graph = parse_and_extract(source, GraphView::Error);
        assert!(graph.nodes.iter().any(|n| n.kind == NodeKind::ErrorCeiling));
    }

    // ── Call graph extraction ──

    #[test]
    fn test_call_graph_basic() {
        let source = "add x y =\n  x + y\n\ndouble x =\n  add(x, x)\n\nresult <= double(5)";
        let graph = parse_and_extract(source, GraphView::Call);
        assert!(
            graph
                .nodes
                .iter()
                .any(|n| n.label == "add" && n.kind == NodeKind::Function)
        );
        assert!(
            graph
                .nodes
                .iter()
                .any(|n| n.label == "double" && n.kind == NodeKind::Function)
        );
        // double calls add
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::Calls && e.label.contains("add"))
        );
    }

    #[test]
    fn test_call_graph_lambda() {
        let source = "numbers <= @[1, 2, 3]\ndoubleFn x =\n  x * 2\nMap[numbers, doubleFn]()";
        let graph = parse_and_extract(source, GraphView::Call);
        assert!(graph.nodes.iter().any(|n| n.kind == NodeKind::Entrypoint));
    }

    #[test]
    fn test_call_graph_func_ref_in_body() {
        // A function assigned to a variable inside another function's body
        // should create a reference edge and not be considered dead code.
        let source = "helper x =\n  x + 1\n\nwrapper =\n  callback <= helper\n  callback(5)\n=> :Int\n\nresult <= wrapper()";
        let graph = parse_and_extract(source, GraphView::Call);
        // helper should have an edge from wrapper (referenced inside body)
        let wrapper_node = graph.nodes.iter().find(|n| n.label == "wrapper");
        assert!(wrapper_node.is_some(), "wrapper function node should exist");
        let helper_node = graph.nodes.iter().find(|n| n.label == "helper");
        assert!(helper_node.is_some(), "helper function node should exist");
        // There should be an edge referencing helper from wrapper's body
        let has_ref = graph
            .edges
            .iter()
            .any(|e| e.source == wrapper_node.unwrap().id && e.label.contains("helper"));
        assert!(
            has_ref,
            "wrapper should reference helper via body assignment"
        );
    }

    #[test]
    fn test_call_graph_func_ref_in_expr_stmt() {
        // A function referenced in a top-level expression statement (e.g., passed as argument)
        let source = "process x =\n  x * 2\n\napply fn x =\n  fn(x)\n\napply(process, 5)";
        let graph = parse_and_extract(source, GraphView::Call);
        let process_node = graph.nodes.iter().find(|n| n.label == "process");
        assert!(process_node.is_some(), "process function node should exist");
        // The entrypoint should have a reference to process (passed as argument)
        let entry_node = graph.nodes.iter().find(|n| n.kind == NodeKind::Entrypoint);
        assert!(entry_node.is_some());
        let has_ref = graph
            .edges
            .iter()
            .any(|e| e.source == entry_node.unwrap().id && e.label.contains("process"));
        assert!(
            has_ref,
            "entrypoint should reference process via expression"
        );
    }

    // ── Cross-function error propagation (V-4) ──

    #[test]
    fn test_error_propagates_edge_with_ceiling() {
        // Function `risky` has an uncovered throw.
        // Function `safe` calls `risky` under a ceiling.
        // A Propagates edge should be generated: ceiling -> risky
        let source = "risky x =
  Error(message <= \"boom\").throw()
=> :Str

safe input =
  |== e: Error =
    \"default\"
  => :Str
  risky(input)
=> :Str";
        let graph = parse_and_extract(source, GraphView::Error);

        // Should have Propagates edge from ceiling to risky function
        let has_propagates = graph
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::Propagates && e.label.contains("risky"));
        assert!(
            has_propagates,
            "Should have Propagates edge from ceiling to risky function"
        );

        // risky's throw should have enclosing_function metadata
        let throw_site = graph.nodes.iter().find(|n| n.kind == NodeKind::ThrowSite);
        assert!(throw_site.is_some(), "Should have a ThrowSite node");
        assert!(
            throw_site
                .unwrap()
                .metadata
                .contains_key("enclosing_function"),
            "ThrowSite should have enclosing_function metadata"
        );
    }

    #[test]
    fn test_error_propagates_edge_without_ceiling() {
        // Function `inner` has a throw.
        // Function `outer` calls `inner` without a ceiling.
        // A Propagates edge should be generated: outer -> inner (throw propagates upward)
        let source = "inner x =
  Error(message <= \"boom\").throw()
=> :Str

outer input =
  inner(input)
=> :Str";
        let graph = parse_and_extract(source, GraphView::Error);

        // Should have Propagates edge from outer to inner
        let outer_node = graph
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Function && n.label == "outer");
        let inner_node = graph
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Function && n.label == "inner");
        assert!(outer_node.is_some(), "outer function node should exist");
        assert!(inner_node.is_some(), "inner function node should exist");

        let has_propagates = graph.edges.iter().any(|e| {
            e.kind == EdgeKind::Propagates
                && e.source == outer_node.as_ref().unwrap().id
                && e.target == inner_node.as_ref().unwrap().id
        });
        assert!(
            has_propagates,
            "Should have Propagates edge from outer to inner"
        );
    }

    #[test]
    fn test_error_self_call_propagates() {
        // Function `risky` has a throw and calls itself (recursive).
        // The self-call should generate a Propagates edge from risky to risky
        // (since the call is inside risky without a ceiling).
        // This is correct behavior: the throw propagates up the call chain.
        let source = "risky x =\n  Error(message <= \"boom\").throw()\n  risky(x)\n=> :Str";
        let graph = parse_and_extract(source, GraphView::Error);

        // Should have a Propagates edge (risky -> risky, self-call)
        let has_propagates = graph.edges.iter().any(|e| e.kind == EdgeKind::Propagates);
        assert!(
            has_propagates,
            "Recursive call should generate Propagates edge"
        );
    }

    // ── Tail call detection (V-5) ──

    #[test]
    fn test_call_graph_tail_call_basic() {
        // The last expression in `wrapper` is a call to `helper` — this is a tail call.
        let source = "helper x =\n  x + 1\n\nwrapper x =\n  helper(x)\n=> :Int";
        let graph = parse_and_extract(source, GraphView::Call);
        let wrapper_node = graph.nodes.iter().find(|n| n.label == "wrapper").unwrap();
        let has_tail = graph.edges.iter().any(|e| {
            e.source == wrapper_node.id
                && e.kind == EdgeKind::TailCalls
                && e.label.contains("helper")
        });
        assert!(has_tail, "Last expression call should be TailCalls edge");
    }

    #[test]
    fn test_call_graph_non_tail_call() {
        // `add(x, x)` is NOT in tail position because there is a statement after it.
        let source = "add x y =\n  x + y\n\ndouble x =\n  add(x, x)\n  42\n=> :Int";
        let graph = parse_and_extract(source, GraphView::Call);
        let double_node = graph.nodes.iter().find(|n| n.label == "double").unwrap();
        let has_tail = graph.edges.iter().any(|e| {
            e.source == double_node.id && e.kind == EdgeKind::TailCalls && e.label.contains("add")
        });
        assert!(
            !has_tail,
            "Non-last expression call should NOT be TailCalls edge"
        );
        let has_regular = graph.edges.iter().any(|e| {
            e.source == double_node.id && e.kind == EdgeKind::Calls && e.label.contains("add")
        });
        assert!(has_regular, "Non-tail call should be regular Calls edge");
    }

    #[test]
    fn test_call_graph_tail_call_in_cond_branch() {
        // Tail call within a conditional branch arm (last expression is CondBranch with calls)
        let source = "helper x =\n  x + 1\n\nother x =\n  x * 2\n\nwrapper x =\n  | x > 0 |> helper(x)\n  | _ |> other(x)";
        let graph = parse_and_extract(source, GraphView::Call);
        let wrapper_node = graph.nodes.iter().find(|n| n.label == "wrapper").unwrap();
        let tail_edges: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.source == wrapper_node.id && e.kind == EdgeKind::TailCalls)
            .collect();
        assert!(
            tail_edges.iter().any(|e| e.label.contains("helper")),
            "helper call in cond arm should be TailCalls"
        );
        assert!(
            tail_edges.iter().any(|e| e.label.contains("other")),
            "other call in cond arm should be TailCalls"
        );
    }

    #[test]
    fn test_call_graph_tail_call_recursive() {
        // Recursive tail call: factorial-like pattern
        let source = "factorial n acc =\n  | n < 1 |> acc\n  | _ |> factorial(n - 1, n * acc)";
        let graph = parse_and_extract(source, GraphView::Call);
        let has_tail = graph
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::TailCalls && e.label.contains("factorial"));
        assert!(
            has_tail,
            "Recursive call in tail position should be TailCalls edge"
        );
    }

    // ── RC-3: Dataflow completeness for all Expr variants ──

    #[test]
    fn test_dataflow_field_access() {
        let source = "name <= \"hello\"\nlen <= name.length";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(
            graph.nodes.iter().any(|n| n.label == ".length"),
            "FieldAccess should produce a node labeled .length"
        );
    }

    #[test]
    fn test_dataflow_unary_op() {
        let source = "x <= true\ny <= !x";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(
            graph.nodes.iter().any(|n| n.label == "unary_not"),
            "UnaryOp(Not) should produce a node labeled unary_not"
        );
    }

    #[test]
    fn test_dataflow_unmold_stmt() {
        // Unmold is statement-level (]=> / <=[), not expression-level
        // .unmold() parses as a MethodCall
        let source = "lax <= Lax[42]()\nlax ]=> val";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::UnmoldForward),
            "UnmoldForward statement should produce UnmoldForward edge"
        );
    }

    #[test]
    fn test_dataflow_lambda() {
        let source = "fn <= _ x = x * 2";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(
            graph.nodes.iter().any(|n| n.label.starts_with("_ ")),
            "Lambda should produce a node with label starting with '_ '"
        );
    }

    #[test]
    fn test_dataflow_type_inst() {
        let source = "Person = @(name: Str, age: Int)\np <= Person(name <= \"Alice\", age <= 30)";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(
            graph.nodes.iter().any(|n| n.label == "Person(...)"),
            "TypeInst should produce a node labeled Person(...)"
        );
    }

    #[test]
    fn test_dataflow_throw() {
        let source =
            "Error = @(message: Str)\nprocess x =\n  Error(message <= \"boom\").throw()\n=> :Str";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(
            graph.nodes.iter().any(|n| n.label == ".throw()"),
            "Throw should produce a node labeled .throw()"
        );
    }

    #[test]
    fn test_dataflow_gorilla_literal() {
        // Gorilla literal >< should produce a Literal node
        let source = "x <= ><";
        let graph = parse_and_extract(source, GraphView::Dataflow);
        assert!(
            graph.nodes.iter().any(|n| n.label == "><"),
            "Gorilla literal should produce a node labeled '><'"
        );
    }
}

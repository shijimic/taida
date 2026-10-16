//! AST-to-graph extraction for all 5 graph views.
//!
//! Because Taida has only 10 operators with unambiguous semantics,
//! graph extraction is deterministic — no type inference or control
//! flow analysis required.

use std::collections::HashMap;

use super::model::*;
use super::walk::{self, Node};
use crate::lexer::Span;
use crate::parser::*;

/// Extractor state.
pub struct GraphExtractor {
    file: String,
    /// Counter for generating unique anonymous function IDs.
    lambda_counter: usize,
    /// Set while walking the body of a function nested in another one: what
    /// it calls is reachable through the enclosing function, but is not a
    /// call the enclosing function makes itself (the nested function may
    /// never be entered), so the edges are references.
    inside_nested_function: bool,
    /// The names bound at the point being walked: parameters, earlier
    /// bindings, lambda parameters. A call written with one of them calls
    /// that local value, not the top-level function of the same name.
    bindings: walk::Bindings,
    /// Set while walking a parameter default: what a default calls is only
    /// evaluated when the argument is left out, so it makes a function
    /// reachable but is not a call the function makes.
    inside_default: bool,
    /// Set while walking the body of a top-level function: a type defined
    /// there has methods that run only when called.
    inside_function: bool,
    /// Set while walking the handler of an error ceiling. A ceiling does not
    /// cover its own handler, so nothing found by reading the code around a
    /// call (a function handed to a mold, a method reached by name) is taken
    /// to be covered there.
    inside_handler: bool,
}

/// The functions and methods of a program, by name, for linking a call (or a
/// reference) to the node of its callee in the error view.
#[derive(Default)]
struct ErrorScope {
    /// Top-level function name → node id. A function defined inside another
    /// one is found through the bindings of the code that can see it.
    functions: HashMap<String, String>,
    /// Method name → node ids of every method with that name.
    methods: HashMap<String, Vec<String>>,
    /// (Type name, method name) → node ids of the methods of that type.
    type_methods: HashMap<(String, String), Vec<String>>,
}

/// The built-in molds that call the function they are given while they run,
/// so that a function named or a lambda written in their arguments runs
/// inside the ceiling of the code around the mold.
const CALLBACK_MOLDS: &[&str] = &[
    "Map",
    "Filter",
    "Fold",
    "Foldr",
    "Reduce",
    "TakeWhile",
    "DropWhile",
    "Find",
    "FindIndex",
    "FindIndexLax",
    "Count",
    "Sort",
    "Unique",
    "ParMap",
    "AsyncTask",
];

impl GraphExtractor {
    pub fn new(file: &str) -> Self {
        Self {
            file: file.to_string(),
            lambda_counter: 0,
            inside_nested_function: false,
            bindings: walk::Bindings::default(),
            inside_default: false,
            inside_function: false,
            inside_handler: false,
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
                // The whole statement runs in the direction of its arrow:
                // `expr => name` is a forward pipe, `name <= expr` a
                // backward one, and a chain's own stages follow suit.
                let edge_kind = if assign.forward {
                    EdgeKind::PipeForward
                } else {
                    EdgeKind::PipeBackward
                };
                self.extract_dataflow_expr_to(graph, &assign.value, &var_id, edge_kind);
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
                        label: ">=>".to_string(),
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
                        label: "<=<".to_string(),
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
            // F62B-022: block-bodied lambda body — extract each statement's
            // dataflow; the block itself contributes no node.
            Expr::Block(stmts, _) => {
                for stmt in stmts {
                    self.extract_dataflow_stmt(graph, stmt);
                }
                None
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
                self.extract_dataflow_pipeline(graph, exprs, span, EdgeKind::PipeForward)
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
                    label: ">=>".to_string(),
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
                        label: ">=>".to_string(),
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

    /// Extract the stages of a pipeline, joined by edges of `edge_kind`.
    /// Returns the node of the last stage.
    fn extract_dataflow_pipeline(
        &mut self,
        graph: &mut Graph,
        exprs: &[Expr],
        span: &Span,
        edge_kind: EdgeKind,
    ) -> Option<String> {
        let mut prev_id: Option<String> = None;
        for (i, e) in exprs.iter().enumerate() {
            let cur_id = self.extract_dataflow_expr(graph, e);
            if i > 0
                && let (Some(prev), Some(cur)) = (&prev_id, &cur_id)
            {
                graph.add_edge(GraphEdge {
                    source: prev.clone(),
                    target: cur.clone(),
                    kind: edge_kind.clone(),
                    label: Self::pipe_label(&edge_kind).to_string(),
                    metadata: HashMap::new(),
                });
            }
            prev_id = cur_id;
        }
        // Return the ID of the last in the pipeline
        if prev_id.is_some() {
            prev_id
        } else {
            let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Variable);
            Some(id)
        }
    }

    fn pipe_label(edge_kind: &EdgeKind) -> &'static str {
        match edge_kind {
            EdgeKind::PipeBackward => "<=",
            _ => "=>",
        }
    }

    /// Add a pipe edge (of `edge_kind`) from an expression to a target node.
    fn extract_dataflow_expr_to(
        &mut self,
        graph: &mut Graph,
        expr: &Expr,
        target_id: &str,
        edge_kind: EdgeKind,
    ) {
        let source_id = match expr {
            Expr::Pipeline(exprs, span) => {
                self.extract_dataflow_pipeline(graph, exprs, span, edge_kind.clone())
            }
            other => self.extract_dataflow_expr(graph, other),
        };
        if let Some(source_id) = source_id {
            graph.add_edge(GraphEdge {
                source: source_id,
                target: target_id.to_string(),
                label: Self::pipe_label(&edge_kind).to_string(),
                kind: edge_kind,
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

                // (E30B-007 sub-step B-5 / Lock-G Sub-G5、2026-04-28)
                // explicit `Name <= RustAddon["fn"](arity <= N)` binding
                // を module graph に **public function** node として追加。
                // AST 上は Assignment で表現されるが、addon-backed callable
                // という contract に従い、graph view では Function node 扱い。
                // metadata には source = "rust_addon"、addon_fn_name、arity
                // を記録、consumer (introspection / 可視化 tool) が判別可能。
                Statement::Assignment(assign) if assign.as_rust_addon_binding().is_some() => {
                    let span = &assign.span;
                    let (fn_name, arity) = assign.as_rust_addon_binding().expect("checked");
                    let fn_id =
                        Graph::make_id(&self.file, span.line, span.column, &NodeKind::Function);
                    let mut meta = HashMap::new();
                    meta.insert("source".to_string(), "rust_addon".to_string());
                    meta.insert("addon_fn_name".to_string(), fn_name);
                    meta.insert("arity".to_string(), arity.to_string());
                    graph.add_node(GraphNode {
                        id: fn_id.clone(),
                        kind: NodeKind::Function,
                        label: assign.target.clone(),
                        location: Location {
                            file: self.file.clone(),
                            line: span.line,
                            column: span.column,
                        },
                        metadata: meta,
                    });
                    graph.add_edge(GraphEdge {
                        source: module_id.clone(),
                        target: fn_id,
                        kind: EdgeKind::SymbolRef,
                        label: format!("defines {}", assign.target),
                        metadata: HashMap::new(),
                    });
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
            // (E30 Phase 7.5 / E30B-006, Lock-F 軸 2) ClassLikeDef → 単一
            // `NodeKind::ClassLikeType` + 単一 `EdgeKind::ClassLikeInheritance`
            // (旧 BuchiPackType / MoldType / ErrorType / MoldInheritance /
            // ErrorInheritance を統合)。surface 上の旧 3 系統 (BuchiPack / Mold /
            // Inheritance) は metadata で記録し、structural_summary 等の
            // consumer はそれを参照して旧分類を復元する。
            if let Statement::ClassLikeDef(cl) = stmt {
                let span = &cl.span;
                let kind_label = match &cl.kind {
                    crate::parser::ClassLikeKind::BuchiPack => "BuchiPack",
                    crate::parser::ClassLikeKind::Mold { .. } => "Mold",
                    crate::parser::ClassLikeKind::Inheritance { .. } => "Inheritance",
                    crate::parser::ClassLikeKind::Alias { .. } => "TypeAlias",
                };
                let id =
                    Graph::make_id(&self.file, span.line, span.column, &NodeKind::ClassLikeType);
                let mut metadata = HashMap::new();
                metadata.insert("class_like_kind".to_string(), kind_label.to_string());
                if let crate::parser::ClassLikeKind::Inheritance { parent, .. } = &cl.kind {
                    metadata.insert("inheritance_parent".to_string(), parent.clone());
                }
                graph.add_node(GraphNode {
                    id: id.clone(),
                    kind: NodeKind::ClassLikeType,
                    label: cl.name.clone(),
                    location: Location {
                        file: self.file.clone(),
                        line: span.line,
                        column: span.column,
                    },
                    metadata,
                });

                match &cl.kind {
                    crate::parser::ClassLikeKind::BuchiPack => {
                        // 単独定義 — inheritance edge なし
                    }
                    crate::parser::ClassLikeKind::Alias { .. } => {
                        // 型エイリアス — inheritance edge なし
                    }
                    crate::parser::ClassLikeKind::Mold { .. } => {
                        // Mold[T] base node + 単一 ClassLikeInheritance edge
                        let mold_base_id = "builtin:Mold".to_string();
                        let mut base_metadata = HashMap::new();
                        base_metadata.insert("class_like_kind".to_string(), "Mold".to_string());
                        graph.add_node(GraphNode {
                            id: mold_base_id.clone(),
                            kind: NodeKind::ClassLikeType,
                            label: "Mold[T]".to_string(),
                            location: Location {
                                file: "builtin".to_string(),
                                line: 0,
                                column: 0,
                            },
                            metadata: base_metadata,
                        });
                        graph.add_edge(GraphEdge {
                            source: mold_base_id,
                            target: id,
                            kind: EdgeKind::ClassLikeInheritance,
                            label: "Mold[T] =>".to_string(),
                            metadata: HashMap::new(),
                        });
                    }
                    crate::parser::ClassLikeKind::Inheritance { parent, .. } => {
                        // 親型 ClassLikeType node + inheritance edge
                        // Lock-F 軸 2: Mold/Error/その他親 の区別は edge level では
                        // 持たず、Error 親のときのみ ClassLikeInheritance edge、
                        // それ以外 (TypeDef inheritance) は StructuralSubtype edge
                        // を維持する (StructuralSubtype は class-like 統合とは
                        // 直交する別概念のため Lock-F 範囲外)。
                        let parent_id = format!("type:{}", parent);
                        let mut parent_metadata = HashMap::new();
                        parent_metadata
                            .insert("class_like_kind".to_string(), "Inheritance".to_string());
                        if parent == "Error" {
                            parent_metadata
                                .insert("inheritance_parent".to_string(), "Error".to_string());
                        }
                        graph.add_node(GraphNode {
                            id: parent_id.clone(),
                            kind: NodeKind::ClassLikeType,
                            label: parent.clone(),
                            location: Location {
                                file: self.file.clone(),
                                line: span.line,
                                column: span.column,
                            },
                            metadata: parent_metadata,
                        });

                        let edge_kind = if parent == "Error" {
                            EdgeKind::ClassLikeInheritance
                        } else {
                            EdgeKind::StructuralSubtype
                        };
                        graph.add_edge(GraphEdge {
                            source: parent_id,
                            target: id,
                            kind: edge_kind,
                            label: format!("{} => {}", parent, cl.name),
                            metadata: HashMap::new(),
                        });
                    }
                }
            }
        }

        graph
    }

    // ── Error Boundary Graph ────────────────────────────

    /// Extract an error boundary graph from a program.
    pub fn extract_error(&mut self, program: &Program) -> Graph {
        let mut graph = Graph::new(GraphView::Error);
        graph.source_files.push(self.file.clone());

        // Every function and method gets its node before any body is
        // walked, so a call to something defined further down the file (or
        // nested in another function) still produces its propagation edge.
        let mut scope = ErrorScope::default();
        for stmt in &program.statements {
            walk::for_each_nested_stmt(stmt, &mut |nested| match nested {
                Statement::FuncDef(fd) => {
                    let id = self.add_error_function_node(&mut graph, &fd.name, &fd.span);
                    if std::ptr::eq(nested, stmt) {
                        scope.functions.entry(fd.name.clone()).or_insert(id);
                    }
                }
                Statement::ClassLikeDef(cl) => {
                    for field in &cl.fields {
                        if let Some(method) = &field.method_def {
                            let label = format!("{}.{}", cl.name, method.name);
                            let id = self.add_error_function_node(&mut graph, &label, &method.span);
                            scope
                                .methods
                                .entry(method.name.clone())
                                .or_default()
                                .push(id.clone());
                            scope
                                .type_methods
                                .entry((cl.name.clone(), method.name.clone()))
                                .or_default()
                                .push(id);
                        }
                    }
                }
                _ => {}
            });
        }

        self.bindings = walk::Bindings::default();
        self.extract_error_stmts(&mut graph, &program.statements, None, &scope);

        graph
    }

    fn add_error_function_node(&self, graph: &mut Graph, label: &str, span: &Span) -> String {
        let id = Graph::make_id(&self.file, span.line, span.column, &NodeKind::Function);
        graph.add_node(GraphNode {
            id: id.clone(),
            kind: NodeKind::Function,
            label: label.to_string(),
            location: Location {
                file: self.file.clone(),
                line: span.line,
                column: span.column,
            },
            metadata: HashMap::new(),
        });
        id
    }

    /// Extract error-related nodes and edges from a list of statements.
    ///
    /// `current_ceiling`: the active error ceiling ID (if any) covering this scope.
    /// `scope`: the functions and methods defined in the program.
    fn extract_error_stmts(
        &mut self,
        graph: &mut Graph,
        stmts: &[Statement],
        current_ceiling: Option<&str>,
        scope: &ErrorScope,
    ) {
        self.extract_error_stmts_inner(graph, stmts, current_ceiling, None, scope);
    }

    fn extract_error_stmts_inner(
        &mut self,
        graph: &mut Graph,
        stmts: &[Statement],
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
        scope: &ErrorScope,
    ) {
        let mut active_ceiling: Option<String> = current_ceiling.map(|s| s.to_string());
        // What a list of statements binds is not visible after it.
        let scope_mark = self.bindings.mark();

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
                    let mark = self.bindings.mark();
                    self.bindings.bind(&ec.error_param);
                    let outer = std::mem::replace(&mut self.inside_handler, true);
                    self.extract_error_stmts_inner(
                        graph,
                        &ec.handler_body,
                        Some(&ceiling_id),
                        current_func,
                        scope,
                    );
                    self.inside_handler = outer;
                    self.bindings.release(mark);

                    active_ceiling = Some(ceiling_id);
                }

                Statement::FuncDef(fd) => {
                    let fn_id = Graph::make_id(
                        &self.file,
                        fd.span.line,
                        fd.span.column,
                        &NodeKind::Function,
                    );
                    // A nested function's name is bound in its own body (it
                    // can call itself) and from its definition on. (A
                    // top-level function is what a call by that name means.)
                    if current_func.is_some() {
                        self.bindings.bind_function(&fd.name, fn_id.clone());
                    }
                    self.extract_error_function(graph, &fd.params, &fd.body, &fn_id, scope);
                }

                // A field default runs when the type is instantiated, and a
                // throw in it is not caught by a ceiling around the
                // instantiation; a method body is a function of its own,
                // entered from wherever the method is called.
                Statement::ClassLikeDef(cl) => {
                    for field in &cl.fields {
                        if let Some(default) = &field.default_value {
                            self.extract_error_expr(graph, default, None, None, scope);
                        }
                        if let Some(method) = &field.method_def {
                            let method_id = Graph::make_id(
                                &self.file,
                                method.span.line,
                                method.span.column,
                                &NodeKind::Function,
                            );
                            self.extract_error_function(
                                graph,
                                &method.params,
                                &method.body,
                                &method_id,
                                scope,
                            );
                        }
                    }
                }

                Statement::Expr(expr) => {
                    self.extract_error_expr(
                        graph,
                        expr,
                        active_ceiling.as_deref(),
                        current_func,
                        scope,
                    );
                }

                Statement::UnmoldForward(uf) => {
                    self.extract_error_expr(
                        graph,
                        &uf.source,
                        active_ceiling.as_deref(),
                        current_func,
                        scope,
                    );
                    self.propagate_unmold_of(
                        graph,
                        &uf.source,
                        active_ceiling.as_deref(),
                        current_func,
                        scope,
                    );
                    self.bindings.bind(&uf.target);
                }

                Statement::UnmoldBackward(ub) => {
                    self.extract_error_expr(
                        graph,
                        &ub.source,
                        active_ceiling.as_deref(),
                        current_func,
                        scope,
                    );
                    self.propagate_unmold_of(
                        graph,
                        &ub.source,
                        active_ceiling.as_deref(),
                        current_func,
                        scope,
                    );
                    self.bindings.bind(&ub.target);
                }

                // The value is read before the name is bound.
                Statement::Assignment(assign) => {
                    self.extract_error_expr(
                        graph,
                        &assign.value,
                        active_ceiling.as_deref(),
                        current_func,
                        scope,
                    );
                    self.bindings.bind(&assign.target);
                }

                Statement::EnumDef(_) | Statement::Import(_) | Statement::Export(_) => {}
            }
        }
        self.bindings.release(scope_mark);
    }

    /// The body of a function or method, entered from wherever it is called
    /// (its own ceiling, if it declares one, is found in the body). Parameter
    /// defaults run inside it, when a call leaves the argument out.
    fn extract_error_function(
        &mut self,
        graph: &mut Graph,
        params: &[Param],
        body: &[Statement],
        fn_id: &str,
        scope: &ErrorScope,
    ) {
        let mark = self.bindings.mark();
        let outer = std::mem::replace(&mut self.inside_handler, false);
        self.bindings.bind_params(params);
        for param in params {
            if let Some(default) = &param.default_value {
                self.extract_error_expr(graph, default, None, Some(fn_id), scope);
            }
        }
        self.extract_error_stmts_inner(graph, body, None, Some(fn_id), scope);
        self.inside_handler = outer;
        self.bindings.release(mark);
    }

    /// Creating a value of a mold of the program runs its `solidify`
    /// method, and unmolding it runs its `unmold` method; either may throw.
    /// The hooks of the mold named there are reached from that place.
    fn propagate_mold_hook(
        &self,
        graph: &mut Graph,
        mold: &str,
        hook: &str,
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
        scope: &ErrorScope,
    ) {
        if self.inside_handler {
            return;
        }
        if let Some(method_ids) = scope
            .type_methods
            .get(&(mold.to_string(), hook.to_string()))
        {
            for method_id in method_ids {
                Self::add_error_propagation(graph, method_id, hook, current_ceiling, current_func);
            }
        }
    }

    /// Unmolding `source` runs the `unmold` method of its mold, when the
    /// source names the mold (`Strict[n]() >=> v`); a value of a type that is
    /// not known here is not taken to be covered.
    fn propagate_unmold_of(
        &self,
        graph: &mut Graph,
        source: &Expr,
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
        scope: &ErrorScope,
    ) {
        if let Expr::MoldInst(mold, _, _, _) = source {
            self.propagate_mold_hook(graph, mold, "unmold", current_ceiling, current_func, scope);
        }
    }

    /// The function a name calls at this point: a function defined in the
    /// code around it, else a top-level function, unless the name is a
    /// value the code binds itself.
    fn resolve_function(&self, name: &str, scope: &ErrorScope) -> Option<String> {
        match self.bindings.lookup(name) {
            Some(walk::Binding::Function(id)) => Some(id.clone()),
            Some(walk::Binding::Value) => None,
            None => scope.functions.get(name).cloned(),
        }
    }

    /// A call of the function `name`.
    fn propagate_call(
        &self,
        graph: &mut Graph,
        name: &str,
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
        scope: &ErrorScope,
    ) {
        if let Some(callee_id) = self.resolve_function(name, scope) {
            Self::add_error_propagation(graph, &callee_id, name, current_ceiling, current_func);
        }
    }

    /// A function named where the code around it hands it to something that
    /// calls it while it runs (an argument of a built-in mold that calls
    /// back, a stage of a pipeline): it runs inside whatever ceiling that
    /// code sits under.
    fn propagate_function_value(
        &self,
        graph: &mut Graph,
        name: &str,
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
        scope: &ErrorScope,
    ) {
        if !self.inside_handler {
            self.propagate_call(graph, name, current_ceiling, current_func, scope);
        }
    }

    /// An argument of a call or mold. `calls_back` says the receiver calls
    /// the function it is given while it runs (see [`CALLBACK_MOLDS`]): then
    /// a function named there, or a lambda written there, runs inside the
    /// ceiling of the code around it. Handed to anything else it may be kept
    /// and called later, so it is not taken to be covered.
    fn extract_error_arg(
        &mut self,
        graph: &mut Graph,
        arg: &Expr,
        calls_back: bool,
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
        scope: &ErrorScope,
    ) {
        match arg {
            Expr::Ident(name, _) => {
                if calls_back {
                    self.propagate_function_value(
                        graph,
                        name,
                        current_ceiling,
                        current_func,
                        scope,
                    );
                }
            }
            Expr::Lambda(params, body, _) => {
                let mark = self.bindings.mark();
                self.bindings.bind_params(params);
                let ceiling = if calls_back && !self.inside_handler {
                    current_ceiling
                } else {
                    None
                };
                self.extract_error_expr(graph, body, ceiling, current_func, scope);
                self.bindings.release(mark);
            }
            other => self.extract_error_expr(graph, other, current_ceiling, current_func, scope),
        }
    }

    /// Add the edge that lets a callee's throws be covered from a call site
    /// (or from a place where the function is passed as a value): a ceiling
    /// in scope covers the callee, otherwise the callee's throws propagate
    /// to the enclosing function.
    fn add_error_propagation(
        graph: &mut Graph,
        callee_id: &str,
        name: &str,
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
    ) {
        if let Some(ceiling_id) = current_ceiling {
            graph.add_edge(GraphEdge {
                source: ceiling_id.to_string(),
                target: callee_id.to_string(),
                kind: EdgeKind::Propagates,
                label: format!("covers {}", name),
                metadata: HashMap::new(),
            });
        } else if let Some(caller_id) = current_func {
            graph.add_edge(GraphEdge {
                source: caller_id.to_string(),
                target: callee_id.to_string(),
                kind: EdgeKind::Propagates,
                label: format!("propagates from {}", name),
                metadata: HashMap::new(),
            });
        }
        // Top-level call without ceiling: callee's uncovered throws
        // remain uncovered (no edge needed).
    }

    /// Extract error-related nodes and edges from an expression.
    ///
    /// `current_ceiling`: the active error ceiling ID (if any) covering this scope.
    /// `current_func`: the Function node ID of the enclosing function (if any).
    /// `scope`: the functions and methods defined in the program (for Propagates edges).
    ///
    /// Propagates edge semantics:
    /// - `ErrorCeiling -> Function`: the ceiling covers the callee's throws (cross-function coverage)
    /// - `Function(caller) -> Function(callee)`: callee's throws propagate to caller
    /// (allows transitive coverage: if caller is covered by a ceiling, callee is too)
    ///
    /// A function named as a value (`Map[xs, risky]()`) is treated like a
    /// call at that place: it runs inside whatever ceiling the code around it
    /// sits under. A method call reaches every method of that name, since the
    /// receiver's type is not known here.
    fn extract_error_expr(
        &mut self,
        graph: &mut Graph,
        expr: &Expr,
        current_ceiling: Option<&str>,
        current_func: Option<&str>,
        scope: &ErrorScope,
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
                self.extract_error_expr(graph, inner, current_ceiling, current_func, scope);
            }

            Expr::MethodCall(obj, method, args, span) if method == "throw" => {
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
                self.extract_error_expr(graph, obj, current_ceiling, current_func, scope);
                for arg in args {
                    self.extract_error_expr(graph, arg, current_ceiling, current_func, scope);
                }
            }

            Expr::FuncCall(callee, args, _) => {
                // Generate Propagates edges for cross-function error propagation.
                if let Expr::Ident(name, _) = callee.as_ref() {
                    self.propagate_call(graph, name, current_ceiling, current_func, scope);
                } else {
                    self.extract_error_expr(graph, callee, current_ceiling, current_func, scope);
                }
                for arg in args {
                    self.extract_error_arg(graph, arg, false, current_ceiling, current_func, scope);
                }
            }

            // A function named as a stage of a pipeline is called with the
            // value that reaches it; the first item is the value it starts from.
            Expr::Pipeline(items, _) => {
                for (i, item) in items.iter().enumerate() {
                    match item {
                        Expr::Ident(name, _) if i > 0 => self.propagate_function_value(
                            graph,
                            name,
                            current_ceiling,
                            current_func,
                            scope,
                        ),
                        other => {
                            self.extract_error_expr(
                                graph,
                                other,
                                current_ceiling,
                                current_func,
                                scope,
                            );
                        }
                    }
                }
            }

            // Unmolding runs the `unmold` method of the value's mold.
            Expr::Unmold(inner, _) => {
                self.propagate_unmold_of(graph, inner, current_ceiling, current_func, scope);
                self.extract_error_expr(graph, inner, current_ceiling, current_func, scope);
            }

            // A method is reached by its name, the receiver's type being
            // unknown here. A name that a built-in type has too may be a
            // call of that built-in method instead, so it is not taken to
            // reach a method of the program.
            Expr::MethodCall(obj, method, args, _) => {
                if !self.inside_handler
                    && !crate::types::is_builtin_method_name(method)
                    && let Some(method_ids) = scope.methods.get(method)
                {
                    for method_id in method_ids {
                        Self::add_error_propagation(
                            graph,
                            method_id,
                            method,
                            current_ceiling,
                            current_func,
                        );
                    }
                }
                self.extract_error_expr(graph, obj, current_ceiling, current_func, scope);
                for arg in args {
                    self.extract_error_arg(graph, arg, false, current_ceiling, current_func, scope);
                }
            }

            Expr::MoldInst(mold, type_args, fields, _) => {
                self.propagate_mold_hook(
                    graph,
                    mold,
                    "solidify",
                    current_ceiling,
                    current_func,
                    scope,
                );
                let calls_back = CALLBACK_MOLDS.contains(&mold.as_str());
                for arg in type_args {
                    self.extract_error_arg(
                        graph,
                        arg,
                        calls_back,
                        current_ceiling,
                        current_func,
                        scope,
                    );
                }
                for field in fields {
                    self.extract_error_arg(
                        graph,
                        &field.value,
                        calls_back,
                        current_ceiling,
                        current_func,
                        scope,
                    );
                }
            }

            // A lambda that is passed to a call runs while the call runs (see
            // `extract_error_arg`). One that is kept or returned may run
            // anywhere, so it is not covered by the code around it.
            Expr::Lambda(params, body, _) => {
                let mark = self.bindings.mark();
                self.bindings.bind_params(params);
                self.extract_error_expr(graph, body, None, current_func, scope);
                self.bindings.release(mark);
            }

            // A block or arm body is a list of statements: a ceiling declared
            // inside it covers the rest of that list only.
            Expr::Block(stmts, _) => {
                self.extract_error_stmts_inner(graph, stmts, current_ceiling, current_func, scope);
            }

            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.extract_error_expr(graph, cond, current_ceiling, current_func, scope);
                    }
                    self.extract_error_stmts_inner(
                        graph,
                        &arm.body,
                        current_ceiling,
                        current_func,
                        scope,
                    );
                }
            }

            // Everything else only passes control to its operands, fields
            // and interpolations.
            Expr::IntLit(..)
            | Expr::FloatLit(..)
            | Expr::StringLit(..)
            | Expr::TemplateLit(..)
            | Expr::BoolLit(..)
            | Expr::Gorilla(..)
            | Expr::Ident(..)
            | Expr::Placeholder(..)
            | Expr::Hole(..)
            | Expr::BuchiPack(..)
            | Expr::ListLit(..)
            | Expr::BinaryOp(..)
            | Expr::UnaryOp(..)
            | Expr::FieldAccess(..)
            | Expr::TypeInst(..)
            | Expr::EnumVariant(..)
            | Expr::TypeLiteral(..) => {
                walk::for_each_child_of_expr(expr, &mut |child| {
                    if let Node::Expr(inner) = child {
                        self.extract_error_expr(graph, inner, current_ceiling, current_func, scope);
                    }
                });
            }
        }
    }

    // ── Call Graph ──────────────────────────────────────

    /// Extract a call graph from a program.
    ///
    /// A call or a reference to a function counts wherever it is written:
    /// operands, arguments, pack and type-instance fields, template
    /// interpolations, arm bodies, method bodies and the bodies of functions
    /// nested in other functions. Code that is not inside a top-level
    /// function (including type definitions and their methods) belongs to
    /// the entrypoint; a nested function's body belongs to the function it
    /// is written in.
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
        let mut func_ids: HashMap<String, String> = HashMap::new();
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
                func_ids.entry(fd.name.clone()).or_insert(fn_id.clone());

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
                    self.bindings = walk::Bindings::default();
                    self.extract_calls_func(&mut graph, &func_ids, &caller_id, fd);
                }
                other => {
                    self.bindings = walk::Bindings::default();
                    self.extract_calls_stmt(&mut graph, &func_ids, &entry_id, other, false);
                }
            }
        }

        graph
    }

    /// Calls made by `fd`'s parameter defaults and body, attributed to
    /// `caller_id`. The last body statement is in tail position.
    fn extract_calls_func(
        &mut self,
        graph: &mut Graph,
        func_ids: &HashMap<String, String>,
        caller_id: &str,
        fd: &FuncDef,
    ) {
        let mark = self.bindings.mark();
        self.bindings.bind_params(&fd.params);
        self.extract_calls_defaults(graph, func_ids, caller_id, &fd.params);
        let outer = std::mem::replace(&mut self.inside_function, true);
        let body_len = fd.body.len();
        for (i, body_stmt) in fd.body.iter().enumerate() {
            let is_last = i + 1 == body_len;
            self.extract_calls_stmt(graph, func_ids, caller_id, body_stmt, is_last);
        }
        self.inside_function = outer;
        self.bindings.release(mark);
    }

    /// What the parameter defaults of a function refer to.
    fn extract_calls_defaults(
        &mut self,
        graph: &mut Graph,
        func_ids: &HashMap<String, String>,
        caller_id: &str,
        params: &[Param],
    ) {
        let outer = std::mem::replace(&mut self.inside_default, true);
        for param in params {
            if let Some(default) = &param.default_value {
                self.extract_calls_expr(graph, func_ids, caller_id, default, false);
            }
        }
        self.inside_default = outer;
    }

    fn extract_calls_stmt(
        &mut self,
        graph: &mut Graph,
        func_ids: &HashMap<String, String>,
        caller_id: &str,
        stmt: &Statement,
        is_tail: bool,
    ) {
        match stmt {
            Statement::Expr(expr) => {
                self.extract_calls_expr(graph, func_ids, caller_id, expr, is_tail);
            }
            // Bindings are never in tail position (they bind a value, not
            // return it). The value is read before the name is bound.
            Statement::Assignment(assign) => {
                self.extract_calls_expr(graph, func_ids, caller_id, &assign.value, false);
                self.bindings.bind(&assign.target);
            }
            Statement::UnmoldForward(uf) => {
                self.extract_calls_expr(graph, func_ids, caller_id, &uf.source, false);
                self.bindings.bind(&uf.target);
            }
            Statement::UnmoldBackward(ub) => {
                self.extract_calls_expr(graph, func_ids, caller_id, &ub.source, false);
                self.bindings.bind(&ub.target);
            }
            Statement::ErrorCeiling(ec) => {
                let mark = self.bindings.mark();
                self.bindings.bind(&ec.error_param);
                for body_stmt in &ec.handler_body {
                    self.extract_calls_stmt(graph, func_ids, caller_id, body_stmt, false);
                }
                self.bindings.release(mark);
            }
            // A nested function is not a node of its own; whatever it calls
            // is reachable through the function it is written in. Its name is
            // bound in its own body (it can call itself) and in what follows
            // its definition.
            Statement::FuncDef(fd) => {
                let outer = std::mem::replace(&mut self.inside_nested_function, true);
                self.bindings.bind(&fd.name);
                let mark = self.bindings.mark();
                self.bindings.bind_params(&fd.params);
                self.extract_calls_defaults(graph, func_ids, caller_id, &fd.params);
                for body_stmt in &fd.body {
                    self.extract_calls_stmt(graph, func_ids, caller_id, body_stmt, false);
                }
                self.bindings.release(mark);
                self.inside_nested_function = outer;
            }
            // The methods of a type defined inside a function run only when
            // they are called: like a nested function, they make what they
            // call reachable, not called.
            Statement::ClassLikeDef(cl) => {
                let in_function = self.inside_nested_function || self.inside_function;
                let outer = std::mem::replace(&mut self.inside_nested_function, in_function);
                for field in &cl.fields {
                    if let Some(default) = &field.default_value {
                        self.extract_calls_expr(graph, func_ids, caller_id, default, false);
                    }
                    if let Some(method) = &field.method_def {
                        let mark = self.bindings.mark();
                        self.bindings.bind_params(&method.params);
                        self.extract_calls_defaults(graph, func_ids, caller_id, &method.params);
                        for body_stmt in &method.body {
                            self.extract_calls_stmt(graph, func_ids, caller_id, body_stmt, false);
                        }
                        self.bindings.release(mark);
                    }
                }
                self.inside_nested_function = outer;
            }
            Statement::EnumDef(_) | Statement::Import(_) | Statement::Export(_) => {}
        }
    }

    fn add_call_edge(
        &self,
        graph: &mut Graph,
        func_ids: &HashMap<String, String>,
        caller_id: &str,
        name: &str,
        kind: EdgeKind,
        label: String,
    ) {
        if self.bindings.is_bound(name) {
            return;
        }
        // A parameter default is evaluated only when the argument is left
        // out: it does not make a call the function itself makes.
        let (kind, label) =
            if self.inside_default && matches!(kind, EdgeKind::Calls | EdgeKind::TailCalls) {
                (EdgeKind::References, format!("references {}", name))
            } else {
                (kind, label)
            };
        if let Some(callee_id) = func_ids.get(name) {
            graph.add_edge(GraphEdge {
                source: caller_id.to_string(),
                target: callee_id.clone(),
                kind,
                label,
                metadata: HashMap::new(),
            });
        }
    }

    fn extract_calls_expr(
        &mut self,
        graph: &mut Graph,
        func_ids: &HashMap<String, String>,
        caller_id: &str,
        expr: &Expr,
        is_tail: bool,
    ) {
        match expr {
            Expr::FuncCall(callee, args, _) => {
                if let Expr::Ident(name, _) = callee.as_ref() {
                    let (edge_kind, label_prefix) = if self.inside_nested_function {
                        (EdgeKind::References, "references")
                    } else if is_tail {
                        (EdgeKind::TailCalls, "tail calls")
                    } else {
                        (EdgeKind::Calls, "calls")
                    };
                    self.add_call_edge(
                        graph,
                        func_ids,
                        caller_id,
                        name,
                        edge_kind,
                        format!("{} {}", label_prefix, name),
                    );
                } else {
                    self.extract_calls_expr(graph, func_ids, caller_id, callee, false);
                }

                // Recurse into arguments
                for arg in args {
                    self.extract_calls_expr(graph, func_ids, caller_id, arg, false);
                }
            }

            // A function named as a value (assigned, passed as an argument)
            // may be called while the function runs, like a lambda that
            // calls it: the recursion checks count it as a call that is not
            // in tail position.
            Expr::Ident(name, _) => {
                let (edge_kind, label_prefix) = if self.inside_nested_function {
                    (EdgeKind::References, "references")
                } else {
                    (EdgeKind::Calls, "uses")
                };
                self.add_call_edge(
                    graph,
                    func_ids,
                    caller_id,
                    name,
                    edge_kind,
                    format!("{} {}", label_prefix, name),
                );
            }

            Expr::Lambda(params, body, span) => {
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

                let mark = self.bindings.mark();
                self.bindings.bind_params(params);
                for param in params {
                    if let Some(default) = &param.default_value {
                        self.extract_calls_expr(graph, func_ids, &lambda_id, default, false);
                    }
                }
                // Recurse into lambda body
                self.extract_calls_expr(graph, func_ids, &lambda_id, body, false);
                self.bindings.release(mark);
            }

            Expr::Block(stmts, _) => {
                let mark = self.bindings.mark();
                for stmt in stmts {
                    self.extract_calls_stmt(graph, func_ids, caller_id, stmt, false);
                }
                self.bindings.release(mark);
            }

            // A function named as a stage of a pipeline (`data => double =>
            // result`) is called with the value that reaches it; the first
            // item is the value the chain starts from.
            Expr::Pipeline(items, _) => {
                for (i, item) in items.iter().enumerate() {
                    match item {
                        Expr::Ident(name, _) if i > 0 => {
                            let (edge_kind, label_prefix) = if self.inside_nested_function {
                                (EdgeKind::References, "references")
                            } else {
                                (EdgeKind::Calls, "pipes to")
                            };
                            self.add_call_edge(
                                graph,
                                func_ids,
                                caller_id,
                                name,
                                edge_kind,
                                format!("{} {}", label_prefix, name),
                            );
                        }
                        other => self.extract_calls_expr(graph, func_ids, caller_id, other, false),
                    }
                }
            }

            Expr::CondBranch(arms, _) => {
                for arm in arms {
                    if let Some(cond) = &arm.condition {
                        self.extract_calls_expr(graph, func_ids, caller_id, cond, false);
                    }
                    // Propagate tail position into each arm body (last statement only).
                    // What an arm binds is not visible after it.
                    let mark = self.bindings.mark();
                    let body_len = arm.body.len();
                    for (si, stmt) in arm.body.iter().enumerate() {
                        let tail = is_tail && si + 1 == body_len;
                        self.extract_calls_stmt(graph, func_ids, caller_id, stmt, tail);
                    }
                    self.bindings.release(mark);
                }
            }

            // Every other form only passes control to its operands, fields
            // and interpolations, none of which is in tail position.
            Expr::IntLit(..)
            | Expr::FloatLit(..)
            | Expr::StringLit(..)
            | Expr::TemplateLit(..)
            | Expr::BoolLit(..)
            | Expr::Gorilla(..)
            | Expr::Placeholder(..)
            | Expr::Hole(..)
            | Expr::BuchiPack(..)
            | Expr::ListLit(..)
            | Expr::BinaryOp(..)
            | Expr::UnaryOp(..)
            | Expr::MethodCall(..)
            | Expr::FieldAccess(..)
            | Expr::MoldInst(..)
            | Expr::Unmold(..)
            | Expr::TypeInst(..)
            | Expr::EnumVariant(..)
            | Expr::TypeLiteral(..)
            | Expr::Throw(..) => {
                walk::for_each_child_of_expr(expr, &mut |child| match child {
                    Node::Expr(inner) => {
                        self.extract_calls_expr(graph, func_ids, caller_id, inner, false);
                    }
                    Node::Stmt(stmt) => {
                        self.extract_calls_stmt(graph, func_ids, caller_id, stmt, false);
                    }
                });
            }
        }
    }

    // ── High-level: extract all views ───────────────────

    /// Extract a specific graph view from a program.
    pub fn extract(&mut self, program: &Program, view: GraphView) -> Graph {
        walk::with_stack_for(program, || match view {
            GraphView::Dataflow => self.extract_dataflow(program),
            GraphView::Module => self.extract_module(program),
            GraphView::TypeHierarchy => self.extract_type_hierarchy(program),
            GraphView::Error => self.extract_error(program),
            GraphView::Call => self.extract_call(program),
        })
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
        let source = "numbers <= @[1, 2, 3]\ndoubleFn x =\n  x * 2\n=> :Int\nMap[numbers, doubleFn]() >=> doubled";
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
        let source = "add x y =\n  x + y\n=> :Int\nresult <= add(1, 2)";
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
        // (E30 Phase 7.5 / E30B-006) NodeKind::BuchiPackType → ClassLikeType に統合。
        // 旧分類は metadata["class_like_kind"] で復元可能。
        let source = "Person = @(name: Str, age: Int)";
        let graph = parse_and_extract(source, GraphView::TypeHierarchy);
        assert!(graph.nodes.iter().any(|n| {
            n.label == "Person"
                && n.kind == NodeKind::ClassLikeType
                && n.metadata.get("class_like_kind").map(|s| s.as_str()) == Some("BuchiPack")
        }));
    }

    #[test]
    fn test_type_hierarchy_error_inheritance() {
        // (E30 Phase 7.5 / E30B-006) EdgeKind::ErrorInheritance → ClassLikeInheritance に統合。
        // 旧分類は metadata["inheritance_parent"] で復元可能。
        let source = "Error => ValidationError = @(field: Str)";
        let graph = parse_and_extract(source, GraphView::TypeHierarchy);
        assert!(graph.nodes.iter().any(|n| n.label == "Error"));
        assert!(graph.nodes.iter().any(|n| n.label == "ValidationError"));
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::ClassLikeInheritance)
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
        let source = "add x y =\n  x + y\n=> :Int\n\ndouble x =\n  add(x, x)\n=> :Int\n\nresult <= double(5)";
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
        // double calls add (in tail position)
        assert!(
            graph
                .edges
                .iter()
                .any(|e| matches!(e.kind, EdgeKind::Calls | EdgeKind::TailCalls)
                    && e.label.contains("add"))
        );
    }

    #[test]
    fn test_call_graph_lambda() {
        let source =
            "numbers <= @[1, 2, 3]\ndoubleFn x =\n  x * 2\n=> :Int\nMap[numbers, doubleFn]()";
        let graph = parse_and_extract(source, GraphView::Call);
        assert!(graph.nodes.iter().any(|n| n.kind == NodeKind::Entrypoint));
    }

    #[test]
    fn test_call_graph_func_ref_in_body() {
        // A function assigned to a variable inside another function's body
        // should create a reference edge and not be considered dead code.
        let source = "helper x =\n  x + 1\n=> :Int\n\nwrapper =\n  callback <= helper\n  callback(5)\n=> :Int\n\nresult <= wrapper()";
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
        let source =
            "process x =\n  x * 2\n=> :Int\n\napply fn x =\n  fn(x)\n=> :Int\n\napply(process, 5)";
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
        let source = "helper x =\n  x + 1\n=> :Int\n\nwrapper x =\n  helper(x)\n=> :Int";
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
        let source = "add x y =\n  x + y\n=> :Int\n\ndouble x =\n  add(x, x)\n  42\n=> :Int";
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
        let source = "helper x =\n  x + 1\n=> :Int\n\nother x =\n  x * 2\n=> :Int\n\nwrapper x =\n  | x > 0 |> helper(x)\n  | _ |> other(x)\n=> :Int";
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
        let source =
            "factorial n acc =\n  | n < 1 |> acc\n  | _ |> factorial(n - 1, n * acc)\n=> :Int";
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
        // Unmold is statement-level (>=> / <=<), not expression-level
        // .unmold() parses as a MethodCall
        let source = "lax <= Lax[42]()\nlax >=> val";
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

    // ── Direction, call positions and forward references ──

    #[test]
    fn test_dataflow_edge_kinds_follow_the_written_arrow() {
        let kinds = |source: &str| -> Vec<EdgeKind> {
            let graph = parse_and_extract(source, GraphView::Dataflow);
            graph
                .edges
                .iter()
                .filter(|e| matches!(e.kind, EdgeKind::PipeForward | EdgeKind::PipeBackward))
                .map(|e| e.kind.clone())
                .collect()
        };
        assert_eq!(kinds("42 => answer"), vec![EdgeKind::PipeForward]);
        assert_eq!(kinds("answer <= 42"), vec![EdgeKind::PipeBackward]);
        // `a => f(_) => name`: two edges, both forward.
        assert_eq!(
            kinds("f x: Int =\n  x\n=> :Int\n1 => f(_) => name"),
            vec![EdgeKind::PipeForward, EdgeKind::PipeForward]
        );
        // The `<=` chain of the same shape is backward all the way.
        assert_eq!(
            kinds("f x: Int =\n  x\n=> :Int\nname <= f(_) <= 1"),
            vec![EdgeKind::PipeBackward, EdgeKind::PipeBackward]
        );
    }

    #[test]
    fn test_call_graph_sees_a_call_in_every_expression_position() {
        let source = "inc x: Int =\n  x + 1\n=> :Int\n\
                      a x: Int =\n  !inc(x)\n=> :Bool\n\
                      b x: Int =\n  @(v <= inc(x))\n=> :Int\n\
                      c x: Int =\n  `t=${inc(x)}`\n=> :Str\n\
                      d x: Int =\n  @[inc(x)]\n=> :Int\n\
                      e x: Int =\n  P(name <= inc(x))\n=> :Int\n\
                      f x: Int =\n  | x > 0 |>\n    t <= inc(x)\n    t\n  | _ |> 0\n=> :Int\n";
        let graph = parse_and_extract(source, GraphView::Call);
        for caller in ["a", "b", "c", "d", "e", "f"] {
            let caller_id = graph
                .nodes
                .iter()
                .find(|n| n.label == caller && n.kind == NodeKind::Function)
                .map(|n| n.id.clone())
                .unwrap();
            assert!(
                graph.edges.iter().any(|edge| edge.source == caller_id
                    && edge.label.contains("inc")
                    && matches!(edge.kind, EdgeKind::Calls | EdgeKind::TailCalls)),
                "{caller} should call inc: {:?}",
                graph.edges
            );
        }
    }

    #[test]
    fn test_call_graph_function_values_close_a_cycle() {
        // `Map` calls `b`, which calls `a` back: a cycle, as it would be if
        // `a` handed `Map` a lambda calling `b`.
        let source = "a xs: @[Int] =\n  Map[xs, b]() >=> ys\n  ys\n=> :@[Int]\n\
                      b x: Int =\n  a(@[x])\n=> :@[Int]\n";
        let graph = parse_and_extract(source, GraphView::Call);
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::Calls && e.label == "uses b")
        );
        match crate::graph::query::find_cycles(&graph) {
            crate::graph::query::QueryResult::Cycles(cycles) => assert!(!cycles.is_empty()),
            other => panic!("unexpected result {other}"),
        }
    }

    #[test]
    fn test_call_graph_nested_function_bodies_do_not_close_a_cycle() {
        // What a nested function calls is reachable through `a`, but `a`
        // does not call it unless it calls the nested function.
        let source = "a n: Int =\n  later x: Int =\n    b(x)\n  => :Int\n  n\n=> :Int\n\
                      b x: Int =\n  a(x) + 1\n=> :Int\n";
        let graph = parse_and_extract(source, GraphView::Call);
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::References && e.label.contains("b"))
        );
        match crate::graph::query::find_cycles(&graph) {
            crate::graph::query::QueryResult::Cycles(cycles) => assert!(cycles.is_empty()),
            other => panic!("unexpected result {other}"),
        }
    }

    #[test]
    fn test_error_view_covers_a_function_defined_after_its_caller() {
        let source = "safe x: Int =\n  |== e: Error =\n    0\n  => :Int\n  risky(x)\n=> :Int\n\
                      risky x: Int =\n  Error(type <= \"E\", message <= \"boom\").throw()\n=> :Int\n";
        let graph = parse_and_extract(source, GraphView::Error);
        match crate::graph::query::uncovered_throws(&graph) {
            crate::graph::query::QueryResult::Nodes(nodes) => {
                assert!(nodes.is_empty(), "{nodes:?}")
            }
            other => panic!("unexpected result {other}"),
        }
    }
}

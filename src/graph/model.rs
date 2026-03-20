//! Core graph model types: GraphNode, GraphEdge, Graph.
//! Types are part of the graph API for `taida verify` and future introspection features.
#![allow(dead_code)]

use std::collections::HashMap;

/// A node in the structural graph.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    /// Unique identifier: `file:line:col:kind`
    pub id: String,
    /// Node kind (view-specific)
    pub kind: NodeKind,
    /// Display label
    pub label: String,
    /// Source location
    pub location: Location,
    /// View-specific metadata
    pub metadata: HashMap<String, String>,
}

/// Source location in a Taida file.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub file: String,
    pub line: usize,
    pub column: usize,
}

/// An edge in the structural graph.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    /// Source node ID
    pub source: String,
    /// Target node ID
    pub target: String,
    /// Edge kind (view-specific)
    pub kind: EdgeKind,
    /// Display label
    pub label: String,
    /// View-specific metadata
    pub metadata: HashMap<String, String>,
}

/// The complete graph for a particular view.
#[derive(Debug, Clone)]
pub struct Graph {
    /// Which view this graph represents
    pub view: GraphView,
    /// All nodes
    pub nodes: Vec<GraphNode>,
    /// All edges
    pub edges: Vec<GraphEdge>,
    /// Graph metadata
    pub source_files: Vec<String>,
}

// ── Node Kinds ──────────────────────────────────────────

/// Node kinds across all graph views.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeKind {
    // Dataflow graph
    Variable,
    FunctionCall,
    Literal,
    BuchiPack,
    Unmold,
    Placeholder,
    Condition,

    // Module dependency graph
    Module,
    ExternalPackage,
    Symbol,

    // Type hierarchy graph
    PrimitiveType,
    BuchiPackType,
    MoldType,
    ErrorType,

    // Error boundary graph
    ErrorCeiling,
    ThrowSite,
    Function,
    GorillaCeiling,

    // Call graph
    AnonymousFn,
    Method,
    Entrypoint,
}

impl std::fmt::Display for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeKind::Variable => write!(f, "Variable"),
            NodeKind::FunctionCall => write!(f, "FunctionCall"),
            NodeKind::Literal => write!(f, "Literal"),
            NodeKind::BuchiPack => write!(f, "BuchiPack"),
            NodeKind::Unmold => write!(f, "Unmold"),
            NodeKind::Placeholder => write!(f, "Placeholder"),
            NodeKind::Condition => write!(f, "Condition"),
            NodeKind::Module => write!(f, "Module"),
            NodeKind::ExternalPackage => write!(f, "ExternalPackage"),
            NodeKind::Symbol => write!(f, "Symbol"),
            NodeKind::PrimitiveType => write!(f, "PrimitiveType"),
            NodeKind::BuchiPackType => write!(f, "BuchiPackType"),
            NodeKind::MoldType => write!(f, "MoldType"),
            NodeKind::ErrorType => write!(f, "ErrorType"),
            NodeKind::ErrorCeiling => write!(f, "ErrorCeiling"),
            NodeKind::ThrowSite => write!(f, "ThrowSite"),
            NodeKind::Function => write!(f, "Function"),
            NodeKind::GorillaCeiling => write!(f, "GorillaCeiling"),
            NodeKind::AnonymousFn => write!(f, "AnonymousFn"),
            NodeKind::Method => write!(f, "Method"),
            NodeKind::Entrypoint => write!(f, "Entrypoint"),
        }
    }
}

// ── Edge Kinds ──────────────────────────────────────────

/// Edge kinds across all graph views.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    // Dataflow
    PipeForward,
    PipeBackward,
    UnmoldForward,
    UnmoldBackward,
    Argument,
    Return,
    ConditionTrue,
    ConditionFalse,

    // Module dependency
    Imports,
    Exports,
    SymbolRef,

    // Type hierarchy
    MoldInheritance,
    ErrorInheritance,
    StructuralSubtype,

    // Error boundary
    Catches,
    ThrowsTo,
    Propagates,

    // Call graph
    Calls,
    TailCalls,
    CallsLambda,
}

impl std::fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EdgeKind::PipeForward => write!(f, "PipeForward"),
            EdgeKind::PipeBackward => write!(f, "PipeBackward"),
            EdgeKind::UnmoldForward => write!(f, "UnmoldForward"),
            EdgeKind::UnmoldBackward => write!(f, "UnmoldBackward"),
            EdgeKind::Argument => write!(f, "Argument"),
            EdgeKind::Return => write!(f, "Return"),
            EdgeKind::ConditionTrue => write!(f, "ConditionTrue"),
            EdgeKind::ConditionFalse => write!(f, "ConditionFalse"),
            EdgeKind::Imports => write!(f, "Imports"),
            EdgeKind::Exports => write!(f, "Exports"),
            EdgeKind::SymbolRef => write!(f, "SymbolRef"),
            EdgeKind::MoldInheritance => write!(f, "MoldInheritance"),
            EdgeKind::ErrorInheritance => write!(f, "ErrorInheritance"),
            EdgeKind::StructuralSubtype => write!(f, "StructuralSubtype"),
            EdgeKind::Catches => write!(f, "Catches"),
            EdgeKind::ThrowsTo => write!(f, "ThrowsTo"),
            EdgeKind::Propagates => write!(f, "Propagates"),
            EdgeKind::Calls => write!(f, "Calls"),
            EdgeKind::TailCalls => write!(f, "TailCalls"),
            EdgeKind::CallsLambda => write!(f, "CallsLambda"),
        }
    }
}

// ── Graph Views ─────────────────────────────────────────

/// The five graph views.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GraphView {
    Dataflow,
    Module,
    TypeHierarchy,
    Error,
    Call,
}

impl std::fmt::Display for GraphView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphView::Dataflow => write!(f, "dataflow"),
            GraphView::Module => write!(f, "module"),
            GraphView::TypeHierarchy => write!(f, "type-hierarchy"),
            GraphView::Error => write!(f, "error"),
            GraphView::Call => write!(f, "call"),
        }
    }
}

impl GraphView {
    pub fn parse(s: &str) -> Option<GraphView> {
        match s {
            "dataflow" => Some(GraphView::Dataflow),
            "module" => Some(GraphView::Module),
            "type-hierarchy" => Some(GraphView::TypeHierarchy),
            "error" => Some(GraphView::Error),
            "call" => Some(GraphView::Call),
            _ => None,
        }
    }
}

impl std::str::FromStr for GraphView {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or(())
    }
}

// ── Graph constructors ──────────────────────────────────

impl Graph {
    pub fn new(view: GraphView) -> Self {
        Self {
            view,
            nodes: Vec::new(),
            edges: Vec::new(),
            source_files: Vec::new(),
        }
    }

    /// Add a node, returning its ID.
    pub fn add_node(&mut self, node: GraphNode) -> String {
        let id = node.id.clone();
        // Avoid duplicates
        if !self.nodes.iter().any(|n| n.id == id) {
            self.nodes.push(node);
        }
        id
    }

    /// Add an edge.
    pub fn add_edge(&mut self, edge: GraphEdge) {
        self.edges.push(edge);
    }

    /// Make a node ID from location and kind.
    pub fn make_id(file: &str, line: usize, col: usize, kind: &NodeKind) -> String {
        format!("{}:{}:{}:{}", file, line, col, kind)
    }
}

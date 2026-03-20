//! Graph queries: path_exists, shortest_path, reachable, find_cycles, etc.
//! Functions are part of the graph API for `taida verify` and future introspection features.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};

use super::model::*;

/// Query results.
#[derive(Debug, Clone)]
pub enum QueryResult {
    Bool(bool),
    Nodes(Vec<GraphNode>),
    Cycles(Vec<Vec<String>>),
    Int(i64),
    Message(String),
}

impl std::fmt::Display for QueryResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryResult::Bool(b) => write!(f, "{}", b),
            QueryResult::Nodes(nodes) => {
                if nodes.is_empty() {
                    write!(f, "(none)")
                } else {
                    let labels: Vec<String> = nodes.iter().map(|n| n.label.clone()).collect();
                    write!(f, "[{}]", labels.join(", "))
                }
            }
            QueryResult::Cycles(cycles) => {
                if cycles.is_empty() {
                    write!(f, "No cycles found.")
                } else {
                    for (i, cycle) in cycles.iter().enumerate() {
                        if i > 0 {
                            writeln!(f)?;
                        }
                        write!(f, "Cycle: {}", cycle.join(" -> "))?;
                    }
                    Ok(())
                }
            }
            QueryResult::Int(n) => write!(f, "{}", n),
            QueryResult::Message(m) => write!(f, "{}", m),
        }
    }
}

/// Build an adjacency list from graph edges.
fn build_adjacency(graph: &Graph) -> HashMap<String, Vec<String>> {
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for node in &graph.nodes {
        adj.entry(node.id.clone()).or_default();
    }
    for edge in &graph.edges {
        adj.entry(edge.source.clone())
            .or_default()
            .push(edge.target.clone());
    }
    adj
}

/// Build a reverse adjacency list.
fn build_reverse_adjacency(graph: &Graph) -> HashMap<String, Vec<String>> {
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for node in &graph.nodes {
        adj.entry(node.id.clone()).or_default();
    }
    for edge in &graph.edges {
        adj.entry(edge.target.clone())
            .or_default()
            .push(edge.source.clone());
    }
    adj
}

/// Check if a path exists from source to target.
pub fn path_exists(graph: &Graph, source_label: &str, target_label: &str) -> QueryResult {
    let source_id = find_node_by_label(graph, source_label);
    let target_id = find_node_by_label(graph, target_label);

    let (source_id, target_id) = match (source_id, target_id) {
        (Some(s), Some(t)) => (s, t),
        _ => return QueryResult::Bool(false),
    };

    let adj = build_adjacency(graph);
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(source_id.clone());
    visited.insert(source_id);

    while let Some(current) = queue.pop_front() {
        if current == target_id {
            return QueryResult::Bool(true);
        }
        if let Some(neighbors) = adj.get(&current) {
            for next in neighbors {
                if !visited.contains(next) {
                    visited.insert(next.clone());
                    queue.push_back(next.clone());
                }
            }
        }
    }

    QueryResult::Bool(false)
}

/// Find the shortest path between two nodes.
pub fn shortest_path(graph: &Graph, source_label: &str, target_label: &str) -> QueryResult {
    let source_id = find_node_by_label(graph, source_label);
    let target_id = find_node_by_label(graph, target_label);

    let (source_id, target_id) = match (source_id, target_id) {
        (Some(s), Some(t)) => (s, t),
        _ => return QueryResult::Nodes(vec![]),
    };

    let adj = build_adjacency(graph);
    let mut visited = HashSet::new();
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut queue = VecDeque::new();
    queue.push_back(source_id.clone());
    visited.insert(source_id.clone());

    while let Some(current) = queue.pop_front() {
        if current == target_id {
            // Reconstruct path
            let mut path = vec![current.clone()];
            let mut cur = current;
            while let Some(prev) = parent.get(&cur) {
                path.push(prev.clone());
                cur = prev.clone();
            }
            path.reverse();

            let nodes: Vec<GraphNode> = path
                .iter()
                .filter_map(|id| graph.nodes.iter().find(|n| &n.id == id).cloned())
                .collect();
            return QueryResult::Nodes(nodes);
        }
        if let Some(neighbors) = adj.get(&current) {
            for next in neighbors {
                if !visited.contains(next) {
                    visited.insert(next.clone());
                    parent.insert(next.clone(), current.clone());
                    queue.push_back(next.clone());
                }
            }
        }
    }

    QueryResult::Nodes(vec![])
}

/// Find all nodes reachable from a given node.
pub fn reachable(graph: &Graph, source_label: &str) -> QueryResult {
    let source_id = match find_node_by_label(graph, source_label) {
        Some(id) => id,
        None => return QueryResult::Nodes(vec![]),
    };

    let adj = build_adjacency(graph);
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(source_id.clone());
    visited.insert(source_id.clone());

    while let Some(current) = queue.pop_front() {
        if let Some(neighbors) = adj.get(&current) {
            for next in neighbors {
                if !visited.contains(next) {
                    visited.insert(next.clone());
                    queue.push_back(next.clone());
                }
            }
        }
    }

    // Exclude the source itself
    visited.remove(&source_id);
    let nodes: Vec<GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| visited.contains(&n.id))
        .cloned()
        .collect();
    QueryResult::Nodes(nodes)
}

/// Find all cycles in the graph.
pub fn find_cycles(graph: &Graph) -> QueryResult {
    let adj = build_adjacency(graph);
    let mut visited = HashSet::new();
    let mut rec_stack = HashSet::new();
    let mut cycles = Vec::new();

    for node in &graph.nodes {
        if !visited.contains(&node.id) {
            let mut path = Vec::new();
            dfs_find_cycles(
                &node.id,
                &adj,
                &mut visited,
                &mut rec_stack,
                &mut path,
                &mut cycles,
                graph,
            );
        }
    }

    QueryResult::Cycles(cycles)
}

fn dfs_find_cycles(
    node: &str,
    adj: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    rec_stack: &mut HashSet<String>,
    path: &mut Vec<String>,
    cycles: &mut Vec<Vec<String>>,
    graph: &Graph,
) {
    visited.insert(node.to_string());
    rec_stack.insert(node.to_string());
    path.push(node.to_string());

    if let Some(neighbors) = adj.get(node) {
        for next in neighbors {
            if !visited.contains(next) {
                dfs_find_cycles(next, adj, visited, rec_stack, path, cycles, graph);
            } else if rec_stack.contains(next) {
                // Found a cycle - extract the cycle from path
                if let Some(start) = path.iter().position(|n| n == next) {
                    let cycle: Vec<String> = path[start..]
                        .iter()
                        .map(|id| {
                            graph
                                .nodes
                                .iter()
                                .find(|n| &n.id == id)
                                .map(|n| n.label.clone())
                                .unwrap_or_else(|| id.clone())
                        })
                        .collect();
                    cycles.push(cycle);
                }
            }
        }
    }

    path.pop();
    rec_stack.remove(node);
}

/// Find uncovered throw sites (throws without a corresponding error ceiling).
///
/// A throw site is considered "covered" if:
/// 1. It has a direct `ThrowsTo` edge to an error ceiling (same-function coverage), OR
/// 2. It is inside a function that is transitively covered via `Propagates` edges:
///    - A `Propagates` edge from an `ErrorCeiling` to the function means the ceiling
///      covers the function's throws (cross-function coverage).
///    - A `Propagates` edge from another `Function` to the function means the throws
///      propagate to the caller; if the caller is itself covered, the callee is too.
pub fn uncovered_throws(graph: &Graph) -> QueryResult {
    let throw_sites: Vec<&GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ThrowSite)
        .collect();

    // Step 1: Find directly covered throws (same-function ThrowsTo edges)
    let directly_covered: HashSet<String> = graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::ThrowsTo)
        .map(|e| e.source.clone())
        .collect();

    // Step 2: Find functions that are transitively covered via Propagates edges
    let covered_functions = find_covered_functions(graph);

    // Step 3: A throw is uncovered if:
    //   - It is NOT directly covered (no ThrowsTo edge), AND
    //   - Its enclosing function is NOT in the covered_functions set
    let uncovered: Vec<GraphNode> = throw_sites
        .into_iter()
        .filter(|n| {
            if directly_covered.contains(&n.id) {
                return false; // directly covered
            }
            // Check if the enclosing function is covered
            if let Some(func_id) = n.metadata.get("enclosing_function") {
                !covered_functions.contains(func_id)
            } else {
                true // top-level throw without ceiling is uncovered
            }
        })
        .cloned()
        .collect();

    QueryResult::Nodes(uncovered)
}

/// Find all function nodes that are transitively covered by error ceilings
/// via Propagates edges.
///
/// A function is "covered" if:
/// - An ErrorCeiling has a Propagates edge pointing to it, OR
/// - A covered Function has a Propagates edge pointing to it (transitive)
fn find_covered_functions(graph: &Graph) -> HashSet<String> {
    // Collect Propagates edges: source -> target (target is the callee function)
    let propagates_edges: Vec<(&str, &str)> = graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Propagates)
        .map(|e| (e.source.as_str(), e.target.as_str()))
        .collect();

    // Build reverse map: callee_function_id -> list of source IDs (ceilings or caller functions)
    let mut callee_to_sources: HashMap<&str, Vec<&str>> = HashMap::new();
    for (source, target) in &propagates_edges {
        callee_to_sources.entry(target).or_default().push(source);
    }

    // Identify ceiling node IDs
    let ceiling_ids: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ErrorCeiling)
        .map(|n| n.id.as_str())
        .collect();

    // BFS/iterative: seed with functions directly covered by ceilings
    let mut covered: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    for (target, sources) in &callee_to_sources {
        for source in sources {
            if ceiling_ids.contains(source) {
                let target_str = target.to_string();
                if !covered.contains(&target_str) {
                    covered.insert(target_str.clone());
                    queue.push_back(target_str);
                }
            }
        }
    }

    // Propagate transitively: if function F is covered, and F has a Propagates edge
    // to function G (meaning F calls G without its own ceiling), then G is also covered.
    // Wait -- that's backwards. If F calls G, Propagates edge is F -> G.
    // If F is covered (by a ceiling), G's throws are also covered because F's caller
    // has a ceiling.
    //
    // Actually, let's re-examine the edge semantics:
    // - ErrorCeiling -> Function(callee): ceiling covers callee's throws
    // - Function(caller) -> Function(callee): callee's throws propagate to caller
    //   This means if caller is covered, callee is also covered.
    //
    // So we need: for each covered function, find all functions it has Propagates edges TO
    // (its callees), and mark those as covered too.

    // Build forward map: function_id -> list of callee function IDs it propagates to
    let function_ids: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Function)
        .map(|n| n.id.as_str())
        .collect();

    let mut caller_to_callees: HashMap<&str, Vec<&str>> = HashMap::new();
    for (source, target) in &propagates_edges {
        if function_ids.contains(source) {
            caller_to_callees.entry(source).or_default().push(target);
        }
    }

    while let Some(covered_fn) = queue.pop_front() {
        // Find all callees of this covered function
        if let Some(callees) = caller_to_callees.get(covered_fn.as_str()) {
            for callee in callees {
                let callee_str = callee.to_string();
                if !covered.contains(&callee_str) {
                    covered.insert(callee_str.clone());
                    queue.push_back(callee_str);
                }
            }
        }
    }

    covered
}

/// Find unreachable functions (not reachable from any entrypoint).
pub fn unreachable_functions(graph: &Graph) -> QueryResult {
    let adj = build_adjacency(graph);

    // Find all entrypoints
    let entrypoints: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Entrypoint)
        .map(|n| n.id.clone())
        .collect();

    // BFS from all entrypoints
    let mut reachable_set = HashSet::new();
    let mut queue = VecDeque::new();
    for ep in &entrypoints {
        queue.push_back(ep.clone());
        reachable_set.insert(ep.clone());
    }
    while let Some(current) = queue.pop_front() {
        if let Some(neighbors) = adj.get(&current) {
            for next in neighbors {
                if !reachable_set.contains(next) {
                    reachable_set.insert(next.clone());
                    queue.push_back(next.clone());
                }
            }
        }
    }

    // Find functions that are not reachable
    let unreachable: Vec<GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Function && !reachable_set.contains(&n.id))
        .cloned()
        .collect();

    QueryResult::Nodes(unreachable)
}

/// Get dependents (nodes that depend on the given node).
pub fn dependents(graph: &Graph, label: &str) -> QueryResult {
    let node_id = match find_node_by_label(graph, label) {
        Some(id) => id,
        None => return QueryResult::Nodes(vec![]),
    };

    let rev_adj = build_reverse_adjacency(graph);
    let incoming: Vec<GraphNode> = rev_adj
        .get(&node_id)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| graph.nodes.iter().find(|n| &n.id == id).cloned())
                .collect()
        })
        .unwrap_or_default();

    QueryResult::Nodes(incoming)
}

/// Get dependencies (nodes the given node depends on).
pub fn dependencies(graph: &Graph, label: &str) -> QueryResult {
    let node_id = match find_node_by_label(graph, label) {
        Some(id) => id,
        None => return QueryResult::Nodes(vec![]),
    };

    let adj = build_adjacency(graph);
    let outgoing: Vec<GraphNode> = adj
        .get(&node_id)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| graph.nodes.iter().find(|n| &n.id == id).cloned())
                .collect()
        })
        .unwrap_or_default();

    QueryResult::Nodes(outgoing)
}

/// Fan-in: number of edges coming into a node.
pub fn fan_in(graph: &Graph, label: &str) -> QueryResult {
    let node_id = match find_node_by_label(graph, label) {
        Some(id) => id,
        None => return QueryResult::Int(0),
    };

    let count = graph.edges.iter().filter(|e| e.target == node_id).count();
    QueryResult::Int(count as i64)
}

/// Fan-out: number of edges going out from a node.
pub fn fan_out(graph: &Graph, label: &str) -> QueryResult {
    let node_id = match find_node_by_label(graph, label) {
        Some(id) => id,
        None => return QueryResult::Int(0),
    };

    let count = graph.edges.iter().filter(|e| e.source == node_id).count();
    QueryResult::Int(count as i64)
}

/// Parse and execute a query string.
pub fn execute_query(graph: &Graph, query: &str) -> QueryResult {
    let query = query.trim();

    if let Some(args) = parse_query_call(query, "path_exists")
        && args.len() == 2
    {
        return path_exists(graph, &args[0], &args[1]);
    }
    if let Some(args) = parse_query_call(query, "shortest_path")
        && args.len() == 2
    {
        return shortest_path(graph, &args[0], &args[1]);
    }
    if let Some(args) = parse_query_call(query, "reachable")
        && args.len() == 1
    {
        return reachable(graph, &args[0]);
    }
    if parse_query_call(query, "find_cycles").is_some() {
        return find_cycles(graph);
    }
    if parse_query_call(query, "uncovered_throws").is_some() {
        return uncovered_throws(graph);
    }
    if parse_query_call(query, "unreachable_functions").is_some() {
        return unreachable_functions(graph);
    }
    if let Some(args) = parse_query_call(query, "dependents")
        && args.len() == 1
    {
        return dependents(graph, &args[0]);
    }
    if let Some(args) = parse_query_call(query, "dependencies")
        && args.len() == 1
    {
        return dependencies(graph, &args[0]);
    }
    if let Some(args) = parse_query_call(query, "fan_in")
        && args.len() == 1
    {
        return fan_in(graph, &args[0]);
    }
    if let Some(args) = parse_query_call(query, "fan_out")
        && args.len() == 1
    {
        return fan_out(graph, &args[0]);
    }

    QueryResult::Message(format!("Unknown query: {}", query))
}

/// Helper: find a node ID by its label.
fn find_node_by_label(graph: &Graph, label: &str) -> Option<String> {
    graph
        .nodes
        .iter()
        .find(|n| n.label == label)
        .map(|n| n.id.clone())
}

/// Parse a query function call like "func_name(arg1, arg2)".
fn parse_query_call(query: &str, name: &str) -> Option<Vec<String>> {
    let prefix = format!("{}(", name);
    if !query.starts_with(&prefix) || !query.ends_with(')') {
        return None;
    }
    let inner = &query[prefix.len()..query.len() - 1];
    if inner.is_empty() {
        return Some(vec![]);
    }
    let args: Vec<String> = inner.split(',').map(|s| s.trim().to_string()).collect();
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::extract::GraphExtractor;

    fn parse_and_query(source: &str, view: GraphView, query: &str) -> QueryResult {
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut extractor = GraphExtractor::new("test.td");
        let graph = extractor.extract(&program, view);
        execute_query(&graph, query)
    }

    #[test]
    fn test_find_cycles_no_cycle() {
        let source = "x <= 42\ny <= x + 1";
        let result = parse_and_query(source, GraphView::Dataflow, "find_cycles()");
        match result {
            QueryResult::Cycles(cycles) => assert!(cycles.is_empty()),
            _ => panic!("Expected Cycles result"),
        }
    }

    #[test]
    fn test_path_exists_simple() {
        let source = "add x y =\n  x + y\nresult <= add(1, 2)";
        let graph = {
            let (program, errors) = crate::parser::parse(source);
            assert!(errors.is_empty());
            let mut extractor = GraphExtractor::new("test.td");
            extractor.extract(&program, GraphView::Dataflow)
        };
        // There should be some reachable nodes from add(...)
        let result = reachable(&graph, "result");
        // result is an assignment target, should exist as a node
        match result {
            QueryResult::Nodes(_) => {} // OK
            _ => panic!("Expected Nodes result"),
        }
    }

    #[test]
    fn test_uncovered_throws_with_ceiling() {
        let source =
            "process input =\n  |== error: Error =\n    \"default\"\n  => :Str\n  input\n=> :Str";
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty());
        let mut extractor = GraphExtractor::new("test.td");
        let graph = extractor.extract(&program, GraphView::Error);
        let result = uncovered_throws(&graph);
        match result {
            QueryResult::Nodes(nodes) => assert!(nodes.is_empty()),
            _ => panic!("Expected Nodes result"),
        }
    }

    #[test]
    fn test_query_parser() {
        assert_eq!(
            parse_query_call("path_exists(a, b)", "path_exists"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            parse_query_call("find_cycles()", "find_cycles"),
            Some(vec![])
        );
        assert_eq!(
            parse_query_call("reachable(input)", "reachable"),
            Some(vec!["input".to_string()])
        );
        assert!(parse_query_call("unknown()", "path_exists").is_none());
    }

    // ── Cross-function error coverage (V-4) ──

    #[test]
    fn test_uncovered_throws_cross_function_covered() {
        // `risky` has an uncovered throw.
        // `safe` calls `risky` under a ceiling.
        // The throw in `risky` should be considered covered via cross-function propagation.
        let source = "risky x =
  Error(message <= \"boom\").throw()
=> :Str

safe input =
  |== e: Error =
    \"default\"
  => :Str
  risky(input)
=> :Str";
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut extractor = GraphExtractor::new("test.td");
        let graph = extractor.extract(&program, GraphView::Error);
        let result = uncovered_throws(&graph);
        match result {
            QueryResult::Nodes(nodes) => {
                assert!(
                    nodes.is_empty(),
                    "risky's throw should be covered by safe's ceiling via Propagates. Uncovered: {:?}",
                    nodes.iter().map(|n| &n.label).collect::<Vec<_>>()
                );
            }
            _ => panic!("Expected Nodes result"),
        }
    }

    #[test]
    fn test_uncovered_throws_transitive_coverage() {
        // `inner` has an uncovered throw.
        // `middle` calls `inner` without a ceiling (propagates upward).
        // `outer` calls `middle` under a ceiling.
        // The throw in `inner` should be considered covered transitively.
        let source = "inner x =
  Error(message <= \"boom\").throw()
=> :Str

middle x =
  inner(x)
=> :Str

outer input =
  |== e: Error =
    \"default\"
  => :Str
  middle(input)
=> :Str";
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);

        let mut extractor = GraphExtractor::new("test.td");
        let graph = extractor.extract(&program, GraphView::Error);

        let result = uncovered_throws(&graph);
        match result {
            QueryResult::Nodes(nodes) => {
                assert!(
                    nodes.is_empty(),
                    "inner's throw should be covered transitively via middle -> outer's ceiling. Uncovered: {:?}",
                    nodes.iter().map(|n| &n.label).collect::<Vec<_>>()
                );
            }
            _ => panic!("Expected Nodes result"),
        }
    }

    #[test]
    fn test_uncovered_throws_no_coverage() {
        // `risky` has a throw, no ceiling anywhere.
        // The throw should remain uncovered.
        let source = "risky x =
  Error(message <= \"boom\").throw()
=> :Str";
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut extractor = GraphExtractor::new("test.td");
        let graph = extractor.extract(&program, GraphView::Error);
        let result = uncovered_throws(&graph);
        match result {
            QueryResult::Nodes(nodes) => {
                assert_eq!(nodes.len(), 1, "Should have exactly 1 uncovered throw");
            }
            _ => panic!("Expected Nodes result"),
        }
    }

    #[test]
    fn test_uncovered_throws_partial_coverage() {
        // `risky1` and `risky2` both have throws.
        // `safe` calls `risky1` under a ceiling, but NOT `risky2`.
        // `risky1`'s throw should be covered, `risky2`'s should not.
        let source = "risky1 x =
  Error(message <= \"boom1\").throw()
=> :Str

risky2 x =
  Error(message <= \"boom2\").throw()
=> :Str

safe input =
  |== e: Error =
    \"default\"
  => :Str
  risky1(input)
=> :Str";
        let (program, errors) = crate::parser::parse(source);
        assert!(errors.is_empty(), "Parse errors: {:?}", errors);
        let mut extractor = GraphExtractor::new("test.td");
        let graph = extractor.extract(&program, GraphView::Error);
        let result = uncovered_throws(&graph);
        match result {
            QueryResult::Nodes(nodes) => {
                // Only risky2's throw should be uncovered
                assert_eq!(
                    nodes.len(),
                    1,
                    "Should have exactly 1 uncovered throw (risky2). Got: {:?}",
                    nodes.iter().map(|n| &n.label).collect::<Vec<_>>()
                );
            }
            _ => panic!("Expected Nodes result"),
        }
    }

    // ── BT-17: Graph cycle detection tests ──

    fn make_node(id: &str, label: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: NodeKind::Variable,
            label: label.to_string(),
            location: Location {
                file: "test.td".to_string(),
                line: 1,
                column: 1,
            },
            metadata: HashMap::new(),
        }
    }

    fn make_edge(source: &str, target: &str) -> GraphEdge {
        GraphEdge {
            source: source.to_string(),
            target: target.to_string(),
            kind: EdgeKind::PipeForward,
            label: "".to_string(),
            metadata: HashMap::new(),
        }
    }

    fn make_graph(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Graph {
        Graph {
            view: GraphView::Dataflow,
            nodes,
            edges,
            source_files: vec!["test.td".to_string()],
        }
    }

    #[test]
    fn test_find_cycles_self_reference() {
        // A -> A (self-loop)
        let graph = make_graph(vec![make_node("a", "A")], vec![make_edge("a", "a")]);
        let result = find_cycles(&graph);
        match result {
            QueryResult::Cycles(cycles) => {
                assert!(
                    !cycles.is_empty(),
                    "Self-referencing node should produce a cycle"
                );
                // The cycle should contain "A"
                assert!(
                    cycles.iter().any(|c| c.contains(&"A".to_string())),
                    "Cycle should include node A, got: {:?}",
                    cycles
                );
            }
            _ => panic!("Expected Cycles result"),
        }
    }

    #[test]
    fn test_find_cycles_two_node_cycle() {
        // A -> B -> A
        let graph = make_graph(
            vec![make_node("a", "A"), make_node("b", "B")],
            vec![make_edge("a", "b"), make_edge("b", "a")],
        );
        let result = find_cycles(&graph);
        match result {
            QueryResult::Cycles(cycles) => {
                assert!(
                    !cycles.is_empty(),
                    "Two-node cycle (A->B->A) should be detected"
                );
            }
            _ => panic!("Expected Cycles result"),
        }
    }

    #[test]
    fn test_find_cycles_three_node_cycle() {
        // A -> B -> C -> A
        let graph = make_graph(
            vec![
                make_node("a", "A"),
                make_node("b", "B"),
                make_node("c", "C"),
            ],
            vec![
                make_edge("a", "b"),
                make_edge("b", "c"),
                make_edge("c", "a"),
            ],
        );
        let result = find_cycles(&graph);
        match result {
            QueryResult::Cycles(cycles) => {
                assert!(
                    !cycles.is_empty(),
                    "Three-node cycle (A->B->C->A) should be detected"
                );
                // Cycle should include all three nodes
                let all_labels: Vec<String> = cycles.iter().flatten().cloned().collect();
                assert!(
                    all_labels.contains(&"A".to_string())
                        && all_labels.contains(&"B".to_string())
                        && all_labels.contains(&"C".to_string()),
                    "Cycle should include A, B, C, got: {:?}",
                    cycles
                );
            }
            _ => panic!("Expected Cycles result"),
        }
    }

    #[test]
    fn test_find_cycles_acyclic_graph() {
        // A -> B -> C (no cycle)
        let graph = make_graph(
            vec![
                make_node("a", "A"),
                make_node("b", "B"),
                make_node("c", "C"),
            ],
            vec![make_edge("a", "b"), make_edge("b", "c")],
        );
        let result = find_cycles(&graph);
        match result {
            QueryResult::Cycles(cycles) => {
                assert!(
                    cycles.is_empty(),
                    "Acyclic graph should have no cycles, got: {:?}",
                    cycles
                );
            }
            _ => panic!("Expected Cycles result"),
        }
    }
}

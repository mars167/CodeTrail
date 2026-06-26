//! Graph builder: construct the call graph from SCIP occurrence data and
//! tree-sitter heuristics.
//!
//! ## Build strategy
//!
//! 1. **SCIP path** — When a SCIP occurrence DB exists and is fresh, read
//!    symbol/occurrence records to extract function-definition locations and
//!    reference relationships.  Every reference from one function scope to
//!    another is recorded as a CALLS edge with `source: scip_precise`.
//!
//! 2. **Tree-sitter path** — Always run tree-sitter AST traversal as a
//!    supplement.  Call expressions discovered this way carry
//!    `source: tree_sitter_heuristic`.
//!
//! 3. Edges are NOT duplicated: if SCIP already provided a precise edge
//!    for a given (caller, callee, site), the tree-sitter duplicate is
//!    skipped.

use std::collections::HashMap;

use anyhow::Result;
use petgraph::{graph::NodeIndex, visit::EdgeRef};

use crate::{
    lsp::scip_gen,
    scip::{self, store::OccurrenceResult},
    scip_index::native_db_path,
    syntax,
    workspace::{ScanOptions, Workspace},
};

use super::{
    schema::{EdgeMetadata, EdgeSource, GraphNode, NodeKind, ReliabilityLevel},
    PetgraphBackend,
};

/// Build the petgraph backend from the workspace.
///
/// This function is called by [`PetgraphBackend::build`].
pub(crate) fn build_petgraph_backend(
    backend: &mut PetgraphBackend,
    workspace: &Workspace,
) -> Result<()> {
    // Reset graphs
    backend.graph = petgraph::graph::DiGraph::new();
    backend.node_by_id = HashMap::new();
    backend.snapshot_id = workspace.snapshot_id.clone();

    let tree_candidates = collect_tree_candidates(workspace);
    let call_sites = tree_sitter_call_sites(&tree_candidates);

    // --- Phase 1: register nodes from SCIP symbols ---
    build_from_scip(backend, workspace, &call_sites);

    // --- Phase 2: tree-sitter call edges ---
    build_tree_sitter_edges(backend, &tree_candidates);

    Ok(())
}

#[derive(Clone)]
struct ScipDefinition {
    symbol_key: String,
    name: String,
    language: String,
    path: String,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

impl ScipDefinition {
    const fn range(&self) -> SourceRange {
        SourceRange::new(
            self.start_line,
            self.start_column,
            self.end_line,
            self.end_column,
        )
    }
}

#[derive(Clone, Copy)]
struct SourceRange {
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

impl SourceRange {
    const fn new(start_line: u32, start_column: u32, end_line: u32, end_column: u32) -> Self {
        Self {
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }

    const fn contains(self, inner: Self) -> bool {
        self.starts_before_or_at(inner.start_line, inner.start_column)
            && self.ends_after_or_at(inner.end_line, inner.end_column)
    }

    const fn starts_before_or_at(self, line: u32, column: u32) -> bool {
        self.start_line < line || (self.start_line == line && self.start_column <= column)
    }

    const fn ends_after_or_at(self, line: u32, column: u32) -> bool {
        self.end_line > line || (self.end_line == line && self.end_column >= column)
    }
}

fn enclosing_definition<'a>(
    def_locations: &'a HashMap<String, ScipDefinition>,
    path: &str,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
) -> Option<&'a ScipDefinition> {
    let inner = SourceRange::new(start_line, start_column, end_line, end_column);
    def_locations
        .values()
        .filter(|definition| definition.path == path)
        .filter(|definition| definition.range().contains(inner))
        .max_by_key(|definition| (definition.start_line, definition.start_column))
        .or_else(|| {
            def_locations
                .values()
                .filter(|definition| definition.path == path)
                .filter(|definition| {
                    (definition.start_line, definition.start_column) <= (start_line, start_column)
                })
                .max_by_key(|definition| (definition.start_line, definition.start_column))
        })
}

fn edge_exists_at_site(
    backend: &PetgraphBackend,
    caller_idx: NodeIndex,
    callee_idx: NodeIndex,
    file_path: &str,
    call_line: u32,
    call_column: u32,
) -> bool {
    backend
        .graph
        .edges_directed(caller_idx, petgraph::Direction::Outgoing)
        .any(|edge| {
            let meta = edge.weight();
            edge.target() == callee_idx
                && meta.file_path == file_path
                && meta.call_line == call_line
                && meta.call_column == call_column
        })
}

fn build_from_scip(
    backend: &mut PetgraphBackend,
    workspace: &Workspace,
    call_sites: &[TreeCallSite],
) {
    let db_path = native_db_path(workspace);
    if !db_path.exists() {
        return;
    }
    if !scip::occurrence_db_fresh(&db_path, &workspace.snapshot_id, &workspace.root) {
        return;
    }
    if !scip_gen::generation_manifests_allow_precise_use(workspace).unwrap_or(false) {
        return;
    }
    // Read all symbols with their definitions
    let Ok(symbols) = scip::query_symbols(&db_path, "") else {
        return;
    };

    let mut def_locations: HashMap<String, ScipDefinition> = HashMap::new();

    for sym in &symbols {
        if sym.role == "definition" && is_callable_kind(&sym.kind) {
            def_locations.insert(
                sym.symbol_key.clone(),
                ScipDefinition {
                    symbol_key: sym.symbol_key.clone(),
                    name: sym.name.clone(),
                    language: sym.language.clone(),
                    path: sym.path.clone(),
                    start_line: sym.start_line,
                    start_column: sym.start_column,
                    end_line: sym.end_line,
                    end_column: sym.end_column,
                },
            );
        }
    }

    for definition in def_locations.values() {
        backend.ensure_node(GraphNode {
            id: definition.symbol_key.clone(),
            display_name: definition.name.clone(),
            signature: Some(definition.name.clone()),
            container: None,
            kind: NodeKind::Function,
            language: definition.language.clone(),
            file_path: definition.path.clone(),
            start_line: definition.start_line,
            start_column: definition.start_column,
            end_line: definition.end_line,
            end_column: definition.end_column,
        });
    }

    for sym in def_locations.values() {
        let Ok(refs) = scip::query_refs_by_symbol_key(&db_path, &sym.symbol_key) else {
            continue;
        };
        for r in refs {
            if !is_reference_at_call_site(&call_sites, &r) {
                continue;
            }
            let enclosing = enclosing_definition(
                &def_locations,
                &r.path,
                r.start_line,
                r.start_column,
                r.end_line,
                r.end_column,
            );

            if let Some(caller) = enclosing {
                backend.ensure_node(GraphNode {
                    id: r.symbol_key.clone(),
                    display_name: r.name.clone(),
                    signature: Some(r.name.clone()),
                    container: None,
                    kind: NodeKind::Function,
                    language: r.language.clone(),
                    file_path: r.path.clone(),
                    start_line: r.start_line,
                    start_column: r.start_column,
                    end_line: r.end_line,
                    end_column: r.end_column,
                });

                let (Some(&caller_idx), Some(&callee_idx)) = (
                    backend.node_by_id.get(&caller.symbol_key),
                    backend.node_by_id.get(&r.symbol_key),
                ) else {
                    continue;
                };

                if !edge_exists_at_site(
                    backend,
                    caller_idx,
                    callee_idx,
                    &r.path,
                    r.start_line,
                    r.start_column,
                ) {
                    backend.graph.add_edge(
                        caller_idx,
                        callee_idx,
                        EdgeMetadata {
                            source: EdgeSource::ScipPrecise,
                            level: ReliabilityLevel::InferredCandidate,
                            file_path: r.path.clone(),
                            call_line: r.start_line,
                            call_column: r.start_column,
                            caller_id: caller.symbol_key.clone(),
                            callee_id: r.symbol_key.clone(),
                            language: r.language.clone(),
                            file_hash: r.file_hash.clone(),
                        },
                    );
                }
            }
        }
    }
}

/// Add call edges from tree-sitter AST traversal.
fn build_tree_sitter_edges(
    backend: &mut PetgraphBackend,
    candidates: &[syntax::TreeSitterCandidate],
) {
    let mut definitions_by_body_hash = HashMap::<String, GraphNode>::new();
    let mut definitions_by_name = HashMap::<String, Vec<GraphNode>>::new();
    let mut definitions_by_scoped_name = HashMap::<String, Vec<GraphNode>>::new();
    for candidate in candidates.iter().filter(|candidate| {
        candidate.kind != "call"
            && candidate
                .symbol_kind
                .as_deref()
                .is_some_and(is_callable_kind)
            && candidate.name.is_some()
    }) {
        let node = graph_node_from_tree_symbol(candidate);
        backend.ensure_node(node.clone());
        if let Some(body_hash) = candidate.body_hash.as_ref() {
            definitions_by_body_hash.insert(body_hash.clone(), node.clone());
        }
        let key = tree_symbol_lookup_key(
            &candidate.language,
            &candidate.root_id,
            candidate.name.as_deref().unwrap_or(""),
        );
        definitions_by_name
            .entry(key)
            .or_default()
            .push(node.clone());
        if let Some(container) = candidate.container.as_deref() {
            let scoped_key = tree_symbol_scoped_lookup_key(
                &candidate.language,
                &candidate.root_id,
                container,
                candidate.name.as_deref().unwrap_or(""),
            );
            definitions_by_scoped_name
                .entry(scoped_key)
                .or_default()
                .push(node.clone());
        }
    }

    for call in candidates
        .iter()
        .filter(|candidate| candidate.kind == "call")
    {
        // Register caller function node
        let Some(caller_node) = call
            .body_hash
            .as_ref()
            .and_then(|hash| definitions_by_body_hash.get(hash))
            .cloned()
            .or_else(|| fallback_caller_node_from_call(call))
        else {
            continue;
        };
        let caller_id = caller_node.id.clone();

        let call_line = range_line(&call.range, "start");
        let call_col = range_column(&call.range, "start");

        // Ensure caller node
        backend.ensure_node(caller_node);

        // Ensure callee node (the call target is the identifier being called)
        let target = call.target.as_deref().unwrap_or_default();
        let target_name = syntax::last_identifier(target);
        let callee_node = resolve_tree_callee(
            &definitions_by_name,
            &definitions_by_scoped_name,
            call,
            target,
            target_name,
        )
        .unwrap_or_else(|| unresolved_callee_node_from_call(call, target));
        let callee_id = callee_node.id.clone();
        backend.ensure_node(callee_node);

        let caller_idx = backend.node_by_id[&caller_id];
        let callee_idx = backend.node_by_id[&callee_id];

        if !edge_exists_at_site(
            backend, caller_idx, callee_idx, &call.path, call_line, call_col,
        ) {
            backend.graph.add_edge(
                caller_idx,
                callee_idx,
                EdgeMetadata {
                    source: EdgeSource::TreeSitterHeuristic,
                    level: ReliabilityLevel::InferredCandidate,
                    file_path: call.path.clone(),
                    call_line,
                    call_column: call_col,
                    caller_id,
                    callee_id,
                    language: call.language.clone(),
                    file_hash: call.file_hash.clone(),
                },
            );
        }
    }
}

fn collect_tree_candidates(workspace: &Workspace) -> Vec<syntax::TreeSitterCandidate> {
    let scan_opts = ScanOptions {
        include: Vec::new(),
        exclude: Vec::new(),
        hidden: false,
        no_ignore: false,
        lang: Vec::new(),
        changed: false,
        cursor: None,
        allow_broad: false,
        limit: 0,
        ..ScanOptions::default()
    };
    let mut warnings = Vec::new();
    syntax::collect_candidates(workspace, &scan_opts, &mut warnings).unwrap_or_default()
}

#[derive(Clone, Debug)]
struct TreeCallSite {
    path: String,
    target_name: String,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

impl TreeCallSite {
    const fn contains(&self, line: u32, column: u32) -> bool {
        (self.start_line < line || (self.start_line == line && self.start_column <= column))
            && (self.end_line > line || (self.end_line == line && self.end_column >= column))
    }
}

fn tree_sitter_call_sites(candidates: &[syntax::TreeSitterCandidate]) -> Vec<TreeCallSite> {
    candidates
        .iter()
        .filter(|candidate| candidate.kind == "call")
        .filter_map(|call| {
            let target = call.target.as_deref()?;
            Some(TreeCallSite {
                path: call.path.clone(),
                target_name: syntax::last_identifier(target).to_string(),
                start_line: range_line(&call.range, "start"),
                start_column: range_column(&call.range, "start"),
                end_line: range_line(&call.range, "end"),
                end_column: range_column(&call.range, "end"),
            })
        })
        .collect()
}

fn is_reference_at_call_site(call_sites: &[TreeCallSite], occurrence: &OccurrenceResult) -> bool {
    call_sites.iter().any(|site| {
        site.path == occurrence.path
            && site.contains(occurrence.start_line, occurrence.start_column)
            && (site.target_name == occurrence.name
                || syntax::last_identifier(&occurrence.name) == site.target_name)
    })
}

fn graph_node_from_tree_symbol(candidate: &syntax::TreeSitterCandidate) -> GraphNode {
    let name = candidate.name.as_deref().unwrap_or("<anonymous>");
    GraphNode {
        id: tree_symbol_id(candidate),
        display_name: name.to_string(),
        signature: Some(tree_symbol_signature(candidate, name)),
        container: candidate.container.clone(),
        kind: NodeKind::Function,
        language: candidate.language.clone(),
        file_path: candidate.path.clone(),
        start_line: range_line(&candidate.range, "start"),
        start_column: range_column(&candidate.range, "start"),
        end_line: range_line(&candidate.range, "end"),
        end_column: range_column(&candidate.range, "end"),
    }
}

fn fallback_caller_node_from_call(candidate: &syntax::TreeSitterCandidate) -> Option<GraphNode> {
    let name = candidate.enclosing_symbol.as_deref()?;
    Some(GraphNode {
        id: format!("parser:{}:{}:{}", candidate.language, candidate.path, name),
        display_name: name.to_string(),
        signature: Some(name.to_string()),
        container: None,
        kind: NodeKind::Function,
        language: candidate.language.clone(),
        file_path: candidate.path.clone(),
        start_line: range_line(&candidate.range, "start"),
        start_column: range_column(&candidate.range, "start"),
        end_line: range_line(&candidate.range, "end"),
        end_column: range_column(&candidate.range, "end"),
    })
}

fn unresolved_callee_node_from_call(
    candidate: &syntax::TreeSitterCandidate,
    target: &str,
) -> GraphNode {
    GraphNode {
        id: target.to_string(),
        display_name: target.to_string(),
        signature: None,
        container: None,
        kind: NodeKind::Function,
        language: candidate.language.clone(),
        file_path: String::new(),
        start_line: 0,
        start_column: 0,
        end_line: 0,
        end_column: 0,
    }
}

fn resolve_tree_callee(
    definitions_by_name: &HashMap<String, Vec<GraphNode>>,
    definitions_by_scoped_name: &HashMap<String, Vec<GraphNode>>,
    call: &syntax::TreeSitterCandidate,
    target: &str,
    target_name: &str,
) -> Option<GraphNode> {
    if call.symbol_kind.as_deref() == Some("constructor") {
        return unique_definition(
            definitions_by_scoped_name.get(&tree_symbol_scoped_lookup_key(
                &call.language,
                &call.root_id,
                target_name,
                "constructor",
            )),
        );
    }

    if is_same_instance_call(target) {
        if let Some(container) = call.container.as_deref() {
            if let Some(node) = unique_definition(definitions_by_scoped_name.get(
                &tree_symbol_scoped_lookup_key(
                    &call.language,
                    &call.root_id,
                    container,
                    target_name,
                ),
            )) {
                return Some(node);
            }
        }
    }

    unique_definition(definitions_by_name.get(&tree_symbol_lookup_key(
        &call.language,
        &call.root_id,
        target_name,
    )))
}

fn tree_symbol_id(candidate: &syntax::TreeSitterCandidate) -> String {
    let name = candidate.name.as_deref().unwrap_or("<anonymous>");
    let container = candidate.container.as_deref().unwrap_or("");
    format!(
        "parser:{}:{}:{}:{}",
        candidate.language, candidate.path, container, name
    )
}

fn tree_symbol_lookup_key(language: &str, root_id: &str, name: &str) -> String {
    format!("{language}:{root_id}:{}", syntax::last_identifier(name))
}

fn tree_symbol_scoped_lookup_key(
    language: &str,
    root_id: &str,
    container: &str,
    name: &str,
) -> String {
    format!(
        "{language}:{root_id}:{}:{}",
        syntax::last_identifier(container),
        syntax::last_identifier(name)
    )
}

fn unique_definition(candidates: Option<&Vec<GraphNode>>) -> Option<GraphNode> {
    candidates.and_then(|candidates| (candidates.len() == 1).then(|| candidates[0].clone()))
}

fn is_same_instance_call(target: &str) -> bool {
    let target = target.trim();
    target.starts_with("this.") || target.starts_with("self.")
}

fn tree_symbol_signature(candidate: &syntax::TreeSitterCandidate, name: &str) -> String {
    let raw = candidate
        .signature
        .as_deref()
        .filter(|signature| !signature.trim().is_empty())
        .unwrap_or(name)
        .trim();
    match candidate.container.as_deref() {
        Some(container) if !raw.contains(container) && candidate.kind == "method" => {
            format!("{container}.{raw}")
        }
        _ => raw.to_string(),
    }
}

fn is_callable_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function" | "method" | "constructor" | "synthetic_method"
    )
}

fn range_line(range: &serde_json::Value, point: &str) -> u32 {
    range
        .pointer(&format!("/{point}/line"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32
}

fn range_column(range: &serde_json::Value, point: &str) -> u32 {
    range
        .pointer(&format!("/{point}/column"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32
}

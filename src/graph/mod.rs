//! Property-graph backend for call/caller queries.
//!
//! This module provides a [`GraphBackend`] trait and a concrete
//! [`GraphStore`] implementation using petgraph.  The trait is
//! designed so that a KuzuDB backend can be swapped in later without
//! changing the public API.
//!
//! ## Architecture
//!
//! ```text
//! GraphStore
//!   ├─ petgraph backend (default)
//!   │   ├─ build()        ── build from SCIP + tree-sitter
//!   │   ├─ query_calls()  ── outgoing call edges
//!   │   ├─ query_callers()── incoming call edges
//!   │   └─ freshness_check()
//!   └─ <future Kuzu backend>
//! ```
//!
//! ## Reliability contract
//!
//! **All** results from `query_calls` and `query_callers` MUST carry
//! `reliability: "inferred_candidate"` — even when the edge was derived
//! from precise SCIP data.  This is because call-graph analysis is
//! inherently incomplete (dynamic dispatch, reflection, macros, …).

pub mod builder;
pub mod schema;

use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use bincode;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use serde_json::{json, Value};

use crate::{
    index, navigation,
    query_input::{matched_variant_value, InputMode, InputPlan, InputVariant, SymbolMatchMode},
    workspace::{ScanOptions, Workspace},
};

use self::schema::{CallCandidate, EdgeMetadata, GraphNode, HierarchyDirection, SerialisedGraph};

// Re-exports
pub use self::schema::EdgeKind;

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

/// Abstraction over graph storage backends (petgraph, KuzuDB, …).
pub trait GraphBackend {
    /// Build the graph from source files and (optionally) SCIP data.
    fn build(&mut self, workspace: &Workspace, graph_dir: &Path) -> Result<()>;

    /// Query outgoing call relationships for a given function identifier.
    fn query_calls(&self, identifier: &str) -> Result<Vec<CallCandidate>>;

    /// Query incoming call relationships for a given function identifier.
    fn query_callers(&self, identifier: &str) -> Result<Vec<CallCandidate>>;

    fn query_calls_with_input(
        &self,
        plan: &InputPlan,
        case_sensitive: bool,
    ) -> Result<Vec<CallCandidate>>;

    fn query_callers_with_input(
        &self,
        plan: &InputPlan,
        case_sensitive: bool,
    ) -> Result<Vec<CallCandidate>>;

    fn query_call_hierarchy(
        &self,
        plan: &InputPlan,
        direction: HierarchyDirection,
        depth: usize,
        limit: usize,
        case_sensitive: bool,
        allowed_paths: Option<&HashSet<String>>,
    ) -> Result<Vec<Value>>;

    /// Check whether the stored graph is fresh relative to the given snapshot.
    fn freshness_check(&self, snapshot_id: &str) -> Result<bool>;
}

// ---------------------------------------------------------------------------
// GraphStore — lifecycle manager
// ---------------------------------------------------------------------------

/// Manages the graph backend lifecycle: build, load, query, freshness.
pub struct GraphStore {
    backend: Box<dyn GraphBackend>,
    graph_dir: PathBuf,
    snapshot_id: String,
}

impl GraphStore {
    /// Create a new store, loading an existing graph if available.
    pub fn open(workspace: &Workspace) -> Result<Self> {
        Self::open_for_snapshot(workspace, &workspace.snapshot_id)
    }

    pub fn open_for_snapshot(workspace: &Workspace, snapshot_id: &str) -> Result<Self> {
        let graph_dir = graph_dir_for_snapshot(workspace, snapshot_id);

        // Try loading the persisted graph; if missing or stale, start fresh.
        let bin_path = graph_dir.join("petgraph.bin");
        let backend: Box<dyn GraphBackend> = if bin_path.exists() {
            match PetgraphBackend::load_from_disk(&bin_path) {
                Ok(backend) => Box::new(backend),
                Err(_) => Box::new(PetgraphBackend::empty()),
            }
        } else {
            Box::new(PetgraphBackend::empty())
        };

        Ok(Self {
            backend,
            graph_dir,
            snapshot_id: snapshot_id.to_string(),
        })
    }

    /// Build (or rebuild) the graph from workspace sources and any
    /// existing SCIP occurrence data.
    pub fn build(&mut self, workspace: &Workspace) -> Result<()> {
        fs::create_dir_all(&self.graph_dir)?;
        self.backend.build(workspace, &self.graph_dir)?;
        self.snapshot_id = workspace.snapshot_id.clone();
        Ok(())
    }

    /// Query outgoing calls from the given function identifier.
    pub fn query_calls(&self, identifier: &str) -> Result<Vec<CallCandidate>> {
        self.backend.query_calls(identifier)
    }

    /// Query incoming callers for the given function identifier.
    pub fn query_callers(&self, identifier: &str) -> Result<Vec<CallCandidate>> {
        self.backend.query_callers(identifier)
    }

    pub fn query_calls_with_input(
        &self,
        plan: &InputPlan,
        case_sensitive: bool,
    ) -> Result<Vec<CallCandidate>> {
        self.backend.query_calls_with_input(plan, case_sensitive)
    }

    pub fn query_callers_with_input(
        &self,
        plan: &InputPlan,
        case_sensitive: bool,
    ) -> Result<Vec<CallCandidate>> {
        self.backend.query_callers_with_input(plan, case_sensitive)
    }

    pub fn query_call_hierarchy(
        &self,
        workspace: &Workspace,
        opts: &ScanOptions,
        identifier: &str,
        direction: HierarchyDirection,
        depth: usize,
    ) -> Result<Vec<Value>> {
        let allowed_paths = if graph_scope_restricts_paths(opts) {
            let mut scope_opts = opts.clone();
            scope_opts.limit = 0;
            Some(
                workspace
                    .scan_catalog(&scope_opts)?
                    .into_iter()
                    .map(|file| file.path)
                    .collect::<HashSet<_>>(),
            )
        } else {
            None
        };
        let plan = InputPlan::new(identifier, opts.input_mode);
        self.backend.query_call_hierarchy(
            &plan,
            direction,
            depth.max(1),
            opts.limit,
            opts.case_sensitive,
            allowed_paths.as_ref(),
        )
    }

    /// Check whether the persisted graph matches the current snapshot.
    pub fn freshness_check(&self) -> Result<bool> {
        self.backend.freshness_check(&self.snapshot_id)
    }

    /// Index metadata for JSON responses.
    pub fn index_meta(&self, fresh: bool) -> Value {
        json!({
            "used": true,
            "fresh": fresh,
            "source": "petgraph",
            "fallback": false,
            "path": self.graph_dir,
            "snapshot_id": self.snapshot_id,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the graph storage directory for the current workspace snapshot.
pub fn graph_dir(workspace: &Workspace) -> PathBuf {
    graph_dir_for_snapshot(workspace, &workspace.snapshot_id)
}

pub fn graph_dir_for_snapshot(workspace: &Workspace, snapshot_id: &str) -> PathBuf {
    let root = workspace.root.join(".codetrail");
    root.join("graph").join(index::snapshot_key(snapshot_id))
}

pub fn graph_index_exists(workspace: &Workspace) -> bool {
    graph_index_exists_for_snapshot(workspace, &workspace.snapshot_id)
}

pub fn graph_index_exists_for_snapshot(workspace: &Workspace, snapshot_id: &str) -> bool {
    graph_dir_for_snapshot(workspace, snapshot_id)
        .join("petgraph.bin")
        .exists()
}

pub fn filter_candidates_by_scan_scope(
    workspace: &Workspace,
    opts: &ScanOptions,
    mut results: Vec<CallCandidate>,
) -> Result<Vec<CallCandidate>> {
    if graph_scope_restricts_paths(opts) {
        let mut scope_opts = opts.clone();
        scope_opts.limit = 0;
        let allowed_paths = workspace
            .scan_catalog(&scope_opts)?
            .into_iter()
            .map(|file| file.path)
            .collect::<HashSet<_>>();
        results.retain(|candidate| allowed_paths.contains(&candidate.path));
    }
    if opts.limit > 0 && results.len() > opts.limit {
        results.truncate(opts.limit);
    }
    Ok(results)
}

fn graph_scope_restricts_paths(opts: &ScanOptions) -> bool {
    opts.changed
        || !opts.dirs.is_empty()
        || !opts.extensions.is_empty()
        || !opts.file_patterns.is_empty()
        || !opts.include.is_empty()
        || !opts.exclude.is_empty()
        || !opts.lang.is_empty()
}

// ---------------------------------------------------------------------------
// Petgraph Backend implementation
// ---------------------------------------------------------------------------

/// Concrete [`GraphBackend`] backed by petgraph's `DiGraph`.
pub struct PetgraphBackend {
    pub(crate) graph: DiGraph<GraphNode, EdgeMetadata>,
    /// Cache: node-index lookup by scoped identifier.
    pub(crate) node_by_id: std::collections::HashMap<String, petgraph::graph::NodeIndex>,
    pub(crate) snapshot_id: String,
    pub(crate) schema_version: u32,
}

impl PetgraphBackend {
    /// Create an empty graph (no nodes, no edges).
    pub fn empty() -> Self {
        Self {
            graph: DiGraph::new(),
            node_by_id: std::collections::HashMap::new(),
            snapshot_id: String::new(),
            schema_version: SerialisedGraph::CURRENT_SCHEMA_VERSION,
        }
    }

    /// Persist the current graph to `petgraph.bin` in `graph_dir`.
    fn save_to_disk(&self, graph_dir: &Path) -> Result<()> {
        let serialised = SerialisedGraph {
            nodes: self.graph.node_weights().cloned().collect(),
            edges: self
                .graph
                .edge_references()
                .map(|e| e.weight().clone())
                .collect(),
            snapshot_id: self.snapshot_id.clone(),
            schema_version: SerialisedGraph::CURRENT_SCHEMA_VERSION,
        };

        let bin_path = graph_dir.join("petgraph.bin");
        let encoded =
            bincode::serialize(&serialised).with_context(|| "failed to serialise graph")?;
        fs::write(&bin_path, &encoded)
            .with_context(|| format!("failed to write {}", bin_path.display()))?;

        // Also write a human-readable manifest
        let manifest_path = graph_dir.join("manifest.json");
        let mut f = fs::File::create(&manifest_path)?;
        serde_json::to_writer_pretty(
            &mut f,
            &json!({
                "source": "petgraph",
                "snapshot_id": self.snapshot_id,
                "nodeCount": self.graph.node_count(),
                "edgeCount": self.graph.edge_count(),
                "schemaVersion": SerialisedGraph::CURRENT_SCHEMA_VERSION,
            }),
        )?;
        writeln!(f)?;

        Ok(())
    }

    /// Load a previously persisted graph from disk.
    pub fn load_from_disk(bin_path: &Path) -> Result<Self> {
        let data =
            fs::read(bin_path).with_context(|| format!("failed to read {}", bin_path.display()))?;
        let serialised: SerialisedGraph =
            bincode::deserialize(&data).with_context(|| "failed to deserialise graph")?;

        let mut graph = DiGraph::new();
        let mut node_by_id = std::collections::HashMap::new();

        for node in &serialised.nodes {
            let idx = graph.add_node(node.clone());
            node_by_id.insert(node.id.clone(), idx);
        }

        for edge in &serialised.edges {
            let caller_idx = node_by_id.get(&edge.caller_id);
            let callee_idx = node_by_id.get(&edge.callee_id);
            if let (Some(&caller), Some(&callee)) = (caller_idx, callee_idx) {
                graph.add_edge(caller, callee, edge.clone());
            }
        }

        Ok(Self {
            graph,
            node_by_id,
            snapshot_id: serialised.snapshot_id,
            schema_version: serialised.schema_version,
        })
    }

    /// Helper: insert a node if not already present.
    pub(crate) fn ensure_node(&mut self, node: GraphNode) -> petgraph::graph::NodeIndex {
        *self
            .node_by_id
            .entry(node.id.clone())
            .or_insert_with(|| self.graph.add_node(node))
    }
}

impl GraphBackend for PetgraphBackend {
    fn build(&mut self, workspace: &Workspace, graph_dir: &Path) -> Result<()> {
        self.snapshot_id = workspace.snapshot_id.clone();
        self.schema_version = SerialisedGraph::CURRENT_SCHEMA_VERSION;
        builder::build_petgraph_backend(self, workspace)?;
        self.save_to_disk(graph_dir)?;
        Ok(())
    }

    fn query_calls(&self, identifier: &str) -> Result<Vec<CallCandidate>> {
        let plan = InputPlan::new(identifier, InputMode::Strict);
        self.query_calls_with_input(&plan, true)
    }

    fn query_callers(&self, identifier: &str) -> Result<Vec<CallCandidate>> {
        let plan = InputPlan::new(identifier, InputMode::Strict);
        self.query_callers_with_input(&plan, true)
    }

    fn query_calls_with_input(
        &self,
        plan: &InputPlan,
        case_sensitive: bool,
    ) -> Result<Vec<CallCandidate>> {
        let mut results: Vec<CallCandidate> = Vec::new();
        for (node_idx, variant) in self.matching_node_indices_for_plan(plan, case_sensitive) {
            results.extend(
                self.graph
                    .edges_directed(node_idx, petgraph::Direction::Outgoing)
                    .map(|edge| {
                        let meta = edge.weight();
                        let caller = &self.graph[edge.source()];
                        let callee = &self.graph[edge.target()];
                        let mut candidate = edge_to_candidate(meta, caller, callee);
                        candidate.matched_input_variant = Some(matched_variant_value(&variant));
                        candidate
                    }),
            );
        }

        finalize_call_candidates(&mut results);
        Ok(results)
    }

    fn query_callers_with_input(
        &self,
        plan: &InputPlan,
        case_sensitive: bool,
    ) -> Result<Vec<CallCandidate>> {
        let mut results: Vec<CallCandidate> = Vec::new();
        for (node_idx, variant) in self.matching_node_indices_for_plan(plan, case_sensitive) {
            results.extend(
                self.graph
                    .edges_directed(node_idx, petgraph::Direction::Incoming)
                    .map(|edge| {
                        let meta = edge.weight();
                        let caller = &self.graph[edge.source()];
                        let callee = &self.graph[edge.target()];
                        let mut candidate = edge_to_caller_candidate(meta, caller, callee);
                        candidate.matched_input_variant = Some(matched_variant_value(&variant));
                        candidate
                    }),
            );
        }

        finalize_call_candidates(&mut results);
        Ok(results)
    }

    fn query_call_hierarchy(
        &self,
        plan: &InputPlan,
        direction: HierarchyDirection,
        depth: usize,
        limit: usize,
        case_sensitive: bool,
        allowed_paths: Option<&HashSet<String>>,
    ) -> Result<Vec<Value>> {
        let mut results = Vec::new();
        let qualified = plan
            .coordinate
            .is_none()
            .then(|| qualified_hierarchy_query(&plan.raw))
            .flatten();
        let mut root_indices = self
            .matching_node_indices_for_plan(plan, case_sensitive)
            .into_iter()
            .filter_map(|(node_idx, _)| {
                let root = &self.graph[node_idx];
                (is_hierarchy_callable(root)
                    && path_allowed(allowed_paths, &root.file_path)
                    && qualified.as_ref().is_none_or(|query| {
                        node_matches_qualified_hierarchy(root, query, case_sensitive)
                    }))
                .then_some(node_idx)
            })
            .collect::<Vec<_>>();
        root_indices.sort_by(|a, b| {
            hierarchy_root_key(&self.graph[*a])
                .cmp(&hierarchy_root_key(&self.graph[*b]))
                .then(
                    hierarchy_root_rank(&self.graph[*a]).cmp(&hierarchy_root_rank(&self.graph[*b])),
                )
        });
        root_indices.dedup_by(|a, b| {
            hierarchy_root_key(&self.graph[*a]) == hierarchy_root_key(&self.graph[*b])
        });

        for node_idx in root_indices {
            let root = &self.graph[node_idx];
            let mut result = json!({
                "root": graph_node_item(root),
                "incomingCalls": [],
                "outgoingCalls": [],
            });
            if direction.include_incoming() {
                result["incomingCalls"] = Value::Array(self.expand_incoming_hierarchy(
                    node_idx,
                    depth,
                    limit,
                    allowed_paths,
                    &mut BTreeSet::new(),
                ));
            }
            if direction.include_outgoing() {
                result["outgoingCalls"] = Value::Array(self.expand_outgoing_hierarchy(
                    node_idx,
                    depth,
                    limit,
                    allowed_paths,
                    &mut BTreeSet::new(),
                ));
            }
            results.push(result);
            if limit > 0 && results.len() >= limit {
                break;
            }
        }
        Ok(results)
    }

    fn freshness_check(&self, snapshot_id: &str) -> Result<bool> {
        Ok(self.snapshot_id == snapshot_id
            && !self.snapshot_id.is_empty()
            && self.schema_version == SerialisedGraph::CURRENT_SCHEMA_VERSION)
    }
}

impl PetgraphBackend {
    fn matching_node_indices_for_plan(
        &self,
        plan: &InputPlan,
        case_sensitive: bool,
    ) -> Vec<(NodeIndex, InputVariant)> {
        let matches = self.matching_node_indices_once(plan, case_sensitive);
        if !matches.is_empty() {
            return matches;
        }
        plan.coordinate_fallback_plan()
            .map(|fallback| self.matching_node_indices_once(&fallback, case_sensitive))
            .unwrap_or(matches)
    }

    fn matching_node_indices_once(
        &self,
        plan: &InputPlan,
        case_sensitive: bool,
    ) -> Vec<(NodeIndex, InputVariant)> {
        let mut matches = Vec::new();
        let mut seen = HashSet::new();
        for idx in self.graph.node_indices() {
            let node = &self.graph[idx];
            let Some(variant) = matched_node_variant(node, plan, case_sensitive) else {
                continue;
            };
            if seen.insert(idx) {
                matches.push((idx, variant));
            }
        }
        matches
    }

    fn expand_incoming_hierarchy(
        &self,
        node_idx: NodeIndex,
        depth: usize,
        limit: usize,
        allowed_paths: Option<&HashSet<String>>,
        seen: &mut BTreeSet<String>,
    ) -> Vec<Value> {
        let symbol_id = self.graph[node_idx].id.clone();
        if depth == 0 || !seen.insert(format!("incoming:{symbol_id}")) {
            return Vec::new();
        }
        let mut calls = Vec::new();
        let mut seen_edges = BTreeSet::new();
        let mut edges = self
            .graph
            .edges_directed(node_idx, petgraph::Direction::Incoming)
            .collect::<Vec<_>>();
        edges.sort_by(|a, b| edge_sort_key(a.weight()).cmp(&edge_sort_key(b.weight())));
        for edge in edges {
            let meta = edge.weight();
            if !path_allowed(allowed_paths, &meta.file_path) {
                continue;
            }
            let caller = &self.graph[edge.source()];
            if !is_hierarchy_callable(caller) {
                continue;
            }
            let edge_key = hierarchy_edge_key(meta, &caller.id);
            if !seen_edges.insert(edge_key) {
                continue;
            }
            let mut value = json!({
                "from": graph_node_item(caller),
                "fromRanges": [edge_range(meta)],
                "dispatchKind": "unknown",
            });
            if depth > 1 {
                value["children"] = Value::Array(self.expand_incoming_hierarchy(
                    edge.source(),
                    depth - 1,
                    limit,
                    allowed_paths,
                    seen,
                ));
            }
            calls.push(value);
            if limit > 0 && calls.len() >= limit {
                break;
            }
        }
        seen.remove(&format!("incoming:{symbol_id}"));
        calls
    }

    fn expand_outgoing_hierarchy(
        &self,
        node_idx: NodeIndex,
        depth: usize,
        limit: usize,
        allowed_paths: Option<&HashSet<String>>,
        seen: &mut BTreeSet<String>,
    ) -> Vec<Value> {
        let symbol_id = self.graph[node_idx].id.clone();
        if depth == 0 || !seen.insert(format!("outgoing:{symbol_id}")) {
            return Vec::new();
        }
        let mut calls = Vec::new();
        let mut seen_edges = BTreeSet::new();
        let mut edges = self
            .graph
            .edges_directed(node_idx, petgraph::Direction::Outgoing)
            .collect::<Vec<_>>();
        edges.sort_by(|a, b| edge_sort_key(a.weight()).cmp(&edge_sort_key(b.weight())));
        for edge in edges {
            let meta = edge.weight();
            if !path_allowed(allowed_paths, &meta.file_path) {
                continue;
            }
            let callee = &self.graph[edge.target()];
            if !is_hierarchy_callable(callee) {
                continue;
            }
            let edge_key = hierarchy_edge_key(meta, &callee.id);
            if !seen_edges.insert(edge_key) {
                continue;
            }
            let mut value = json!({
                "to": graph_node_item(callee),
                "fromRanges": [edge_range(meta)],
                "dispatchKind": "unknown",
            });
            if depth > 1 {
                value["children"] = Value::Array(self.expand_outgoing_hierarchy(
                    edge.target(),
                    depth - 1,
                    limit,
                    allowed_paths,
                    seen,
                ));
            }
            calls.push(value);
            if limit > 0 && calls.len() >= limit {
                break;
            }
        }
        seen.remove(&format!("outgoing:{symbol_id}"));
        calls
    }
}

// ---------------------------------------------------------------------------
// JSON conversion helpers
// ---------------------------------------------------------------------------

fn edge_to_candidate(meta: &EdgeMetadata, caller: &GraphNode, callee: &GraphNode) -> CallCandidate {
    CallCandidate {
        path: meta.file_path.clone(),
        language: meta.language.clone(),
        target: node_display_name(callee),
        enclosing_symbol: Some(node_display_name(caller)),
        range: json!({
            "start": { "line": meta.call_line, "column": meta.call_column },
            "end": { "line": meta.call_line, "column": meta.call_column + 1 }
        }),
        target_definition: Some(graph_node_item(callee)),
        enclosing_definition: Some(graph_node_item(caller)),
        file_hash: meta.file_hash.clone(),
        producer: format!("graph:{}", meta.source),
        source: format!("{}", meta.source),
        level: "inferred_candidate".to_string(), // ALWAYS inferred_candidate per spec
        matched_input_variant: None,
    }
}

fn edge_to_caller_candidate(
    meta: &EdgeMetadata,
    caller: &GraphNode,
    callee: &GraphNode,
) -> CallCandidate {
    CallCandidate {
        path: meta.file_path.clone(),
        language: meta.language.clone(),
        target: node_display_name(callee),
        enclosing_symbol: Some(node_display_name(caller)),
        range: json!({
            "start": { "line": meta.call_line, "column": meta.call_column },
            "end": { "line": meta.call_line, "column": meta.call_column + 1 }
        }),
        target_definition: Some(graph_node_item(callee)),
        enclosing_definition: Some(graph_node_item(caller)),
        file_hash: meta.file_hash.clone(),
        producer: format!("graph:{}", meta.source),
        source: format!("{}", meta.source),
        level: "inferred_candidate".to_string(), // ALWAYS inferred_candidate per spec
        matched_input_variant: None,
    }
}

fn finalize_call_candidates(results: &mut Vec<CallCandidate>) {
    if results.is_empty() {
        return;
    }
    results.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.enclosing_symbol.cmp(&b.enclosing_symbol))
            .then(a.target.cmp(&b.target))
            .then(candidate_start_line(a).cmp(&candidate_start_line(b)))
            .then(candidate_start_column(a).cmp(&candidate_start_column(b)))
    });
    prefer_scip_precise_candidates(results);
    results.dedup_by(|a, b| same_candidate_site(a, b));
}

fn matched_node_variant(
    node: &GraphNode,
    plan: &InputPlan,
    case_sensitive: bool,
) -> Option<InputVariant> {
    if let Some(coord) = &plan.coordinate {
        let names = [
            node.id.as_str(),
            node.display_name.as_str(),
            last_identifier(&node.id),
            last_identifier(&node.display_name),
            method_base_identifier(&node.id),
            method_base_identifier(&node.display_name),
        ];
        if !navigation::coordinate_matches_parts(
            coord,
            Some(&node.file_path),
            Some(u64::from(node.start_line)),
            Some(u64::from(node.end_line)),
            &names,
        ) {
            return None;
        }
    }
    [
        node.id.as_str(),
        node.display_name.as_str(),
        last_identifier(&node.id),
        last_identifier(&node.display_name),
        method_base_identifier(&node.id),
        method_base_identifier(&node.display_name),
    ]
    .into_iter()
    .find_map(|candidate| {
        plan.matched_variant(candidate, case_sensitive, SymbolMatchMode::Exact)
            .cloned()
    })
}

#[derive(Clone, Debug)]
struct QualifiedHierarchyQuery {
    qualifier: String,
    name: String,
}

fn qualified_hierarchy_query(input: &str) -> Option<QualifiedHierarchyQuery> {
    let head = input
        .trim()
        .split_once('(')
        .map(|(head, _)| head)
        .unwrap_or_else(|| input.trim());
    let (qualifier, name) = head
        .rsplit_once("::")
        .or_else(|| head.rsplit_once('.'))
        .or_else(|| head.rsplit_once('#'))
        .or_else(|| head.rsplit_once('$'))?;
    let qualifier = last_identifier(qualifier).trim();
    let name = last_identifier(name).trim();
    (!qualifier.is_empty() && !name.is_empty()).then(|| QualifiedHierarchyQuery {
        qualifier: qualifier.to_string(),
        name: name.to_string(),
    })
}

fn node_matches_qualified_hierarchy(
    node: &GraphNode,
    query: &QualifiedHierarchyQuery,
    case_sensitive: bool,
) -> bool {
    let display_name = node_display_name(node);
    let node_name = method_base_identifier(&display_name);
    if !text_eq(node_name, &query.name, case_sensitive) {
        return false;
    }
    node.container.as_deref().is_some_and(|container| {
        text_eq(last_identifier(container), &query.qualifier, case_sensitive)
    }) || text_eq(
        &canonical_qualified_node_name(node),
        &format!("{}.{}", query.qualifier, query.name),
        case_sensitive,
    )
}

fn canonical_qualified_node_name(node: &GraphNode) -> String {
    if let Some(container) = node.container.as_deref() {
        return format!(
            "{}.{}",
            last_identifier(container),
            method_base_identifier(&node_display_name(node))
        );
    }
    node.id
        .replace("::", ".")
        .replace('#', ".")
        .replace('$', ".")
        .replace(':', ".")
}

fn text_eq(left: &str, right: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        left == right
    } else {
        left.eq_ignore_ascii_case(right)
    }
}

fn node_display_name(node: &GraphNode) -> String {
    if node.display_name.is_empty() {
        node.id.clone()
    } else {
        node.display_name.clone()
    }
}

fn graph_node_item(node: &GraphNode) -> Value {
    let signature = node
        .signature
        .clone()
        .filter(|signature| !signature.is_empty())
        .unwrap_or_else(|| node_display_name(node));
    json!({
        "symbol_id": node.id,
        "name": node_display_name(node),
        "kind": "function",
        "path": node.file_path,
        "range": node_range(node),
        "selectionRange": node_range(node),
        "signature": signature,
        "language": node.language,
        "container": node.container,
    })
}

fn node_range(node: &GraphNode) -> Value {
    json!({
        "start": { "line": node.start_line, "character": node.start_column },
        "end": { "line": node.end_line, "character": node.end_column }
    })
}

fn edge_range(meta: &EdgeMetadata) -> Value {
    json!({
        "start": { "line": meta.call_line, "character": meta.call_column },
        "end": { "line": meta.call_line, "character": meta.call_column + 1 }
    })
}

fn is_hierarchy_callable(node: &GraphNode) -> bool {
    node.kind == self::schema::NodeKind::Function
        && !node.file_path.is_empty()
        && node.start_line > 0
}

fn path_allowed(allowed_paths: Option<&HashSet<String>>, path: &str) -> bool {
    allowed_paths
        .map(|paths| paths.contains(path))
        .unwrap_or(true)
}

fn edge_sort_key(meta: &EdgeMetadata) -> (&str, u32, u32, &str) {
    (
        meta.file_path.as_str(),
        meta.call_line,
        meta.call_column,
        meta.callee_id.as_str(),
    )
}

fn hierarchy_edge_key(meta: &EdgeMetadata, item_id: &str) -> (String, u32, u32, String) {
    (
        meta.file_path.clone(),
        meta.call_line,
        meta.call_column,
        item_id.to_string(),
    )
}

fn hierarchy_root_key(node: &GraphNode) -> (String, String, String, String) {
    (
        node.language.clone(),
        node.file_path.clone(),
        node.container.clone().unwrap_or_default(),
        node_display_name(node),
    )
}

fn hierarchy_root_rank(node: &GraphNode) -> u8 {
    let has_signature = node
        .signature
        .as_deref()
        .is_some_and(|signature| signature.contains('('));
    match (node.id.starts_with("parser:"), has_signature) {
        (true, true) => 0,
        (false, true) => 1,
        (true, false) => 2,
        (false, false) => 3,
    }
}

fn candidate_start_line(candidate: &CallCandidate) -> u64 {
    candidate.range["start"]["line"].as_u64().unwrap_or(0)
}

fn candidate_start_column(candidate: &CallCandidate) -> u64 {
    candidate.range["start"]["column"].as_u64().unwrap_or(0)
}

fn same_candidate_site(a: &CallCandidate, b: &CallCandidate) -> bool {
    a.path == b.path
        && a.enclosing_symbol == b.enclosing_symbol
        && a.target == b.target
        && a.source == b.source
        && candidate_start_line(a) == candidate_start_line(b)
        && candidate_start_column(a) == candidate_start_column(b)
}

fn prefer_scip_precise_candidates(results: &mut Vec<CallCandidate>) {
    let precise_sites: HashSet<_> = results
        .iter()
        .filter(|candidate| candidate.source == "scip_precise")
        .map(candidate_normalized_site)
        .collect();
    results.retain(|candidate| {
        candidate.source == "scip_precise"
            || !precise_sites.contains(&candidate_normalized_site(candidate))
    });
}

fn candidate_normalized_site(candidate: &CallCandidate) -> CandidateSite {
    CandidateSite {
        path: candidate.path.clone(),
        enclosing_symbol: candidate
            .enclosing_symbol
            .as_deref()
            .map(method_base_identifier)
            .map(ToString::to_string),
        target: method_base_identifier(&candidate.target).to_string(),
        line: candidate_start_line(candidate),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CandidateSite {
    path: String,
    enclosing_symbol: Option<String>,
    target: String,
    line: u64,
}

fn last_identifier(target: &str) -> &str {
    target
        .rsplit(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .find(|part| !part.is_empty())
        .unwrap_or(target)
}

fn method_base_identifier(target: &str) -> &str {
    let before_signature = target
        .split_once('(')
        .map(|(head, _)| head)
        .unwrap_or(target);
    last_identifier(before_signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::schema::{EdgeSource, GraphNode, NodeKind, ReliabilityLevel};
    use serde_json::json;
    use tempfile::tempdir;

    fn make_test_node(id: &str, kind: NodeKind) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            display_name: id.to_string(),
            signature: Some(id.to_string()),
            container: None,
            kind,
            language: "rust".to_string(),
            file_path: format!("src/{}.rs", id),
            start_line: 1,
            start_column: 0,
            end_line: 10,
            end_column: 0,
        }
    }

    fn make_test_edge(
        caller_id: &str,
        callee_id: &str,
        file_path: &str,
        source: EdgeSource,
    ) -> EdgeMetadata {
        EdgeMetadata {
            source,
            level: ReliabilityLevel::InferredCandidate,
            file_path: file_path.to_string(),
            call_line: 5,
            call_column: 10,
            caller_id: caller_id.to_string(),
            callee_id: callee_id.to_string(),
            language: "rust".to_string(),
            file_hash: "blake3:deadbeef".to_string(),
        }
    }

    #[test]
    fn graph_build_and_query_calls() {
        let mut backend = PetgraphBackend::empty();
        backend.snapshot_id = "test-snap".to_string();

        // Add nodes
        let caller = make_test_node("foo", NodeKind::Function);
        let callee = make_test_node("bar", NodeKind::Function);
        backend.ensure_node(caller.clone());
        backend.ensure_node(callee.clone());

        // Add edge foo -> bar
        let edge = make_test_edge("foo", "bar", "src/main.rs", EdgeSource::TreeSitterHeuristic);
        let caller_idx = *backend.node_by_id.get("foo").unwrap();
        let callee_idx = *backend.node_by_id.get("bar").unwrap();
        backend.graph.add_edge(caller_idx, callee_idx, edge);

        // Query calls from foo
        let calls = backend.query_calls("foo").unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].target, "bar");
        assert_eq!(calls[0].level, "inferred_candidate");
        assert_eq!(calls[0].source, "tree_sitter_heuristic");
        assert_eq!(calls[0].enclosing_symbol, Some("foo".to_string()));
        assert_eq!(calls[0].path, "src/main.rs");

        // Query callers of bar
        let callers = backend.query_callers("bar").unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].target, "bar");
        assert_eq!(callers[0].level, "inferred_candidate");
        assert_eq!(callers[0].enclosing_symbol, Some("foo".to_string()));
    }

    #[test]
    fn graph_preserves_multiple_call_sites_between_same_nodes() {
        let mut backend = PetgraphBackend::empty();
        let caller = make_test_node("caller", NodeKind::Function);
        let callee = make_test_node("callee", NodeKind::Function);
        backend.ensure_node(caller);
        backend.ensure_node(callee);

        let caller_idx = *backend.node_by_id.get("caller").unwrap();
        let callee_idx = *backend.node_by_id.get("callee").unwrap();
        let first = make_test_edge("caller", "callee", "src/lib.rs", EdgeSource::ScipPrecise);
        let mut second = make_test_edge("caller", "callee", "src/lib.rs", EdgeSource::ScipPrecise);
        second.call_line = 8;
        second.call_column = 4;
        backend.graph.add_edge(caller_idx, callee_idx, first);
        backend.graph.add_edge(caller_idx, callee_idx, second);

        let callers = backend.query_callers("callee").unwrap();
        assert_eq!(callers.len(), 2);
        assert_eq!(callers[0].range["start"]["line"], 5);
        assert_eq!(callers[1].range["start"]["line"], 8);
    }

    #[test]
    fn graph_prefers_scip_over_tree_sitter_at_same_call_site() {
        let mut backend = PetgraphBackend::empty();
        let caller = make_test_node("run", NodeKind::Function);
        let callee = make_test_node("helper", NodeKind::Function);
        let mut precise_caller = make_test_node("run(String)", NodeKind::Function);
        precise_caller.display_name = "run(String)".to_string();
        let mut precise_callee = make_test_node("helper(Long)", NodeKind::Function);
        precise_callee.display_name = "helper(Long)".to_string();
        backend.ensure_node(caller);
        backend.ensure_node(callee);
        backend.ensure_node(precise_caller);
        backend.ensure_node(precise_callee);

        let tree_caller_idx = *backend.node_by_id.get("run").unwrap();
        let tree_callee_idx = *backend.node_by_id.get("helper").unwrap();
        let precise_caller_idx = *backend.node_by_id.get("run(String)").unwrap();
        let precise_callee_idx = *backend.node_by_id.get("helper(Long)").unwrap();
        backend.graph.add_edge(
            tree_caller_idx,
            tree_callee_idx,
            make_test_edge(
                "run",
                "helper",
                "src/lib.rs",
                EdgeSource::TreeSitterHeuristic,
            ),
        );
        backend.graph.add_edge(
            precise_caller_idx,
            precise_callee_idx,
            make_test_edge(
                "run(String)",
                "helper(Long)",
                "src/lib.rs",
                EdgeSource::ScipPrecise,
            ),
        );

        let callers = backend.query_callers("helper").unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].source, "scip_precise");
        assert_eq!(callers[0].target, "helper(Long)");
    }

    #[test]
    fn graph_returns_same_symbol_call_edges() {
        let mut backend = PetgraphBackend::empty();
        let node = make_test_node("selectJobById(Long)", NodeKind::Function);
        backend.ensure_node(node);

        let node_idx = *backend.node_by_id.get("selectJobById(Long)").unwrap();
        let edge = make_test_edge(
            "selectJobById(Long)",
            "selectJobById(Long)",
            "src/SysJobServiceImpl.java",
            EdgeSource::ScipPrecise,
        );
        backend.graph.add_edge(node_idx, node_idx, edge);

        let callers = backend.query_callers("selectJobById(Long)").unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(
            callers[0].enclosing_symbol,
            Some("selectJobById(Long)".to_string())
        );
        assert_eq!(callers[0].source, "scip_precise");

        let bare_callers = backend.query_callers("selectJobById").unwrap();
        assert_eq!(bare_callers.len(), 1);
        assert_eq!(bare_callers[0].source, "scip_precise");
    }

    #[test]
    fn graph_keeps_unique_ids_for_duplicate_display_names() {
        let mut backend = PetgraphBackend::empty();
        let mut caller = make_test_node("scip:crate/a#parse", NodeKind::Function);
        caller.display_name = "parse".to_string();
        let mut callee = make_test_node("scip:crate/b#parse", NodeKind::Function);
        callee.display_name = "parse".to_string();
        backend.ensure_node(caller);
        backend.ensure_node(callee);

        let edge = make_test_edge(
            "scip:crate/a#parse",
            "scip:crate/b#parse",
            "src/lib.rs",
            EdgeSource::ScipPrecise,
        );
        let caller_idx = *backend.node_by_id.get("scip:crate/a#parse").unwrap();
        let callee_idx = *backend.node_by_id.get("scip:crate/b#parse").unwrap();
        backend.graph.add_edge(caller_idx, callee_idx, edge);

        assert_eq!(backend.graph.node_count(), 2);
        let calls = backend.query_calls("scip:crate/a#parse").unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].target, "parse");

        let display_calls = backend.query_calls("parse").unwrap();
        assert_eq!(display_calls.len(), 1);
        assert_eq!(display_calls[0].target, "parse");
        assert_eq!(display_calls[0].enclosing_symbol, Some("parse".to_string()));
    }

    #[test]
    fn graph_matches_simple_identifier_against_qualified_call_targets() {
        let mut backend = PetgraphBackend::empty();
        backend.snapshot_id = "test-snap".to_string();

        let caller = make_test_node("run", NodeKind::Function);
        let mut callee = make_test_node("self.helper", NodeKind::Function);
        callee.display_name = "self.helper".to_string();
        backend.ensure_node(caller);
        backend.ensure_node(callee);

        let edge = make_test_edge(
            "run",
            "self.helper",
            "src/lib.rs",
            EdgeSource::TreeSitterHeuristic,
        );
        let caller_idx = *backend.node_by_id.get("run").unwrap();
        let callee_idx = *backend.node_by_id.get("self.helper").unwrap();
        backend.graph.add_edge(caller_idx, callee_idx, edge);

        let callers = backend.query_callers("helper").unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].target, "self.helper");
        assert_eq!(callers[0].enclosing_symbol, Some("run".to_string()));
    }

    #[test]
    fn query_unknown_identifier_returns_empty() {
        let backend = PetgraphBackend::empty();
        assert!(backend.query_calls("nonexistent").unwrap().is_empty());
        assert!(backend.query_callers("nonexistent").unwrap().is_empty());
    }

    #[test]
    fn freshness_check_matches_snapshot() {
        let backend = PetgraphBackend::empty();
        assert!(!backend.freshness_check("test").unwrap());

        let mut backend = PetgraphBackend::empty();
        backend.snapshot_id = "commit:abc123".to_string();
        assert!(backend.freshness_check("commit:abc123").unwrap());
        assert!(!backend.freshness_check("commit:def456").unwrap());
    }

    #[test]
    fn freshness_rejects_stored_graph_with_old_schema_version() {
        let dir = tempdir().unwrap();
        let graph = SerialisedGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
            snapshot_id: "commit:abc123".to_string(),
            schema_version: SerialisedGraph::CURRENT_SCHEMA_VERSION.saturating_sub(1),
        };
        let bin_path = dir.path().join("petgraph.bin");
        std::fs::write(&bin_path, bincode::serialize(&graph).unwrap()).unwrap();

        let backend = PetgraphBackend::load_from_disk(&bin_path).unwrap();

        assert!(!backend.freshness_check("commit:abc123").unwrap());
    }

    #[test]
    fn serialisation_roundtrip() {
        let dir = tempdir().unwrap();
        let graph_dir = dir.path();

        let mut backend = PetgraphBackend::empty();
        backend.snapshot_id = "snapshot-1".to_string();

        let caller = make_test_node("alpha", NodeKind::Function);
        let callee = make_test_node("beta", NodeKind::Function);
        backend.ensure_node(caller);
        backend.ensure_node(callee);

        let edge = make_test_edge("alpha", "beta", "src/lib.rs", EdgeSource::ScipPrecise);
        let a_idx = *backend.node_by_id.get("alpha").unwrap();
        let b_idx = *backend.node_by_id.get("beta").unwrap();
        backend.graph.add_edge(a_idx, b_idx, edge);

        // Save
        backend.save_to_disk(graph_dir).unwrap();
        assert!(graph_dir.join("petgraph.bin").exists());
        assert!(graph_dir.join("manifest.json").exists());

        // Load
        let loaded = PetgraphBackend::load_from_disk(&graph_dir.join("petgraph.bin")).unwrap();
        assert_eq!(loaded.graph.node_count(), 2);
        assert_eq!(loaded.graph.edge_count(), 1);
        assert_eq!(loaded.snapshot_id, "snapshot-1");

        // Queries work on loaded graph
        let calls = loaded.query_calls("alpha").unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].target, "beta");
        assert_eq!(calls[0].source, "scip_precise");
    }

    #[test]
    fn graph_index_meta_includes_freshness() {
        let backend = PetgraphBackend::empty();
        let store = GraphStore {
            backend: Box::new(backend),
            graph_dir: "/tmp/test".into(),
            snapshot_id: "snap1".to_string(),
        };

        let meta = store.index_meta(true);
        assert_eq!(meta["used"], json!(true));
        assert_eq!(meta["fresh"], json!(true));
        assert_eq!(meta["source"], "petgraph");
        assert_eq!(meta["snapshot_id"], "snap1");

        let stale = store.index_meta(false);
        assert_eq!(stale["fresh"], json!(false));
    }

    #[test]
    fn all_results_are_inferred_candidate() {
        // Verify that even edges from SCIP produce inferred_candidate
        let mut backend = PetgraphBackend::empty();
        let caller = make_test_node("caller", NodeKind::Function);
        let callee = make_test_node("callee", NodeKind::Function);
        backend.ensure_node(caller);
        backend.ensure_node(callee);

        let edge = make_test_edge("caller", "callee", "f.rs", EdgeSource::ScipPrecise);
        let c1 = *backend.node_by_id.get("caller").unwrap();
        let c2 = *backend.node_by_id.get("callee").unwrap();
        backend.graph.add_edge(c1, c2, edge);

        let calls = backend.query_calls("caller").unwrap();
        assert_eq!(calls.len(), 1);
        // CRITICAL: Even SCIP edges must produce inferred_candidate
        assert_eq!(calls[0].level, "inferred_candidate");
        assert_eq!(calls[0].source, "scip_precise");

        let callers = backend.query_callers("callee").unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].level, "inferred_candidate");
    }
}

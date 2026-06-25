use std::collections::{BTreeMap, BTreeSet};

use clap::ValueEnum;
use serde_json::{json, Value};

use crate::java_semantic::model::{JavaCallEdge, JavaSemanticData, JavaSymbol};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum CallHierarchyDirection {
    Incoming,
    Outgoing,
    Both,
}

impl CallHierarchyDirection {
    pub const fn include_incoming(self) -> bool {
        matches!(self, Self::Incoming | Self::Both)
    }

    pub const fn include_outgoing(self) -> bool {
        matches!(self, Self::Outgoing | Self::Both)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Incoming => "incoming",
            Self::Outgoing => "outgoing",
            Self::Both => "both",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CallHierarchyOptions {
    pub direction: CallHierarchyDirection,
    pub depth: usize,
    pub include_overrides: bool,
}

impl Default for CallHierarchyOptions {
    fn default() -> Self {
        Self {
            direction: CallHierarchyDirection::Both,
            depth: 1,
            include_overrides: false,
        }
    }
}

pub fn hierarchy_for_roots(
    data: &JavaSemanticData,
    roots: &[JavaSymbol],
    options: CallHierarchyOptions,
    limit: usize,
) -> Vec<Value> {
    let index = HierarchyIndex::new(data);
    let mut results = Vec::new();
    for root in roots {
        let mut result = json!({
            "root": item(root),
            "incomingCalls": [],
            "outgoingCalls": [],
        });
        if options.direction.include_incoming() {
            result["incomingCalls"] = Value::Array(expand_incoming(
                &index,
                &root.symbol_id,
                options.depth.max(1),
                limit,
                options.include_overrides,
                &mut BTreeSet::new(),
            ));
        }
        if options.direction.include_outgoing() {
            result["outgoingCalls"] = Value::Array(expand_outgoing(
                &index,
                &root.symbol_id,
                options.depth.max(1),
                limit,
                options.include_overrides,
                &mut BTreeSet::new(),
            ));
        }
        results.push(result);
        if limit > 0 && results.len() >= limit {
            break;
        }
    }
    results
}

struct HierarchyIndex<'a> {
    symbols: BTreeMap<&'a str, &'a JavaSymbol>,
    incoming_declared: BTreeMap<&'a str, Vec<&'a JavaCallEdge>>,
    incoming_possible: BTreeMap<&'a str, Vec<&'a JavaCallEdge>>,
    outgoing: BTreeMap<&'a str, Vec<&'a JavaCallEdge>>,
}

impl<'a> HierarchyIndex<'a> {
    fn new(data: &'a JavaSemanticData) -> Self {
        let symbols = data
            .symbols
            .iter()
            .map(|symbol| (symbol.symbol_id.as_str(), symbol))
            .collect::<BTreeMap<_, _>>();
        let mut incoming_declared = BTreeMap::<&str, Vec<&JavaCallEdge>>::new();
        let mut incoming_possible = BTreeMap::<&str, Vec<&JavaCallEdge>>::new();
        let mut outgoing = BTreeMap::<&str, Vec<&JavaCallEdge>>::new();
        for edge in &data.call_edges {
            outgoing
                .entry(edge.caller_symbol.as_str())
                .or_default()
                .push(edge);
            if let Some(callee) = edge.callee_symbol.as_deref() {
                push_unique_edge(incoming_declared.entry(callee).or_default(), edge);
                push_unique_edge(incoming_possible.entry(callee).or_default(), edge);
            }
            for possible in &edge.possible_callees {
                push_unique_edge(
                    incoming_possible.entry(possible.as_str()).or_default(),
                    edge,
                );
            }
        }
        Self {
            symbols,
            incoming_declared,
            incoming_possible,
            outgoing,
        }
    }

    fn symbol(&self, symbol_id: &str) -> Option<&'a JavaSymbol> {
        self.symbols.get(symbol_id).copied()
    }

    fn incoming(&self, symbol_id: &str, include_possible: bool) -> &[&'a JavaCallEdge] {
        let source = if include_possible {
            &self.incoming_possible
        } else {
            &self.incoming_declared
        };
        source.get(symbol_id).map(Vec::as_slice).unwrap_or(&[])
    }

    fn outgoing(&self, symbol_id: &str) -> &[&'a JavaCallEdge] {
        self.outgoing
            .get(symbol_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

fn expand_incoming(
    index: &HierarchyIndex<'_>,
    symbol_id: &str,
    depth: usize,
    limit: usize,
    include_overrides: bool,
    seen: &mut BTreeSet<String>,
) -> Vec<Value> {
    if depth == 0 || !seen.insert(format!("incoming:{symbol_id}")) {
        return Vec::new();
    }
    let mut calls = index
        .incoming(symbol_id, include_overrides)
        .iter()
        .copied()
        .filter_map(|edge| {
            let caller = index.symbol(&edge.caller_symbol)?;
            let mut value = json!({
                "from": item(caller),
                "fromRanges": [edge.range.to_lsp_json()],
                "dispatchKind": format!("{:?}", edge.dispatch_kind).to_lowercase(),
            });
            if depth > 1 {
                value["children"] = Value::Array(expand_incoming(
                    index,
                    &caller.symbol_id,
                    depth - 1,
                    limit,
                    include_overrides,
                    seen,
                ));
            }
            Some(value)
        })
        .collect::<Vec<_>>();
    truncate(&mut calls, limit);
    seen.remove(&format!("incoming:{symbol_id}"));
    calls
}

fn expand_outgoing(
    index: &HierarchyIndex<'_>,
    symbol_id: &str,
    depth: usize,
    limit: usize,
    include_overrides: bool,
    seen: &mut BTreeSet<String>,
) -> Vec<Value> {
    if depth == 0 || !seen.insert(format!("outgoing:{symbol_id}")) {
        return Vec::new();
    }
    let mut calls = index
        .outgoing(symbol_id)
        .iter()
        .copied()
        .filter_map(|edge| {
            let targets = edge_targets(edge, include_overrides);
            let mut items = Vec::new();
            for target in targets {
                let Some(callee) = index.symbol(&target) else {
                    continue;
                };
                let mut value = json!({
                    "to": item(callee),
                    "fromRanges": [edge.range.to_lsp_json()],
                    "dispatchKind": format!("{:?}", edge.dispatch_kind).to_lowercase(),
                });
                if depth > 1 {
                    value["children"] = Value::Array(expand_outgoing(
                        index,
                        &callee.symbol_id,
                        depth - 1,
                        limit,
                        include_overrides,
                        seen,
                    ));
                }
                items.push(value);
            }
            Some(items)
        })
        .flatten()
        .collect::<Vec<_>>();
    truncate(&mut calls, limit);
    seen.remove(&format!("outgoing:{symbol_id}"));
    calls
}

fn edge_targets(edge: &JavaCallEdge, include_overrides: bool) -> Vec<String> {
    let mut targets = edge
        .callee_symbol
        .clone()
        .into_iter()
        .collect::<Vec<String>>();
    if include_overrides {
        targets.extend(edge.possible_callees.iter().cloned());
    }
    targets.sort();
    targets.dedup();
    targets
}

fn push_unique_edge<'a>(edges: &mut Vec<&'a JavaCallEdge>, edge: &'a JavaCallEdge) {
    let key = edge_key(edge);
    if edges.iter().any(|existing| edge_key(existing) == key) {
        return;
    }
    edges.push(edge);
}

fn edge_key(edge: &JavaCallEdge) -> (&str, &str, u32, u32, Option<&str>) {
    (
        edge.caller_symbol.as_str(),
        edge.path.as_str(),
        edge.range.start_line,
        edge.range.start_column,
        edge.callee_symbol.as_deref(),
    )
}

pub fn item(symbol: &JavaSymbol) -> Value {
    json!({
        "symbol_id": symbol.symbol_id,
        "name": symbol.name,
        "signature": symbol.display_signature(),
        "kind": symbol.kind.public_kind(),
        "path": symbol.path,
        "range": symbol.range.as_ref().map(|range| range.to_lsp_json()),
        "selectionRange": symbol.selection_range.as_ref().map(|range| range.to_lsp_json()),
        "detail": symbol.qualified_name,
    })
}

fn truncate(values: &mut Vec<Value>, limit: usize) {
    if limit > 0 && values.len() > limit {
        values.truncate(limit);
    }
}

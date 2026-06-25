use std::collections::BTreeSet;

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
    let mut results = Vec::new();
    for root in roots {
        let mut result = json!({
            "root": item(root),
            "incomingCalls": [],
            "outgoingCalls": [],
        });
        if options.direction.include_incoming() {
            result["incomingCalls"] = Value::Array(expand_incoming(
                data,
                &root.symbol_id,
                options.depth.max(1),
                limit,
                &mut BTreeSet::new(),
            ));
        }
        if options.direction.include_outgoing() {
            result["outgoingCalls"] = Value::Array(expand_outgoing(
                data,
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

fn expand_incoming(
    data: &JavaSemanticData,
    symbol_id: &str,
    depth: usize,
    limit: usize,
    seen: &mut BTreeSet<String>,
) -> Vec<Value> {
    if depth == 0 || !seen.insert(format!("incoming:{symbol_id}")) {
        return Vec::new();
    }
    let mut calls = data
        .call_edges
        .iter()
        .filter(|edge| edge_targets_symbol(edge, symbol_id, false))
        .filter_map(|edge| {
            let caller = symbol(data, &edge.caller_symbol)?;
            let mut value = json!({
                "from": item(caller),
                "fromRanges": [edge.range.to_lsp_json()],
                "dispatchKind": format!("{:?}", edge.dispatch_kind).to_lowercase(),
            });
            if depth > 1 {
                value["children"] = Value::Array(expand_incoming(
                    data,
                    &caller.symbol_id,
                    depth - 1,
                    limit,
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
    data: &JavaSemanticData,
    symbol_id: &str,
    depth: usize,
    limit: usize,
    include_overrides: bool,
    seen: &mut BTreeSet<String>,
) -> Vec<Value> {
    if depth == 0 || !seen.insert(format!("outgoing:{symbol_id}")) {
        return Vec::new();
    }
    let mut calls = data
        .call_edges
        .iter()
        .filter(|edge| edge.caller_symbol == symbol_id)
        .filter_map(|edge| {
            let targets = edge_targets(edge, include_overrides);
            let mut items = Vec::new();
            for target in targets {
                let Some(callee) = symbol(data, &target) else {
                    continue;
                };
                let mut value = json!({
                    "to": item(callee),
                    "fromRanges": [edge.range.to_lsp_json()],
                    "dispatchKind": format!("{:?}", edge.dispatch_kind).to_lowercase(),
                });
                if depth > 1 {
                    value["children"] = Value::Array(expand_outgoing(
                        data,
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

fn edge_targets_symbol(edge: &JavaCallEdge, symbol_id: &str, include_possible: bool) -> bool {
    edge.callee_symbol.as_deref() == Some(symbol_id)
        || (include_possible
            && edge
                .possible_callees
                .iter()
                .any(|candidate| candidate == symbol_id))
}

fn edge_targets(edge: &JavaCallEdge, include_overrides: bool) -> Vec<String> {
    if include_overrides && !edge.possible_callees.is_empty() {
        return edge.possible_callees.clone();
    }
    edge.callee_symbol
        .clone()
        .into_iter()
        .collect::<Vec<String>>()
}

fn symbol<'a>(data: &'a JavaSemanticData, symbol_id: &str) -> Option<&'a JavaSymbol> {
    data.symbols
        .iter()
        .find(|symbol| symbol.symbol_id == symbol_id)
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

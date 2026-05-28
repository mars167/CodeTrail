use std::{fs, path::Path};

use anyhow::Result;
use serde_json::{json, Value};
use tree_sitter::{Language, Node, Parser};

use crate::{
    search::line_range_for_node,
    workspace::{language_for_path, ScanOptions, Workspace},
};

#[derive(Clone, Debug)]
struct Symbol {
    path: String,
    language: String,
    name: String,
    kind: String,
    range: Value,
    body_range: Value,
    producer: String,
    warning: Option<String>,
}

#[derive(Clone, Debug)]
struct CallCandidate {
    path: String,
    language: String,
    target: String,
    enclosing_symbol: Option<String>,
    range: Value,
    producer: String,
}

pub fn symbols(
    workspace: &Workspace,
    opts: &ScanOptions,
    query: &str,
) -> Result<(Value, Vec<String>)> {
    let mut results = Vec::new();
    let mut warnings = Vec::new();
    for symbol in collect_symbols(workspace, opts, &mut warnings)? {
        if symbol.name.contains(query) {
            results.push(symbol_to_json(symbol));
        }
        if opts.limit > 0 && results.len() >= opts.limit {
            break;
        }
    }
    Ok((Value::Array(results), warnings))
}

pub fn defs(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<(Value, Vec<String>)> {
    let mut results = Vec::new();
    let mut warnings = Vec::new();
    for symbol in collect_symbols(workspace, opts, &mut warnings)? {
        if symbol.name == identifier {
            results.push(symbol_to_json(symbol));
        }
        if opts.limit > 0 && results.len() >= opts.limit {
            break;
        }
    }
    Ok((Value::Array(results), warnings))
}

pub fn calls(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<(Value, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut results = Vec::new();
    for call in collect_calls(workspace, opts, &mut warnings)? {
        if call.enclosing_symbol.as_deref() == Some(identifier) {
            results.push(call_to_json(call));
        }
        if opts.limit > 0 && results.len() >= opts.limit {
            break;
        }
    }
    Ok((Value::Array(results), warnings))
}

pub fn callers(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<(Value, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut results = Vec::new();
    for call in collect_calls(workspace, opts, &mut warnings)? {
        if last_identifier(&call.target) == identifier {
            results.push(call_to_json(call));
        }
        if opts.limit > 0 && results.len() >= opts.limit {
            break;
        }
    }
    Ok((Value::Array(results), warnings))
}

fn collect_symbols(
    workspace: &Workspace,
    opts: &ScanOptions,
    warnings: &mut Vec<String>,
) -> Result<Vec<Symbol>> {
    let mut scan_opts = opts.clone();
    scan_opts.limit = 0;
    let mut symbols = Vec::new();
    for file in workspace.scan_files(&scan_opts)? {
        let path = workspace.abs_path(&file.path);
        let Some(language) = parser_language(&path) else {
            continue;
        };
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let mut parser = Parser::new();
        parser.set_language(&language)?;
        let Some(tree) = parser.parse(&content, None) else {
            warnings.push(format!("parser failed for {}", file.path));
            continue;
        };
        if tree.root_node().has_error() {
            warnings.push(format!("partial parse with syntax errors: {}", file.path));
        }
        walk_symbols(
            tree.root_node(),
            &file.path,
            &file.language,
            content.as_bytes(),
            &mut symbols,
        );
    }
    symbols.sort_by(|a, b| a.path.cmp(&b.path).then(a.name.cmp(&b.name)));
    Ok(symbols)
}

fn collect_calls(
    workspace: &Workspace,
    opts: &ScanOptions,
    warnings: &mut Vec<String>,
) -> Result<Vec<CallCandidate>> {
    let mut scan_opts = opts.clone();
    scan_opts.limit = 0;
    let mut calls = Vec::new();
    for file in workspace.scan_files(&scan_opts)? {
        let path = workspace.abs_path(&file.path);
        let Some(language) = parser_language(&path) else {
            continue;
        };
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let mut parser = Parser::new();
        parser.set_language(&language)?;
        let Some(tree) = parser.parse(&content, None) else {
            warnings.push(format!("parser failed for {}", file.path));
            continue;
        };
        if tree.root_node().has_error() {
            warnings.push(format!("partial parse with syntax errors: {}", file.path));
        }
        walk_calls(
            tree.root_node(),
            &file.path,
            &file.language,
            content.as_bytes(),
            &mut calls,
        );
    }
    calls.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.target.cmp(&b.target))
            .then(a.enclosing_symbol.cmp(&b.enclosing_symbol))
    });
    Ok(calls)
}

fn walk_symbols(node: Node, path: &str, language: &str, source: &[u8], symbols: &mut Vec<Symbol>) {
    if let Some(kind) = symbol_kind(node.kind()) {
        if let Some(name_node) = node
            .child_by_field_name("name")
            .or_else(|| first_named_child(node))
        {
            if let Ok(name) = name_node.utf8_text(source) {
                let body_node = node.child_by_field_name("body").unwrap_or(node);
                symbols.push(Symbol {
                    path: path.to_string(),
                    language: language.to_string(),
                    name: name.to_string(),
                    kind: kind.to_string(),
                    range: point_range(node),
                    body_range: point_range(body_node),
                    producer: "tree_sitter_parser".to_string(),
                    warning: None,
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_symbols(child, path, language, source, symbols);
    }
}

fn walk_calls(
    node: Node,
    path: &str,
    language: &str,
    source: &[u8],
    calls: &mut Vec<CallCandidate>,
) {
    if is_call_node(node.kind()) {
        if let Some(target_node) = node
            .child_by_field_name("function")
            .or_else(|| first_named_child(node))
        {
            if let Ok(target) = target_node.utf8_text(source) {
                calls.push(CallCandidate {
                    path: path.to_string(),
                    language: language.to_string(),
                    target: target.trim().to_string(),
                    enclosing_symbol: enclosing_symbol(node, source),
                    range: point_range(node),
                    producer: "tree_sitter_call_heuristic".to_string(),
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_calls(child, path, language, source, calls);
    }
}

fn parser_language(path: &Path) -> Option<Language> {
    match language_for_path(path) {
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "javascript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        _ => None,
    }
}

fn symbol_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "function_item" | "function_definition" | "function_declaration" | "method_definition" => {
            Some("function")
        }
        "struct_item" => Some("struct"),
        "enum_item" => Some("enum"),
        "trait_item" => Some("trait"),
        "impl_item" => Some("impl"),
        "mod_item" => Some("module"),
        "class_definition" | "class_declaration" => Some("class"),
        "lexical_declaration" => Some("variable"),
        _ => None,
    }
}

fn is_call_node(kind: &str) -> bool {
    matches!(kind, "call_expression" | "call")
}

fn first_named_child(node: Node) -> Option<Node> {
    (0..node.named_child_count()).find_map(|idx| node.named_child(idx))
}

fn enclosing_symbol(node: Node, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(node) = current {
        if symbol_kind(node.kind()).is_some() {
            if let Some(name_node) = node
                .child_by_field_name("name")
                .or_else(|| first_named_child(node))
            {
                return name_node.utf8_text(source).ok().map(ToString::to_string);
            }
        }
        current = node.parent();
    }
    None
}

fn point_range(node: Node) -> Value {
    let start = node.start_position();
    let end = node.end_position();
    line_range_for_node(start.row, start.column, end.row, end.column)
}

fn symbol_to_json(symbol: Symbol) -> Value {
    json!({
        "path": symbol.path,
        "name": symbol.name,
        "kind": symbol.kind,
        "language": symbol.language,
        "range": symbol.range,
        "bodyRange": symbol.body_range,
        "producer": symbol.producer,
        "reliability": "parser_fact",
        "exact": false,
        "warning": symbol.warning
    })
}

fn call_to_json(call: CallCandidate) -> Value {
    json!({
        "path": call.path,
        "target": call.target,
        "enclosingSymbol": call.enclosing_symbol,
        "language": call.language,
        "range": call.range,
        "producer": call.producer,
        "reliability": "inferred_candidate",
        "exact": false,
        "knownBlindSpots": [
            "dynamic dispatch",
            "trait/interface implementations",
            "reflection",
            "macro generated code",
            "framework injection",
            "alias-heavy imports"
        ]
    })
}

fn last_identifier(target: &str) -> &str {
    target
        .rsplit(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .find(|part| !part.is_empty())
        .unwrap_or(target)
}

use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::Path,
};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::{
    index,
    syntax::{self, CallCandidate},
    workspace::{ScanOptions, Workspace},
};

pub struct GraphQueryOutput {
    pub results: Value,
    pub index: Value,
    pub warnings: Vec<String>,
}

pub(crate) fn write_relations(
    path: &Path,
    workspace: &Workspace,
    opts: &ScanOptions,
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    let records = syntax::collect_calls(workspace, opts, &mut warnings)?;
    let mut file = File::create(path)?;
    for record in records {
        serde_json::to_writer(&mut file, &record)?;
        writeln!(file)?;
    }
    fs::write(
        path.with_file_name("graph-manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "source": "local_relation_graph_store",
            "producer": "tree_sitter_call_heuristic",
            "snapshot_id": workspace.snapshot_id
        }))?,
    )?;
    Ok(warnings)
}

pub fn calls(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<GraphQueryOutput>> {
    query_graph(workspace, opts, |record| {
        record.enclosing_symbol.as_deref() == Some(identifier)
    })
}

pub fn callers(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<GraphQueryOutput>> {
    query_graph(workspace, opts, |record| {
        syntax::last_identifier(&record.target) == identifier
    })
}

fn query_graph(
    workspace: &Workspace,
    opts: &ScanOptions,
    matches: impl Fn(&CallCandidate) -> bool,
) -> Result<Option<GraphQueryOutput>> {
    let Some((records, index_meta)) = fresh_records(workspace, opts)? else {
        return Ok(None);
    };

    let mut results = Vec::new();
    for record in records.into_iter().filter(matches) {
        results.push(record_to_json(record));
        if opts.limit > 0 && results.len() >= opts.limit {
            break;
        }
    }

    Ok(Some(GraphQueryOutput {
        results: Value::Array(results),
        index: index_meta,
        warnings: Vec::new(),
    }))
}

fn fresh_records(
    workspace: &Workspace,
    opts: &ScanOptions,
) -> Result<Option<(Vec<CallCandidate>, Value)>> {
    let path = index::index_root(workspace).join("relations.jsonl");
    if !path.exists() || !path.with_file_name("graph-manifest.json").exists() {
        return Ok(None);
    }

    let mut scan_opts = opts.clone();
    scan_opts.limit = 0;
    let allowed_paths = workspace
        .scan_files(&scan_opts)?
        .into_iter()
        .map(|file| file.path)
        .collect::<HashSet<_>>();

    let records = read_records(&path)?;
    let mut fresh_records = Vec::new();
    for record in records {
        if !allowed_paths.contains(&record.path) {
            continue;
        }
        let hash = match current_file_hash(workspace, &record.path) {
            Ok(hash) => hash,
            Err(_) => return Ok(None),
        };
        if hash != record.file_hash {
            return Ok(None);
        }
        fresh_records.push(record);
    }

    Ok(Some((
        fresh_records,
        json!({
            "used": true,
            "fresh": true,
            "source": "local_relation_graph_store",
            "fallback": false,
            "path": path
        }),
    )))
}

fn current_file_hash(workspace: &Workspace, path: &str) -> Result<String> {
    let content = fs::read(workspace.abs_path(path))?;
    Ok(format!("blake3:{}", blake3::hash(&content).to_hex()))
}

fn read_records(path: &Path) -> Result<Vec<CallCandidate>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        records.push(serde_json::from_str(&line)?);
    }
    Ok(records)
}

fn record_to_json(record: CallCandidate) -> Value {
    json!({
        "path": record.path,
        "target": record.target,
        "enclosingSymbol": record.enclosing_symbol,
        "language": record.language,
        "range": record.range,
        "fileHash": record.file_hash,
        "producer": "local_relation_graph_store",
        "sourceProducer": record.producer,
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

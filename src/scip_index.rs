use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    index,
    search::line_range_for_node,
    workspace::{ScanOptions, Workspace},
};

const ROLE_DEFINITION: i32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreciseOccurrenceRecord {
    path: String,
    language: String,
    symbol: String,
    name: String,
    kind: String,
    role: String,
    range: Value,
    file_hash: String,
    producer: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScipJsonIndex {
    documents: Vec<ScipDocument>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScipDocument {
    #[serde(alias = "relative_path")]
    relative_path: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    occurrences: Vec<ScipOccurrence>,
    #[serde(default)]
    symbols: Vec<ScipSymbol>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScipOccurrence {
    range: Vec<usize>,
    symbol: String,
    #[serde(default, alias = "symbol_roles")]
    symbol_roles: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScipSymbol {
    symbol: String,
    #[serde(default, alias = "display_name")]
    display_name: String,
    #[serde(default)]
    kind: Value,
}

pub struct PreciseQueryOutput {
    pub results: Value,
    pub index: Value,
}

pub fn import_scip_json(workspace: &Workspace, path: impl AsRef<Path>) -> Result<Value> {
    let source_path = path.as_ref();
    let input = fs::read(source_path)
        .with_context(|| format!("failed to read SCIP JSON {}", source_path.display()))?;
    let parsed: ScipJsonIndex = serde_json::from_slice(&input)
        .with_context(|| "failed to parse SCIP JSON; binary index.scip protobuf import is not available in this build")?;

    let root = index::index_root(workspace);
    fs::create_dir_all(&root)?;

    let mut records = Vec::new();
    for document in parsed.documents {
        let symbols = document
            .symbols
            .iter()
            .map(|symbol| (symbol.symbol.as_str(), symbol))
            .collect::<HashMap<_, _>>();
        let file_hash = current_file_hash(workspace, &document.relative_path).unwrap_or_default();
        for occurrence in document.occurrences {
            if occurrence.symbol.is_empty() || occurrence.range.is_empty() {
                continue;
            }
            let Some(range) = scip_range(&occurrence.range) else {
                continue;
            };
            let symbol_info = symbols.get(occurrence.symbol.as_str());
            let name = symbol_info
                .and_then(|info| (!info.display_name.is_empty()).then(|| info.display_name.clone()))
                .unwrap_or_else(|| display_name_from_symbol(&occurrence.symbol));
            let kind = symbol_info
                .map(|info| kind_to_string(&info.kind))
                .filter(|kind| !kind.is_empty())
                .unwrap_or_else(|| "symbol".to_string());
            records.push(PreciseOccurrenceRecord {
                path: document.relative_path.clone(),
                language: document.language.clone(),
                symbol: occurrence.symbol,
                name,
                kind,
                role: if occurrence.symbol_roles & ROLE_DEFINITION != 0 {
                    "definition".to_string()
                } else {
                    "reference".to_string()
                },
                range,
                file_hash: file_hash.clone(),
                producer: "scip".to_string(),
            });
        }
    }

    records.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.name.cmp(&b.name))
            .then(a.role.cmp(&b.role))
    });
    write_records(&root.join("occurrences.jsonl"), &records)?;
    write_records(
        &root.join("declarations.jsonl"),
        &records
            .iter()
            .filter(|record| record.role == "definition")
            .cloned()
            .collect::<Vec<_>>(),
    )?;
    write_records(
        &root.join("symbols.jsonl"),
        &records
            .iter()
            .filter(|record| record.role == "definition")
            .cloned()
            .collect::<Vec<_>>(),
    )?;
    fs::write(
        root.join("scip-manifest.json"),
        serde_json::to_vec_pretty(&json!({
            "source": "scip_json",
            "path": source_path.to_string_lossy(),
            "recordCount": records.len(),
            "definitionCount": records.iter().filter(|record| record.role == "definition").count()
        }))?,
    )?;

    Ok(json!({
        "index": {
            "used": true,
            "fresh": true,
            "source": "scip_json",
            "path": root,
            "recordCount": records.len(),
            "definitionCount": records.iter().filter(|record| record.role == "definition").count()
        }
    }))
}

pub fn symbols(
    workspace: &Workspace,
    opts: &ScanOptions,
    query: &str,
) -> Result<Option<PreciseQueryOutput>> {
    query_precise(workspace, opts, |record| {
        record.role == "definition" && record.name.contains(query)
    })
}

pub fn defs(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<PreciseQueryOutput>> {
    query_precise(workspace, opts, |record| {
        record.role == "definition" && matches_identifier(record, identifier)
    })
}

pub fn refs(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<PreciseQueryOutput>> {
    query_precise(workspace, opts, |record| {
        record.role != "definition" && matches_identifier(record, identifier)
    })
}

fn query_precise(
    workspace: &Workspace,
    opts: &ScanOptions,
    matches: impl Fn(&PreciseOccurrenceRecord) -> bool,
) -> Result<Option<PreciseQueryOutput>> {
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
    Ok(Some(PreciseQueryOutput {
        results: Value::Array(results),
        index: index_meta,
    }))
}

fn fresh_records(
    workspace: &Workspace,
    opts: &ScanOptions,
) -> Result<Option<(Vec<PreciseOccurrenceRecord>, Value)>> {
    let root = index::index_root(workspace);
    let path = root.join("occurrences.jsonl");
    if !path.exists() || !root.join("scip-manifest.json").exists() {
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
            "source": "scip_json",
            "fallback": false,
            "path": path
        }),
    )))
}

fn current_file_hash(workspace: &Workspace, path: &str) -> Result<String> {
    let content = fs::read(workspace.abs_path(path))?;
    Ok(format!("blake3:{}", blake3::hash(&content).to_hex()))
}

fn scip_range(range: &[usize]) -> Option<Value> {
    match range {
        [start_line, start_col, end_col] => Some(line_range_for_node(
            *start_line,
            *start_col,
            *start_line,
            *end_col,
        )),
        [start_line, start_col, end_line, end_col] => Some(line_range_for_node(
            *start_line,
            *start_col,
            *end_line,
            *end_col,
        )),
        _ => None,
    }
}

fn display_name_from_symbol(symbol: &str) -> String {
    symbol
        .split(|ch: char| ch == '/' || ch == '#' || ch == '.' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .next_back()
        .unwrap_or(symbol)
        .trim_end_matches("().")
        .to_string()
}

fn kind_to_string(kind: &Value) -> String {
    match kind {
        Value::String(value) => value.to_ascii_lowercase(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    }
}

fn matches_identifier(record: &PreciseOccurrenceRecord, identifier: &str) -> bool {
    record.name == identifier || record.symbol == identifier
}

fn record_to_json(record: PreciseOccurrenceRecord) -> Value {
    json!({
        "path": record.path,
        "name": record.name,
        "kind": record.kind,
        "symbol": record.symbol,
        "role": record.role,
        "language": record.language,
        "range": record.range,
        "fileHash": record.file_hash,
        "producer": record.producer,
        "reliability": "precise_fact",
        "exact": true
    })
}

fn write_records(path: &Path, records: &[PreciseOccurrenceRecord]) -> Result<()> {
    let mut file = File::create(path)?;
    for record in records {
        serde_json::to_writer(&mut file, record)?;
        writeln!(file)?;
    }
    Ok(())
}

fn read_records(path: &Path) -> Result<Vec<PreciseOccurrenceRecord>> {
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

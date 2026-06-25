use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    java_semantic::{
        classfile, extract,
        hierarchy::{self, CallHierarchyOptions},
        lombok,
        model::{
            ExtractedJavaFile, JavaCallEdge, JavaSemanticData, JavaSemanticManifest, JavaSymbol,
            JavaSymbolKind, ResolveConfidence, SourceRange, SymbolOrigin,
        },
        resolver::{self, ResolverInput},
    },
    output,
    project_graph::{discover_project_graph, ProjectLanguage},
    query_input::{attach_matched_input, InputPlan, SymbolMatchMode},
    scip, scip_index,
    workspace::{FileRecord, ScanOptions, Workspace, MAX_FILE_BYTES},
};

const SCHEMA_VERSION: u32 = 1;
const PRODUCER: &str = "java_semantic_resolver";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaSemanticBuildReport {
    pub attempted: bool,
    pub skipped: bool,
    pub skip_reason: Option<String>,
    pub path: Option<String>,
    pub file_count: usize,
    pub symbol_count: usize,
    pub call_edge_count: usize,
    pub classpath_symbol_count: usize,
}

impl JavaSemanticBuildReport {
    pub fn skipped(reason: &str) -> Self {
        Self {
            attempted: false,
            skipped: true,
            skip_reason: Some(reason.to_string()),
            path: None,
            file_count: 0,
            symbol_count: 0,
            call_edge_count: 0,
            classpath_symbol_count: 0,
        }
    }
}

pub fn build(
    workspace: &Workspace,
    records: &[FileRecord],
    snapshot_id: &str,
    verbose: output::VerboseLogger,
) -> Result<JavaSemanticBuildReport> {
    let mut java_records = java_records(workspace, records)?;
    java_records.sort_by(|a, b| a.path.cmp(&b.path));
    java_records.dedup_by(|a, b| a.path == b.path);
    if java_records.is_empty() {
        return Ok(JavaSemanticBuildReport::skipped("no_java_sources"));
    }

    verbose.log(format!(
        "java semantic: extracting files={}",
        java_records.len()
    ));
    let root_ids = root_ids_by_path(workspace);
    let extracted = java_records
        .par_iter()
        .filter_map(|record| {
            let root_id = root_ids
                .get(&record.path)
                .cloned()
                .unwrap_or_else(|| "java:.".to_string());
            let generated = is_generated_path(&record.path);
            match extract::extract_file(workspace, record, &root_id, generated) {
                Ok(mut file) => {
                    lombok::apply_lombok_overlay(&mut file);
                    Some(file)
                }
                Err(error) => {
                    verbose.log(format!("java semantic: skipped {}: {error}", record.path));
                    None
                }
            }
        })
        .collect::<Vec<ExtractedJavaFile>>();

    let root_id = root_ids
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| "java:.".to_string());
    let mut external_symbols = classfile::load_classpath_symbols(workspace, &root_id);
    let classpath_symbol_count = external_symbols.len();
    let mut extracted = extracted;
    merge_scip_symbols(workspace, &mut extracted);
    external_symbols.sort_by(|a, b| a.symbol_id.cmp(&b.symbol_id));
    external_symbols.dedup_by(|a, b| a.symbol_id == b.symbol_id);

    let manifest = JavaSemanticManifest {
        schema_version: SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        snapshot_id: snapshot_id.to_string(),
        snapshot_key: crate::index::snapshot_key(snapshot_id),
        source: "java_semantic_resolver".to_string(),
        file_count: 0,
        symbol_count: 0,
        occurrence_count: 0,
        call_edge_count: 0,
        type_edge_count: 0,
    };
    let data = resolver::resolve(ResolverInput {
        manifest,
        files: extracted,
        external_symbols,
    });
    let dir = semantic_dir_for_snapshot(workspace, snapshot_id);
    write_data(&dir, &data)?;
    Ok(JavaSemanticBuildReport {
        attempted: true,
        skipped: false,
        skip_reason: None,
        path: Some(dir.to_string_lossy().to_string()),
        file_count: data.manifest.file_count,
        symbol_count: data.manifest.symbol_count,
        call_edge_count: data.manifest.call_edge_count,
        classpath_symbol_count,
    })
}

pub fn is_fresh(workspace: &Workspace) -> bool {
    read_manifest(&semantic_dir(workspace).join("manifest.json"))
        .is_ok_and(|manifest| manifest.snapshot_id == workspace.snapshot_id)
}

pub fn index_meta(workspace: &Workspace, fresh: bool) -> Value {
    json!({
        "used": true,
        "fresh": fresh,
        "source": "java_semantic",
        "fallback": false,
        "path": semantic_dir(workspace),
        "snapshot_id": workspace.snapshot_id,
    })
}

pub fn calls(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<(Value, Value)>> {
    let Some(data) = load_fresh(workspace)? else {
        return Ok(None);
    };
    let plan = InputPlan::new(identifier, opts.input_mode);
    let path_filter = PathFilter::new(workspace, opts)?;
    let symbols = symbol_lookup(&data);
    let mut results = Vec::new();
    for edge in &data.call_edges {
        let Some(caller) = symbols.get(edge.caller_symbol.as_str()).copied() else {
            continue;
        };
        let Some(variant) = matched_symbol_variant(caller, &plan, opts.case_sensitive) else {
            continue;
        };
        if !path_filter.allows(&edge.path) {
            continue;
        }
        results.push(attach_matched_input(
            call_candidate_json(&symbols, edge),
            variant,
        ));
    }
    finalize_results(&mut results, opts.limit);
    if results.is_empty() {
        return Ok(None);
    }
    Ok(Some((index_meta(workspace, true), Value::Array(results))))
}

pub fn callers(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<(Value, Value)>> {
    let Some(data) = load_fresh(workspace)? else {
        return Ok(None);
    };
    let plan = InputPlan::new(identifier, opts.input_mode);
    let path_filter = PathFilter::new(workspace, opts)?;
    let symbols = symbol_lookup(&data);
    let mut results = Vec::new();
    for edge in &data.call_edges {
        let variant = edge
            .callee_symbol
            .as_deref()
            .and_then(|callee| symbols.get(callee).copied())
            .and_then(|callee| matched_symbol_variant(callee, &plan, opts.case_sensitive))
            .or_else(|| {
                plan.matched_variant(
                    &edge.target_name,
                    opts.case_sensitive,
                    SymbolMatchMode::Exact,
                )
            });
        let Some(variant) = variant else {
            continue;
        };
        if !path_filter.allows(&edge.path) {
            continue;
        }
        results.push(attach_matched_input(
            call_candidate_json(&symbols, edge),
            variant,
        ));
    }
    finalize_results(&mut results, opts.limit);
    if results.is_empty() {
        return Ok(None);
    }
    Ok(Some((index_meta(workspace, true), Value::Array(results))))
}

pub fn query_call_hierarchy(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
    hierarchy_opts: CallHierarchyOptions,
) -> Result<Option<(Value, Value)>> {
    let Some(data) = load_fresh(workspace)? else {
        return Ok(None);
    };
    let plan = InputPlan::new(identifier, opts.input_mode);
    let path_filter = PathFilter::new(workspace, opts)?;
    let mut roots = data
        .symbols
        .iter()
        .filter(|symbol| {
            matches!(
                symbol.kind,
                JavaSymbolKind::Method
                    | JavaSymbolKind::Constructor
                    | JavaSymbolKind::SyntheticMethod
            )
        })
        .filter(|symbol| matched_symbol_variant(symbol, &plan, opts.case_sensitive).is_some())
        .filter(|symbol| {
            symbol
                .path
                .as_deref()
                .map(|path| path_filter.allows(path))
                .unwrap_or(true)
        })
        .cloned()
        .collect::<Vec<_>>();
    roots.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    roots.dedup_by(|a, b| a.symbol_id == b.symbol_id);
    if opts.limit > 0 && roots.len() > opts.limit {
        roots.truncate(opts.limit);
    }
    if roots.is_empty() {
        return Ok(None);
    }
    let results = hierarchy::hierarchy_for_roots(&data, &roots, hierarchy_opts, opts.limit);
    Ok(Some((index_meta(workspace, true), Value::Array(results))))
}

fn load_fresh(workspace: &Workspace) -> Result<Option<JavaSemanticData>> {
    let dir = semantic_dir(workspace);
    let manifest_path = dir.join("manifest.json");
    let manifest = match read_manifest(&manifest_path) {
        Ok(manifest) if manifest.snapshot_id == workspace.snapshot_id => manifest,
        _ => return Ok(None),
    };
    Ok(Some(read_data(&dir, manifest)?))
}

fn java_records(workspace: &Workspace, records: &[FileRecord]) -> Result<Vec<FileRecord>> {
    let mut result = records
        .iter()
        .filter(|record| record.language == "java" || record.path.ends_with(".java"))
        .cloned()
        .collect::<Vec<_>>();
    result.extend(generated_source_records(workspace, records)?);
    Ok(result)
}

fn generated_source_records(
    workspace: &Workspace,
    source_records: &[FileRecord],
) -> Result<Vec<FileRecord>> {
    let mut records = Vec::new();
    let module_roots = java_module_roots(workspace, source_records);
    for base in module_roots {
        for rel in generated_source_rel_paths() {
            let dir = base.join(rel);
            collect_generated_sources(workspace, &dir, &mut records)?;
        }
    }
    records.sort_by(|a, b| a.path.cmp(&b.path));
    records.dedup_by(|a, b| a.path == b.path);
    Ok(records)
}

fn generated_source_rel_paths() -> &'static [&'static str] {
    &[
        "target/generated-sources/annotations",
        "target/generated-test-sources/test-annotations",
        "build/generated/sources/annotationProcessor/java/main",
        "build/generated/sources/annotationProcessor/java/test",
        "build/generated/sources/delombok",
        "generated/sources/annotationProcessor/java/main",
    ]
}

fn java_module_roots(workspace: &Workspace, records: &[FileRecord]) -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::from([workspace.root.clone()]);
    for record in records {
        if record.language == "java" || record.path.ends_with(".java") {
            if let Some(root) = module_root_from_java_source(&workspace.root, &record.path) {
                roots.insert(root);
            }
        }
    }
    if let Ok(graph) = discover_project_graph(&workspace.root) {
        for owner in graph.source_owners {
            if owner.language != ProjectLanguage::Java {
                continue;
            }
            if let Some(root) = module_root_from_java_source(&workspace.root, &owner.path) {
                roots.insert(root);
            }
        }
    }
    roots
}

fn module_root_from_java_source(workspace_root: &Path, rel_path: &str) -> Option<PathBuf> {
    for marker in [
        "/src/main/java/",
        "/src/test/java/",
        "/src/integrationTest/java/",
        "/src/it/java/",
    ] {
        if let Some(prefix) = rel_path.split_once(marker).map(|(prefix, _)| prefix) {
            return Some(if prefix.is_empty() {
                workspace_root.to_path_buf()
            } else {
                workspace_root.join(prefix)
            });
        }
    }
    for marker in [
        "src/main/java/",
        "src/test/java/",
        "src/integrationTest/java/",
        "src/it/java/",
    ] {
        if rel_path.starts_with(marker) {
            return Some(workspace_root.to_path_buf());
        }
    }
    None
}

fn collect_generated_sources(
    workspace: &Workspace,
    dir: &Path,
    records: &mut Vec<FileRecord>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in WalkBuilder::new(dir).hidden(false).build().flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|ext| ext == "java") {
            continue;
        }
        let metadata = match fs::metadata(path) {
            Ok(metadata) if metadata.len() <= MAX_FILE_BYTES => metadata,
            _ => continue,
        };
        let content = match fs::read(path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let rel_path = path
            .strip_prefix(&workspace.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        records.push(FileRecord {
            path: rel_path,
            language: "java".to_string(),
            size: metadata.len(),
            mtime_ms: metadata
                .modified()
                .ok()
                .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis())
                .unwrap_or_default(),
            mode: 0,
            hash: format!("blake3:{}", blake3::hash(&content).to_hex()),
        });
    }
    Ok(())
}

fn root_ids_by_path(workspace: &Workspace) -> BTreeMap<String, String> {
    let Ok(graph) = discover_project_graph(&workspace.root) else {
        return BTreeMap::new();
    };
    let mut roots = BTreeMap::new();
    for owner in graph.source_owners {
        if owner.language == ProjectLanguage::Java {
            roots.insert(owner.path, owner.root_id);
        }
    }
    for generated in graph.generated_sources {
        if generated.language == ProjectLanguage::Java {
            roots.insert(generated.path, generated.owner_root_id);
        }
    }
    roots
}

fn merge_scip_symbols(workspace: &Workspace, files: &mut [ExtractedJavaFile]) {
    let db_path = scip_index::native_db_path(workspace);
    if !db_path.exists()
        || !scip::occurrence_db_fresh(&db_path, &workspace.snapshot_id, &workspace.root)
    {
        return;
    }
    let Ok(symbols) = scip::query_all_symbols(&db_path) else {
        return;
    };
    let by_site = symbols
        .into_iter()
        .filter(|symbol| symbol.language == "java" && symbol.role == "definition")
        .map(|symbol| {
            (
                (symbol.path, symbol.name, symbol.start_line),
                symbol.symbol_key,
            )
        })
        .collect::<BTreeMap<_, _>>();
    for file in files {
        for symbol in &mut file.symbols {
            let Some(range) = &symbol.range else {
                continue;
            };
            let key = (file.path.clone(), symbol.name.clone(), range.start_line);
            if let Some(scip_symbol) = by_site.get(&key) {
                symbol.symbol_id = scip_symbol.clone();
                symbol.origin = SymbolOrigin::Scip;
                symbol.confidence = ResolveConfidence::Scip;
            }
        }
    }
}

fn semantic_dir(workspace: &Workspace) -> PathBuf {
    semantic_dir_for_snapshot(workspace, &workspace.snapshot_id)
}

pub fn semantic_dir_for_snapshot(workspace: &Workspace, snapshot_id: &str) -> PathBuf {
    workspace
        .root
        .join(".codetrail")
        .join("java-semantic")
        .join(crate::index::snapshot_key(snapshot_id))
}

fn write_data(dir: &Path, data: &JavaSemanticData) -> Result<()> {
    let parent = dir
        .parent()
        .with_context(|| format!("semantic index path has no parent: {}", dir.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let tmp = sidecar_dir(dir, "tmp");
    let backup = sidecar_dir(dir, "old");
    if tmp.exists() {
        fs::remove_dir_all(&tmp)
            .with_context(|| format!("failed to remove stale {}", tmp.display()))?;
    }
    if backup.exists() {
        fs::remove_dir_all(&backup)
            .with_context(|| format!("failed to remove stale {}", backup.display()))?;
    }
    fs::create_dir_all(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    write_jsonl(&tmp.join("symbols.jsonl"), &data.symbols)?;
    write_jsonl(&tmp.join("occurrences.jsonl"), &data.occurrences)?;
    write_jsonl(&tmp.join("call_edges.jsonl"), &data.call_edges)?;
    write_jsonl(&tmp.join("type_edges.jsonl"), &data.type_edges)?;
    write_jsonl(
        &tmp.join("file_contributions.jsonl"),
        &data.file_contributions,
    )?;
    write_manifest(&tmp.join("manifest.json"), &data.manifest)?;
    if dir.exists() {
        fs::rename(dir, &backup)
            .with_context(|| format!("failed to move {} to {}", dir.display(), backup.display()))?;
    }
    if let Err(error) = fs::rename(&tmp, dir) {
        if backup.exists() && !dir.exists() {
            let _ = fs::rename(&backup, dir);
        }
        return Err(error)
            .with_context(|| format!("failed to activate semantic index {}", dir.display()));
    }
    if backup.exists() {
        fs::remove_dir_all(&backup)
            .with_context(|| format!("failed to remove {}", backup.display()))?;
    }
    Ok(())
}

fn sidecar_dir(dir: &Path, suffix: &str) -> PathBuf {
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("semantic");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    dir.with_file_name(format!("{name}.{suffix}-{}-{stamp}", std::process::id()))
}

fn read_data(dir: &Path, manifest: JavaSemanticManifest) -> Result<JavaSemanticData> {
    Ok(JavaSemanticData {
        manifest,
        symbols: read_jsonl(&dir.join("symbols.jsonl"))?,
        occurrences: read_jsonl(&dir.join("occurrences.jsonl"))?,
        call_edges: read_jsonl(&dir.join("call_edges.jsonl"))?,
        type_edges: read_jsonl(&dir.join("type_edges.jsonl"))?,
        file_contributions: read_jsonl(&dir.join("file_contributions.jsonl"))?,
    })
}

fn write_manifest(path: &Path, manifest: &JavaSemanticManifest) -> Result<()> {
    let mut file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, manifest)?;
    writeln!(file)?;
    Ok(())
}

fn read_manifest(path: &Path) -> Result<JavaSemanticManifest> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    serde_json::from_reader(file).with_context(|| format!("failed to read {}", path.display()))
}

fn write_jsonl<T: serde::Serialize>(path: &Path, values: &[T]) -> Result<()> {
    let file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    for value in values {
        serde_json::to_writer(&mut writer, value)?;
        writeln!(writer)?;
    }
    Ok(())
}

fn read_jsonl<T: for<'de> serde::Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    reader
        .lines()
        .filter(|line| line.as_ref().is_ok_and(|line| !line.trim().is_empty()))
        .map(|line| {
            let line = line?;
            serde_json::from_str(&line).map_err(anyhow::Error::from)
        })
        .collect()
}

fn matched_symbol_variant<'a>(
    symbol: &JavaSymbol,
    plan: &'a InputPlan,
    case_sensitive: bool,
) -> Option<&'a crate::query_input::InputVariant> {
    for candidate in [
        symbol.symbol_id.as_str(),
        symbol.name.as_str(),
        symbol.qualified_name.as_str(),
    ]
    .into_iter()
    {
        if let Some(variant) =
            plan.matched_variant(candidate, case_sensitive, SymbolMatchMode::Exact)
        {
            return Some(variant);
        }
    }
    let signature = symbol.display_signature();
    plan.matched_variant(&signature, case_sensitive, SymbolMatchMode::Exact)
}

fn call_candidate_json(symbols: &BTreeMap<&str, &JavaSymbol>, edge: &JavaCallEdge) -> Value {
    let caller = symbols.get(edge.caller_symbol.as_str()).copied();
    let callee = edge
        .callee_symbol
        .as_deref()
        .and_then(|symbol_id| symbols.get(symbol_id).copied());
    json!({
        "path": edge.path,
        "target": callee.map(|symbol| symbol.name.clone()).unwrap_or_else(|| edge.target_name.clone()),
        "targetDetail": callee.map(|symbol| symbol.qualified_name.clone()),
        "targetSignature": callee.map(JavaSymbol::display_signature),
        "targetSymbolId": edge.callee_symbol.clone(),
        "kind": "call",
        "enclosingSymbol": caller.map(|symbol| symbol.name.clone()),
        "enclosingSymbolDetail": caller.map(|symbol| symbol.qualified_name.clone()),
        "enclosingSymbolSignature": caller.map(JavaSymbol::display_signature),
        "enclosingSymbolId": edge.caller_symbol.clone(),
        "language": "java",
        "rootId": caller.map(|symbol| symbol.root_id.clone()).unwrap_or_else(|| "java:.".to_string()),
        "range": edge.range.to_codetrail_json(),
        "fileHash": edge.file_hash,
        "producer": PRODUCER,
        "reliability": "inferred_candidate",
        "layer": "inferred_candidate",
        "exact": false,
        "source": "java_semantic",
        "level": "inferred_candidate",
        "dispatchKind": format!("{:?}", edge.dispatch_kind).to_lowercase(),
        "resolveStatus": format!("{:?}", edge.status),
        "confidence": format!("{:?}", edge.confidence).to_lowercase(),
    })
}

fn finalize_results(results: &mut Vec<Value>, limit: usize) {
    results.sort_by(|a, b| {
        let ap = a.get("path").and_then(Value::as_str).unwrap_or_default();
        let bp = b.get("path").and_then(Value::as_str).unwrap_or_default();
        let al = a["range"]["start"]["line"].as_u64().unwrap_or(0);
        let bl = b["range"]["start"]["line"].as_u64().unwrap_or(0);
        ap.cmp(bp).then(al.cmp(&bl))
    });
    results.dedup_by(|a, b| {
        a.get("path") == b.get("path")
            && a["range"]["start"] == b["range"]["start"]
            && a.get("target") == b.get("target")
            && a.get("enclosingSymbol") == b.get("enclosingSymbol")
    });
    if limit > 0 && results.len() > limit {
        results.truncate(limit);
    }
}

struct PathFilter {
    allowed: Option<BTreeSet<String>>,
}

impl PathFilter {
    fn new(workspace: &Workspace, opts: &ScanOptions) -> Result<Self> {
        if !scope_restricts_paths(opts) {
            return Ok(Self { allowed: None });
        }
        let mut scope_opts = opts.clone();
        scope_opts.limit = 0;
        let allowed = workspace
            .scan_catalog(&scope_opts)?
            .into_iter()
            .map(|record| record.path)
            .collect::<BTreeSet<_>>();
        Ok(Self {
            allowed: Some(allowed),
        })
    }

    fn allows(&self, path: &str) -> bool {
        self.allowed
            .as_ref()
            .map(|allowed| allowed.contains(path))
            .unwrap_or(true)
    }
}

fn scope_restricts_paths(opts: &ScanOptions) -> bool {
    opts.changed
        || !opts.dirs.is_empty()
        || !opts.extensions.is_empty()
        || !opts.file_patterns.is_empty()
        || !opts.include.is_empty()
        || !opts.exclude.is_empty()
        || !opts.lang.is_empty()
}

fn symbol_lookup(data: &JavaSemanticData) -> BTreeMap<&str, &JavaSymbol> {
    data.symbols
        .iter()
        .map(|symbol| (symbol.symbol_id.as_str(), symbol))
        .collect()
}

fn is_generated_path(path: &str) -> bool {
    path.contains("/generated/")
        || path.contains("generated-sources")
        || path.contains("generated-test-sources")
        || path.contains("annotationProcessor")
        || path.contains("delombok")
}

#[allow(dead_code)]
fn source_range_from_scip(
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
) -> SourceRange {
    SourceRange::new(start_line, start_column, end_line, end_column)
}

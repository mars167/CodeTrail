use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    java_semantic::{
        classfile, extract,
        hierarchy::CallHierarchyOptions,
        lombok,
        model::{ExtractedJavaFile, JavaSemanticManifest, ResolveConfidence, SymbolOrigin},
        resolver::{self, ResolverInput},
        store,
    },
    output,
    project_graph::{discover_project_graph, ProjectLanguage},
    scip, scip_index,
    workspace::{FileRecord, ScanOptions, Workspace, MAX_FILE_BYTES},
};

const SCHEMA_VERSION: u32 = 1;
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
    cleanup_legacy_json_artifacts(workspace)?;
    let mut store = store::JavaSemanticStore::open_or_create(&workspace.root)?;
    store.write_snapshot(&data, classpath_symbol_count)?;
    let db_path = store.path().to_path_buf();
    Ok(JavaSemanticBuildReport {
        attempted: true,
        skipped: false,
        skip_reason: None,
        path: Some(db_path.to_string_lossy().to_string()),
        file_count: data.manifest.file_count,
        symbol_count: data.manifest.symbol_count,
        call_edge_count: data.manifest.call_edge_count,
        classpath_symbol_count,
    })
}

pub fn is_fresh(workspace: &Workspace) -> bool {
    store::is_fresh(workspace)
}

pub fn index_meta(workspace: &Workspace, fresh: bool) -> Value {
    let mut value = store::index_meta(workspace, fresh);
    if !fresh {
        value["fresh"] = Value::Bool(false);
    }
    value
}

pub fn calls(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<(Value, Value)>> {
    let Some(mut store) = store::JavaSemanticStore::open_existing(&workspace.root)? else {
        return Ok(None);
    };
    store.calls(workspace, opts, identifier)
}

pub fn callers(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<(Value, Value)>> {
    let Some(mut store) = store::JavaSemanticStore::open_existing(&workspace.root)? else {
        return Ok(None);
    };
    store.callers(workspace, opts, identifier)
}

pub fn query_call_hierarchy(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
    hierarchy_opts: CallHierarchyOptions,
) -> Result<Option<(Value, Value)>> {
    let Some(mut store) = store::JavaSemanticStore::open_existing(&workspace.root)? else {
        return Ok(None);
    };
    store.call_hierarchy(workspace, opts, identifier, hierarchy_opts)
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

fn is_generated_path(path: &str) -> bool {
    path.contains("/generated/")
        || path.contains("generated-sources")
        || path.contains("generated-test-sources")
        || path.contains("annotationProcessor")
        || path.contains("delombok")
}

fn cleanup_legacy_json_artifacts(workspace: &Workspace) -> Result<()> {
    let legacy_dir = workspace.root.join(".codetrail").join("java-semantic");
    if legacy_dir.exists() {
        fs::remove_dir_all(&legacy_dir)
            .with_context(|| format!("failed to remove {}", legacy_dir.display()))?;
    }
    Ok(())
}

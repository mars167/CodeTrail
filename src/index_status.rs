use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde_json::{json, Value};

use crate::{
    generation_manifest::{GenerationManifest, ManifestState},
    index::scip_root,
    lsp::registry::{resolve_binary, resolve_server},
    project_graph::{discover_project_graph, ProjectLanguage, ProjectRoot},
    scip,
    workspace::{FileRecord, Workspace},
};

pub(crate) fn indexed_languages(records: &[FileRecord]) -> Value {
    let mut counts = BTreeMap::<String, usize>::new();
    for record in records {
        *counts.entry(record.language.clone()).or_insert(0) += 1;
    }
    Value::Array(
        counts
            .into_iter()
            .map(|(language, file_count)| json!({ "language": language, "fileCount": file_count }))
            .collect(),
    )
}

pub(crate) fn semantic_status(
    workspace: &Workspace,
    records: &[FileRecord],
    manifests: &[GenerationManifest],
) -> Value {
    let db_path = scip_root(workspace).join("occurrences.db");
    let db_exists = db_path.exists();
    let db_fresh = scip::occurrence_db_fresh(&db_path, &workspace.snapshot_id, &workspace.root);
    let (scip_languages, scip_symbol_count, scip_read_error) = scip_language_summary(&db_path);

    let graph = discover_project_graph(&workspace.root);
    let (roots, language_servers, graph_error) = match graph {
        Ok(graph) => semantic_roots_and_servers(&graph.roots, manifests),
        Err(error) => (
            Vec::new(),
            Vec::new(),
            Some(format!("project graph discovery failed: {error}")),
        ),
    };

    let mut scip_index = json!({
        "enabled": db_exists || !manifests.is_empty(),
        "available": db_exists,
        "usable": db_exists && db_fresh,
        "fresh": db_fresh,
        "state": aggregate_semantic_state(manifests, db_exists, db_fresh),
        "path": db_path,
        "symbolCount": scip_symbol_count,
        "languages": scip_languages,
    });
    if let Some(error) = scip_read_error {
        scip_index["readError"] = Value::String(error);
    }

    let mut status = json!({
        "indexedLanguages": indexed_languages(records),
        "scipIndex": scip_index,
        "roots": roots,
        "languageServers": language_servers,
    });
    if let Some(error) = graph_error {
        status["projectGraphError"] = Value::String(error);
    }
    status
}

fn scip_language_summary(db_path: &Path) -> (Value, usize, Option<String>) {
    if !db_path.exists() {
        return (Value::Array(Vec::new()), 0, None);
    }
    let symbols = match scip::query_all_symbols(db_path) {
        Ok(symbols) => symbols,
        Err(error) => return (Value::Array(Vec::new()), 0, Some(error.to_string())),
    };
    let mut counts = BTreeMap::<String, usize>::new();
    for symbol in &symbols {
        *counts.entry(symbol.language.clone()).or_insert(0) += 1;
    }
    (
        Value::Array(
            counts
                .into_iter()
                .map(|(language, symbol_count)| {
                    json!({ "language": language, "symbolCount": symbol_count })
                })
                .collect(),
        ),
        symbols.len(),
        None,
    )
}

fn semantic_roots_and_servers(
    roots: &[ProjectRoot],
    manifests: &[GenerationManifest],
) -> (Vec<Value>, Vec<Value>, Option<String>) {
    let mut manifest_by_root = BTreeMap::<&str, &GenerationManifest>::new();
    for manifest in manifests {
        manifest_by_root.insert(&manifest.root_id, manifest);
    }

    let root_values = roots
        .iter()
        .map(|root| {
            let manifest = manifest_by_root.get(root.id.as_str()).copied();
            json!({
                "rootId": root.id,
                "path": root.path,
                "language": root.language.to_string(),
                "kind": &root.kind,
                "semanticState": manifest
                    .map(|manifest| manifest_state_name(&manifest.state))
                    .unwrap_or("not_generated"),
                "provider": manifest
                    .map(|manifest| manifest.provider_name.as_str())
                    .unwrap_or(default_lsp_command(&root.language)),
                "partialReasons": manifest
                    .map(|manifest| manifest.partial_reasons.clone())
                    .unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();

    let mut languages = BTreeSet::<ProjectLanguage>::new();
    for root in roots {
        languages.insert(root.language.clone());
    }
    let language_servers = languages
        .iter()
        .map(language_server_status)
        .collect::<Vec<_>>();

    (root_values, language_servers, None)
}

fn language_server_status(language: &ProjectLanguage) -> Value {
    let env_key = lsp_env_key(language);
    let env_override = std::env::var(env_key).ok();
    let default_command = default_lsp_command(language);
    let spec = resolve_server(language);
    match spec {
        Some(spec) => {
            let available = lsp_program_available(&spec.program);
            let missing_dependencies = if available {
                Vec::new()
            } else {
                vec![spec.program.clone()]
            };
            let mut value = json!({
                "language": language.to_string(),
                "required": true,
                "status": if available { "available" } else { "missing" },
                "available": available,
                "provider": spec.provider_id,
                "program": spec.program,
                "args": spec.args,
                "envKey": env_key,
                "defaultCommand": default_command,
                "missingDependencies": missing_dependencies,
            });
            if let Some(env_override) = env_override {
                value["envOverride"] = Value::String(env_override);
            }
            value
        }
        None => {
            let mut value = json!({
                "language": language.to_string(),
                "required": true,
                "status": "missing",
                "available": false,
                "provider": default_command,
                "program": Value::Null,
                "args": Vec::<String>::new(),
                "envKey": env_key,
                "defaultCommand": default_command,
                "missingDependencies": [default_command],
            });
            if let Some(env_override) = env_override {
                value["envOverride"] = Value::String(env_override);
            }
            value
        }
    }
}

fn lsp_program_available(program: &str) -> bool {
    resolve_binary(program).is_some()
}

const fn default_lsp_command(language: &ProjectLanguage) -> &'static str {
    match language {
        ProjectLanguage::Go => "gopls",
        ProjectLanguage::Rust => "rust-analyzer",
        ProjectLanguage::Java => "jdtls",
        ProjectLanguage::TypeScript => "typescript-language-server",
        ProjectLanguage::Ruby => "ruby-lsp",
    }
}

const fn lsp_env_key(language: &ProjectLanguage) -> &'static str {
    match language {
        ProjectLanguage::Go => "CODETRAIL_LSP_GO",
        ProjectLanguage::Rust => "CODETRAIL_LSP_RUST",
        ProjectLanguage::Java => "CODETRAIL_LSP_JAVA",
        ProjectLanguage::TypeScript => "CODETRAIL_LSP_TYPESCRIPT",
        ProjectLanguage::Ruby => "CODETRAIL_LSP_RUBY",
    }
}

fn aggregate_semantic_state(
    manifests: &[GenerationManifest],
    db_exists: bool,
    db_fresh: bool,
) -> &'static str {
    if db_exists && !db_fresh {
        return "stale";
    }
    if manifests.is_empty() {
        return if db_exists { "fresh" } else { "not_generated" };
    }
    if !db_exists {
        return "missing";
    }

    let mut states = BTreeSet::<&'static str>::new();
    for manifest in manifests {
        states.insert(manifest_state_name(&manifest.state));
    }
    if states.len() == 1 {
        return states.into_iter().next().unwrap_or("not_generated");
    }
    if states.contains("stale") {
        "stale"
    } else if states.contains("updating") {
        "updating"
    } else if states.contains("partial") {
        "partial"
    } else {
        "mixed"
    }
}

const fn manifest_state_name(state: &ManifestState) -> &'static str {
    match state {
        ManifestState::Fresh => "fresh",
        ManifestState::Stale => "stale",
        ManifestState::Updating => "updating",
        ManifestState::Partial => "partial",
        ManifestState::Missing => "missing",
    }
}

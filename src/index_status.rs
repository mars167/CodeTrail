use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde_json::{json, Value};

use crate::{
    generation_manifest::{GenerationManifest, ManifestState},
    index::scip_root,
    lsp::registry::resolve_binary,
    project_graph::{
        discover_project_graph, ProjectGraph, ProjectLanguage, ProjectRoot, ProjectRootKind,
    },
    provider_help::{requirement_for_language, ProviderRequirement},
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
    let (roots, semantic_providers, graph_error) = match graph {
        Ok(graph) => semantic_roots_and_providers(workspace, &graph, manifests),
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
        "semanticProviders": semantic_providers.clone(),
        "languageServers": semantic_providers,
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

fn semantic_roots_and_providers(
    workspace: &Workspace,
    graph: &ProjectGraph,
    manifests: &[GenerationManifest],
) -> (Vec<Value>, Vec<Value>, Option<String>) {
    let mut manifest_by_root = BTreeMap::<&str, &GenerationManifest>::new();
    for manifest in manifests {
        manifest_by_root.insert(&manifest.root_id, manifest);
    }

    let root_values = graph
        .roots
        .iter()
        .map(|root| {
            let manifest = manifest_by_root.get(root.id.as_str()).copied();
            let mut value = json!({
                "rootId": root.id,
                "path": root.path,
                "language": root.language.to_string(),
                "kind": &root.kind,
                "semanticState": manifest
                    .map(|manifest| manifest_state_name(&manifest.state))
                    .unwrap_or("not_generated"),
                "provider": manifest
                    .map(|manifest| manifest.provider_name.as_str())
                    .unwrap_or(requirement_for_language(&root.language).provider),
                "partialReasons": manifest
                    .map(|manifest| manifest.partial_reasons.clone())
                    .unwrap_or_default(),
            });
            if let Some(config) = swift_config_status(workspace, root) {
                value["swiftConfig"] = config;
            }
            value
        })
        .collect::<Vec<_>>();

    let mut languages = BTreeSet::<ProjectLanguage>::new();
    for root in &graph.roots {
        languages.insert(root.language.clone());
    }
    let semantic_providers = languages
        .iter()
        .map(semantic_provider_status)
        .collect::<Vec<_>>();

    (root_values, semantic_providers, None)
}

fn swift_config_status(workspace: &Workspace, root: &ProjectRoot) -> Option<Value> {
    if root.language != ProjectLanguage::Swift {
        return None;
    }
    match root.kind {
        ProjectRootKind::SwiftPackage => Some(json!({
            "kind": "swiftpm",
            "ready": true,
            "status": "configured",
            "message": "SwiftPM root is SourceKit-LSP eligible"
        })),
        ProjectRootKind::SwiftXcodeProject | ProjectRootKind::SwiftXcodeWorkspace => {
            let root_path = if root.path == "." {
                workspace.root.clone()
            } else {
                workspace.root.join(&root.path)
            };
            let build_server = root_path.join("buildServer.json").exists();
            let compile_commands = root_path.join("compile_commands.json").exists();
            let ready = build_server || compile_commands;
            Some(json!({
                "kind": "xcode",
                "ready": ready,
                "status": if ready { "configured" } else { "missing_config" },
                "buildServerJson": build_server,
                "compileCommandsJson": compile_commands,
                "missing": if ready {
                    Vec::<String>::new()
                } else {
                    vec!["buildServer.json".to_string(), "compile_commands.json".to_string()]
                },
                "message": if ready {
                    "Xcode root has SourceKit-LSP build configuration"
                } else {
                    "Create buildServer.json or compile_commands.json to enable precise SourceKit-LSP facts"
                }
            }))
        }
        _ => None,
    }
}

fn semantic_provider_status(language: &ProjectLanguage) -> Value {
    let requirement = requirement_for_language(language);
    let env_override = std::env::var(requirement.env_key)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let program = provider_program(&requirement);
    let available = program.is_some();
    let mut value = json!({
        "language": language.to_string(),
        "required": true,
        "status": if available { "available" } else { "missing" },
        "available": available,
        "provider": requirement.provider,
        "kind": requirement.kind,
        "program": program,
        "args": requirement.args,
        "envKey": requirement.env_key,
        "defaultCommand": requirement.command,
        "defaultArgs": requirement.args,
        "fallback": requirement.fallback,
        "missingDependencies": if available {
            Vec::<&str>::new()
        } else {
            vec![requirement.provider]
        },
    });
    if let Some(env_override) = env_override {
        value["envOverride"] = Value::String(env_override);
    }
    value
}

fn provider_program(requirement: &ProviderRequirement) -> Option<String> {
    if let Some(override_value) = std::env::var(requirement.env_key)
        .ok()
        .and_then(|value| first_shell_word(&value))
    {
        return resolve_binary(&override_value);
    }
    resolve_binary(requirement.command)
}

fn first_shell_word(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        return rest
            .find('"')
            .map(|end| rest[..end].to_string())
            .filter(|word| !word.is_empty());
    }
    if let Some(rest) = trimmed.strip_prefix('\'') {
        return rest
            .find('\'')
            .map(|end| rest[..end].to_string())
            .filter(|word| !word.is_empty());
    }
    trimmed.split_whitespace().next().map(ToString::to_string)
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

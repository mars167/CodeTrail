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
    provider_help::{env_keys_for_requirement, requirement_for_language, ProviderRequirement},
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

pub(crate) fn summary_status(full: &Value) -> Value {
    let indexed_languages = full
        .get("indexedLanguages")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let file_count = full
        .pointer("/manifest/fileCount")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| indexed_language_file_count(&indexed_languages));
    let semantic_status = full.get("semanticStatus").cloned().unwrap_or_else(|| {
        json!({
            "scipIndex": {},
            "languageCoverage": []
        })
    });

    let scip_index = compact_scip_index(semantic_status.get("scipIndex"));
    let language_coverage = language_coverage(&semantic_status);
    let (query_mode, fallback_reason) = semantic_query_mode(&scip_index, &language_coverage);

    json!({
        "exists": full.get("exists").and_then(Value::as_bool).unwrap_or(false),
        "fresh": full.get("fresh").and_then(Value::as_bool).unwrap_or(false),
        "fileCount": file_count,
        "indexedLanguages": indexed_languages,
        "semanticStatus": {
            "scipIndex": scip_index,
            "queryMode": query_mode,
            "fallbackReason": fallback_reason,
            "languageCoverage": language_coverage,
        }
    })
}

fn indexed_language_file_count(indexed_languages: &Value) -> u64 {
    indexed_languages
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|language| language.get("fileCount").and_then(Value::as_u64))
        .sum()
}

fn compact_scip_index(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return json!({
            "available": false,
            "usable": false,
            "fresh": false,
            "state": "not_generated",
            "languages": []
        });
    };
    json!({
        "available": value.get("available").and_then(Value::as_bool).unwrap_or(false),
        "usable": value.get("usable").and_then(Value::as_bool).unwrap_or(false),
        "fresh": value.get("fresh").and_then(Value::as_bool).unwrap_or(false),
        "state": value.get("state").and_then(Value::as_str).unwrap_or("unknown"),
        "languages": value.get("languages").cloned().unwrap_or_else(|| Value::Array(Vec::new())),
    })
}

fn language_coverage(semantic_status: &Value) -> Value {
    let provider_status = semantic_status
        .get("semanticProviders")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|provider| {
            let language = provider.get("language").and_then(Value::as_str)?;
            Some((language.to_string(), provider.clone()))
        })
        .collect::<BTreeMap<_, _>>();

    let mut coverage = BTreeMap::<String, Value>::new();
    for root in semantic_status
        .get("roots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(language) = root.get("language").and_then(Value::as_str) else {
            continue;
        };
        let provider = root
            .get("provider")
            .and_then(Value::as_str)
            .or_else(|| {
                provider_status
                    .get(language)
                    .and_then(|status| status.get("provider"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("unknown");
        let provider_available = provider_status
            .get(language)
            .and_then(|status| status.get("available"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let state = root
            .get("semanticState")
            .and_then(Value::as_str)
            .unwrap_or("not_generated");
        let partial_reasons = root
            .get("partialReasons")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let entry = coverage.entry(language.to_string()).or_insert_with(|| {
            json!({
                "language": language,
                "provider": provider,
                "precise": precise_coverage_state(state, provider_available),
                "mode": coverage_mode(precise_coverage_state(state, provider_available)),
                "fallback": "tree_sitter_parser",
                "rootCount": 0,
                "partialReasons": []
            })
        });
        entry["rootCount"] = json!(entry.get("rootCount").and_then(Value::as_u64).unwrap_or(0) + 1);
        let current = entry
            .get("precise")
            .and_then(Value::as_str)
            .unwrap_or("missing");
        entry["precise"] = Value::String(merge_precise_coverage(
            current,
            precise_coverage_state(state, provider_available),
        ));
        let precise = entry
            .get("precise")
            .and_then(Value::as_str)
            .unwrap_or("missing");
        entry["mode"] = Value::String(coverage_mode(precise).to_string());
        append_unique_strings(&mut entry["partialReasons"], &partial_reasons);
    }

    Value::Array(coverage.into_values().collect())
}

fn semantic_query_mode(scip_index: &Value, language_coverage: &Value) -> (&'static str, Value) {
    if scip_index
        .get("usable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && scip_index
            .get("fresh")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return ("precise", Value::Null);
    }
    if language_coverage
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        let state = scip_index
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return (
            "parser_fallback",
            Value::String(format!("scip_index_{state}")),
        );
    }
    (
        "source_only",
        Value::String("no_semantic_roots".to_string()),
    )
}

fn coverage_mode(precise: &str) -> &'static str {
    if precise == "fresh" {
        "precise"
    } else {
        "parser_fallback"
    }
}

fn precise_coverage_state(state: &str, provider_available: bool) -> &'static str {
    match state {
        "fresh" => "fresh",
        "partial" => "partial",
        "missing" => "manual_required",
        "not_generated" if !provider_available => "manual_required",
        _ => "missing",
    }
}

fn merge_precise_coverage(left: &str, right: &str) -> String {
    if left == right {
        return left.to_string();
    }
    for state in ["partial", "manual_required", "missing", "fresh"] {
        if left == state || right == state {
            return state.to_string();
        }
    }
    "missing".to_string()
}

fn append_unique_strings(target: &mut Value, extra: &Value) {
    let Some(target_items) = target.as_array_mut() else {
        return;
    };
    for item in extra.as_array().into_iter().flatten() {
        if target_items.iter().any(|existing| existing == item) {
            continue;
        }
        target_items.push(item.clone());
    }
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
    let env_override = env_keys_for_requirement(&requirement)
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(|value| (key, value))
        });
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
    if let Some((key, value_override)) = env_override {
        value["envOverride"] = Value::String(value_override);
        value["envOverrideKey"] = Value::String(key.to_string());
    }
    value
}

fn provider_program(requirement: &ProviderRequirement) -> Option<String> {
    for key in env_keys_for_requirement(requirement) {
        if let Some(override_value) = std::env::var(key)
            .ok()
            .and_then(|value| first_shell_word(&value))
        {
            return resolve_binary(&override_value);
        }
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

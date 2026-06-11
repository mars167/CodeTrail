use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    generation_manifest::{
        hash_config_proof, hash_provider_version, hash_source_proof, mark_fresh, mark_missing,
        mark_partial, new_manifest, GenerationManifest, ProofHashes,
    },
    index,
    output::VerboseLogger,
    project_graph::{
        discover_project_graph, ProjectGraph, ProjectLanguage, ProjectRoot, SemanticFactPolicy,
    },
    scip,
    scip_index::native_db_path,
    semantic_facts::{
        write_scip_index, FactReliability, InternalRange, OccurrenceRole, ProviderProof,
        ProviderRange, RangeEncoding, SemanticOccurrence, SemanticSymbol, SymbolDescriptor,
        SymbolDescriptorKind, SymbolIdentity, SymbolKind, SymbolPackage,
    },
    semantic_provider::{ProviderCapabilities, SemanticProviderVersion},
    workspace::{FileRecord, Workspace},
};

use super::client::{DocumentSymbol, LspClient, LspPosition};
use super::provider::{collect_reference_locations, LSP_PROVIDER_NAME};
use super::registry::{file_path_to_uri, resolve_server, uri_to_relative_path};

const DEFAULT_SEMANTIC_BUDGET_MS: u64 = 60_000;
const MAX_REFERENCE_PROBES: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticLanguageReport {
    pub language: String,
    pub root_id: String,
    pub provider: Option<String>,
    pub state: String,
    pub occurrence_count: usize,
    pub partial_reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticBuildReport {
    pub attempted: bool,
    pub skipped: bool,
    pub skip_reason: Option<String>,
    pub languages: Vec<SemanticLanguageReport>,
}

impl SemanticBuildReport {
    pub fn skipped(reason: &str) -> Self {
        Self {
            attempted: false,
            skipped: true,
            skip_reason: Some(reason.to_string()),
            languages: Vec::new(),
        }
    }
}

pub fn generate_best_effort(
    workspace: &Workspace,
    records: &[FileRecord],
    verbose: VerboseLogger,
) -> Result<SemanticBuildReport> {
    let db_path = native_db_path(workspace);
    if scip::occurrence_db_fresh(&db_path, &workspace.snapshot_id, &workspace.root) {
        verbose.log("semantic: occurrence DB already fresh; skipping LSP phase");
        return Ok(SemanticBuildReport {
            attempted: false,
            skipped: true,
            skip_reason: Some("occurrence_db_fresh".to_string()),
            languages: Vec::new(),
        });
    }

    let budget_ms = semantic_budget_ms();
    let deadline = Instant::now() + Duration::from_millis(budget_ms);
    verbose.log(format!(
        "semantic: starting LSP bridge (budget={budget_ms}ms)"
    ));

    let graph = discover_project_graph(&workspace.root).unwrap_or_else(|_| ProjectGraph {
        schema_version: ProjectGraph::CURRENT_SCHEMA_VERSION,
        roots: Vec::new(),
        source_owners: Vec::new(),
        generated_sources: Vec::new(),
        config_edges: Vec::new(),
        environment_edges: Vec::new(),
        dependency_edges: Vec::new(),
        caveats: Vec::new(),
    });

    let file_contents = load_file_contents(workspace, records);
    let mut all_occurrences = Vec::new();
    let mut language_reports = Vec::new();
    let mut manifests = Vec::new();

    for root in &graph.roots {
        if Instant::now() >= deadline {
            verbose.log("semantic: wall-clock budget exhausted");
            break;
        }
        let files = source_files_for_root(&graph, root);
        if files.is_empty() {
            continue;
        }
        let report = index_root(
            workspace,
            root,
            &files,
            &file_contents,
            &mut all_occurrences,
            deadline,
            verbose,
        );
        language_reports.push(report.clone());
        manifests.push(build_manifest(workspace, root, &report, &files, records));
    }

    if all_occurrences.is_empty() {
        write_generation_manifests(workspace, &manifests)?;
        return Ok(SemanticBuildReport {
            attempted: true,
            skipped: false,
            skip_reason: None,
            languages: language_reports,
        });
    }

    let scip_index = write_scip_index(&all_occurrences, &workspace.root.to_string_lossy())?;
    fs::create_dir_all(index::scip_root(workspace))?;
    scip::build_occurrences_db(
        &scip_index,
        &db_path,
        &workspace.snapshot_id,
        &workspace.root,
    )
    .with_context(|| "failed to build occurrence database from LSP facts")?;

    write_generation_manifests(workspace, &manifests)?;
    verbose.log(format!(
        "semantic: wrote {} occurrences to {}",
        all_occurrences.len(),
        db_path.display()
    ));

    Ok(SemanticBuildReport {
        attempted: true,
        skipped: false,
        skip_reason: None,
        languages: language_reports,
    })
}

fn semantic_budget_ms() -> u64 {
    std::env::var("CODETRAIL_SEMANTIC_BUDGET_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEMANTIC_BUDGET_MS)
}

fn load_file_contents(workspace: &Workspace, records: &[FileRecord]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for record in records {
        let path = workspace.abs_path(&record.path);
        if let Ok(content) = fs::read_to_string(&path) {
            map.insert(record.path.clone(), content);
        }
    }
    map
}

fn source_files_for_root(graph: &ProjectGraph, root: &ProjectRoot) -> Vec<String> {
    graph
        .source_owners
        .iter()
        .filter(|owner| {
            owner.root_id == root.id
                && owner.semantic_fact_policy == SemanticFactPolicy::PreciseEligible
        })
        .map(|owner| owner.path.clone())
        .collect()
}

fn index_root(
    workspace: &Workspace,
    root: &ProjectRoot,
    files: &[String],
    file_contents: &BTreeMap<String, String>,
    occurrences: &mut Vec<SemanticOccurrence>,
    deadline: Instant,
    verbose: VerboseLogger,
) -> SemanticLanguageReport {
    let language = root.language.clone();
    let Some(spec) = resolve_server(&language) else {
        return SemanticLanguageReport {
            language: language.to_string(),
            root_id: root.id.clone(),
            provider: None,
            state: "missing".to_string(),
            occurrence_count: 0,
            partial_reasons: vec!["semantic_provider_missing".to_string()],
        };
    };

    let root_path = workspace.root.join(&root.path);
    let mut client = match LspClient::spawn(&spec, &root_path) {
        Ok(client) => client,
        Err(error) => {
            return SemanticLanguageReport {
                language: language.to_string(),
                root_id: root.id.clone(),
                provider: Some(spec.provider_id.clone()),
                state: "partial".to_string(),
                occurrence_count: 0,
                partial_reasons: vec![format!("semantic_provider_startup_failed: {error}")],
            };
        }
    };

    let root_uri = match file_path_to_uri(&root_path) {
        Ok(uri) => uri,
        Err(error) => {
            return SemanticLanguageReport {
                language: language.to_string(),
                root_id: root.id.clone(),
                provider: Some(spec.provider_id.clone()),
                state: "partial".to_string(),
                occurrence_count: 0,
                partial_reasons: vec![format!("semantic_provider_startup_failed: {error}")],
            };
        }
    };

    if let Err(error) = client.initialize(&root_uri, &spec.readiness) {
        let _ = client.shutdown();
        return SemanticLanguageReport {
            language: language.to_string(),
            root_id: root.id.clone(),
            provider: Some(spec.provider_id.clone()),
            state: "partial".to_string(),
            occurrence_count: 0,
            partial_reasons: vec![format!("semantic_provider_startup_failed: {error}")],
        };
    }

    verbose.log(format!(
        "semantic: indexing root {} ({}) via {}",
        root.id, language, spec.provider_id
    ));

    let language_id = lsp_language_id(&language);
    let provider_version = provider_version_from_client(&client);
    let package = package_for_root(root);
    let ctx = OccurrenceBuildCtx {
        workspace,
        root,
        package: &package,
        provider_version: &provider_version,
        provider_id: &spec.provider_id,
        encoding: client.position_encoding(),
    };
    let mut root_occurrences = Vec::new();
    let mut partial_reasons = Vec::new();

    for path in files {
        if Instant::now() >= deadline {
            partial_reasons.push("semantic_provider_partial: wall_clock_budget".to_string());
            break;
        }
        let Some(lsp_path) = lsp_relative_path(root, path) else {
            partial_reasons.push(format!(
                "semantic_provider_partial: path_outside_root:{path}"
            ));
            continue;
        };
        let content = file_contents.get(path).cloned().unwrap_or_default();
        if client.did_open(&lsp_path, language_id, &content).is_err() {
            continue;
        }
        let symbols = match client.document_symbol(&lsp_path) {
            Ok(symbols) => symbols,
            Err(error) => {
                partial_reasons.push(format!("semantic_provider_partial: {error}"));
                continue;
            }
        };
        flatten_symbol_occurrences(&ctx, path, &symbols, &content, &mut root_occurrences);
    }

    let mut reference_budget = MAX_REFERENCE_PROBES;
    if Instant::now() < deadline && reference_budget > 0 {
        let probes = unique_probe_positions_from_occurrences(&root_occurrences, reference_budget);
        for probe in probes {
            if Instant::now() >= deadline || reference_budget == 0 {
                partial_reasons.push("semantic_provider_partial: reference_budget".to_string());
                break;
            }
            let Some(lsp_path) = lsp_relative_path(root, &probe.path) else {
                partial_reasons.push(format!(
                    "semantic_provider_partial: path_outside_root:{}",
                    probe.path
                ));
                continue;
            };
            let locations = collect_reference_locations(&client, &lsp_path, &probe.position, 32);
            for location in locations {
                let Some(path) = uri_to_relative_path(&workspace.root, &location.uri) else {
                    continue;
                };
                let Some(content) = file_contents.get(&path) else {
                    continue;
                };
                if let Some(occurrence) = reference_occurrence_from_lsp(
                    &ctx,
                    &path,
                    &location.range,
                    content,
                    &probe.symbol,
                ) {
                    root_occurrences.push(occurrence);
                }
            }
            reference_budget = reference_budget.saturating_sub(1);
        }
    }

    let _ = client.shutdown();
    let count = root_occurrences.len();
    occurrences.append(&mut root_occurrences);

    SemanticLanguageReport {
        language: language.to_string(),
        root_id: root.id.clone(),
        provider: Some(spec.provider_id.clone()),
        state: if partial_reasons.is_empty() {
            "fresh".to_string()
        } else {
            "partial".to_string()
        },
        occurrence_count: count,
        partial_reasons,
    }
}

fn provider_version_from_client(client: &LspClient) -> SemanticProviderVersion {
    if let Some(info) = client.server_info() {
        SemanticProviderVersion {
            name: info.name.clone(),
            version: info.version.clone(),
            protocol_version: 1,
        }
    } else {
        SemanticProviderVersion {
            name: LSP_PROVIDER_NAME.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: 1,
        }
    }
}

fn package_for_root(root: &ProjectRoot) -> SymbolPackage {
    SymbolPackage {
        manager: format!("{:?}", root.kind).to_ascii_lowercase(),
        name: root.path.clone(),
        version: "0.0.0".to_string(),
    }
}

fn lsp_language_id(language: &ProjectLanguage) -> &'static str {
    match language {
        ProjectLanguage::Go => "go",
        ProjectLanguage::Rust => "rust",
        ProjectLanguage::Java => "java",
        ProjectLanguage::TypeScript => "typescript",
    }
}

fn lsp_relative_path(root: &ProjectRoot, path: &str) -> Option<String> {
    if root.path == "." {
        return Some(path.to_string());
    }
    path.strip_prefix(&root.path)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(ToString::to_string)
}

struct OccurrenceBuildCtx<'a> {
    workspace: &'a Workspace,
    root: &'a ProjectRoot,
    package: &'a SymbolPackage,
    provider_version: &'a SemanticProviderVersion,
    provider_id: &'a str,
    encoding: &'a str,
}

struct ReferenceProbe {
    path: String,
    position: LspPosition,
    symbol: SemanticSymbol,
}

fn flatten_symbol_occurrences(
    ctx: &OccurrenceBuildCtx<'_>,
    path: &str,
    symbols: &[DocumentSymbol],
    content: &str,
    out: &mut Vec<SemanticOccurrence>,
) {
    for symbol in symbols {
        if let Some(occurrence) = definition_occurrence_from_lsp(ctx, path, symbol, content) {
            out.push(occurrence);
        }
        flatten_symbol_occurrences(ctx, path, &symbol.children, content, out);
    }
}

fn definition_occurrence_from_lsp(
    ctx: &OccurrenceBuildCtx<'_>,
    path: &str,
    symbol: &DocumentSymbol,
    content: &str,
) -> Option<SemanticOccurrence> {
    let range = lsp_range_to_internal(
        &symbol.selection_range.start,
        &symbol.selection_range.end,
        ctx.encoding,
        content,
    )
    .ok()?;
    Some(SemanticOccurrence {
        file_path: path.to_string(),
        range,
        role: OccurrenceRole::Definition,
        symbol: semantic_symbol_from_lsp(ctx.root, symbol, ctx.package, ctx.provider_version),
        proof: ProviderProof {
            provider_id: ctx.provider_id.to_string(),
            provider_version: ctx.provider_version.clone(),
            reliability: FactReliability::ProviderConfirmed,
            evidence: format!("lsp:documentSymbol:{}", ctx.workspace.snapshot_id),
        },
    })
}

fn reference_occurrence_from_lsp(
    ctx: &OccurrenceBuildCtx<'_>,
    path: &str,
    range: &super::client::LspRange,
    content: &str,
    symbol: &SemanticSymbol,
) -> Option<SemanticOccurrence> {
    let internal = lsp_range_to_internal(&range.start, &range.end, ctx.encoding, content).ok()?;
    Some(SemanticOccurrence {
        file_path: path.to_string(),
        range: internal,
        role: OccurrenceRole::Reference,
        symbol: symbol.clone(),
        proof: ProviderProof {
            provider_id: ctx.provider_id.to_string(),
            provider_version: ctx.provider_version.clone(),
            reliability: FactReliability::ProviderConfirmed,
            evidence: format!("lsp:references:{}", ctx.workspace.snapshot_id),
        },
    })
}

fn semantic_symbol_from_lsp(
    root: &ProjectRoot,
    symbol: &DocumentSymbol,
    package: &SymbolPackage,
    provider_version: &SemanticProviderVersion,
) -> SemanticSymbol {
    let kind = lsp_kind_to_symbol_kind(symbol.kind);
    SemanticSymbol {
        identity: SymbolIdentity {
            language: root.language.clone(),
            project_id: root.id.clone(),
            package: package.clone(),
            descriptors: vec![SymbolDescriptor {
                name: symbol.name.clone(),
                kind: SymbolDescriptorKind::from_symbol_kind(&kind),
            }],
            signature: None,
            disambiguator: None,
            provider_version: provider_version.clone(),
            generated: false,
            local_id: None,
        },
        kind,
        display_name: symbol.name.clone(),
        documentation: Vec::new(),
    }
}

fn lsp_kind_to_symbol_kind(kind: u32) -> SymbolKind {
    match kind {
        5 => SymbolKind::Class,
        6 => SymbolKind::Method,
        10 => SymbolKind::Enum,
        11 => SymbolKind::Interface,
        12 => SymbolKind::Function,
        23 => SymbolKind::Struct,
        13 => SymbolKind::Variable,
        22 => SymbolKind::Constant,
        4 => SymbolKind::Module,
        _ => SymbolKind::Unknown,
    }
}

fn lsp_range_to_internal(
    start: &LspPosition,
    end: &LspPosition,
    encoding: &str,
    content: &str,
) -> Result<InternalRange> {
    let range_encoding = if encoding == "utf-8" {
        RangeEncoding::Utf8ByteOffset
    } else {
        RangeEncoding::LspUtf16
    };
    ProviderRange {
        start_line: start.line,
        start_character: start.character,
        end_line: end.line,
        end_character: end.character,
        encoding: range_encoding,
    }
    .to_internal_range(content)
}

fn unique_probe_positions_from_occurrences(
    occurrences: &[SemanticOccurrence],
    limit: usize,
) -> Vec<ReferenceProbe> {
    let mut seen = BTreeSet::new();
    let mut probes = Vec::new();
    for occurrence in occurrences {
        if occurrence.role != OccurrenceRole::Definition {
            continue;
        }
        let key = format!(
            "{}:{}:{}",
            occurrence.file_path, occurrence.range.start_line, occurrence.range.start_column
        );
        if !seen.insert(key) {
            continue;
        }
        probes.push(ReferenceProbe {
            path: occurrence.file_path.clone(),
            position: LspPosition {
                line: occurrence.range.start_line,
                character: occurrence.range.start_column,
            },
            symbol: occurrence.symbol.clone(),
        });
        if probes.len() >= limit {
            break;
        }
    }
    probes
}

fn build_manifest(
    workspace: &Workspace,
    root: &ProjectRoot,
    report: &SemanticLanguageReport,
    files: &[String],
    records: &[FileRecord],
) -> GenerationManifest {
    let caps = ProviderCapabilities {
        language: root.language.clone(),
        provider_version: SemanticProviderVersion {
            name: report
                .provider
                .clone()
                .unwrap_or_else(|| LSP_PROVIDER_NAME.to_string()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: 1,
        },
        supports_batch_resolve: true,
        supports_import_graph: false,
        supports_workspace_symbols: true,
        max_batch_size: MAX_REFERENCE_PROBES,
        partial_reasons: Vec::new(),
    };
    let file_set: BTreeSet<_> = files.iter().cloned().collect();
    let proofs: Vec<(String, String)> = records
        .iter()
        .filter(|record| file_set.contains(&record.path))
        .map(|record| {
            (
                record.path.clone(),
                record
                    .hash
                    .strip_prefix("blake3:")
                    .unwrap_or(&record.hash)
                    .to_string(),
            )
        })
        .collect();
    let hashes = ProofHashes {
        provider_version_hash: hash_provider_version(&caps),
        environment_hash: environment_hash(report),
        source_proof_hash: hash_source_proof(&proofs),
        config_proof_hash: hash_config_proof(&[]),
    };
    let mut manifest = new_manifest(
        root,
        report.provider.as_deref().unwrap_or(LSP_PROVIDER_NAME),
        &hashes,
    );
    match report.state.as_str() {
        "fresh" => mark_fresh(&mut manifest, &hashes),
        "missing" => mark_missing(&mut manifest),
        _ => mark_partial(&mut manifest, report.partial_reasons.clone()),
    }
    let _ = workspace;
    manifest
}

fn environment_hash(report: &SemanticLanguageReport) -> String {
    let payload = format!(
        "{}:{}",
        report.provider.as_deref().unwrap_or("missing"),
        report.language
    );
    blake3::hash(payload.as_bytes()).to_hex().to_string()
}

pub fn generation_manifest_path(workspace: &Workspace) -> std::path::PathBuf {
    index::scip_root(workspace).join("generation.json")
}

pub fn read_generation_manifests(workspace: &Workspace) -> Result<Vec<GenerationManifest>> {
    let path = generation_manifest_path(workspace);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read(&path)?;
    Ok(serde_json::from_slice(&data)?)
}

fn write_generation_manifests(
    workspace: &Workspace,
    manifests: &[GenerationManifest],
) -> Result<()> {
    if manifests.is_empty() {
        return Ok(());
    }
    let path = generation_manifest_path(workspace);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(manifests)?)?;
    Ok(())
}

pub fn semantic_summary_json(report: &SemanticBuildReport) -> Value {
    json!({
        "attempted": report.attempted,
        "skipped": report.skipped,
        "skipReason": report.skip_reason,
        "languages": report.languages.iter().map(|lang| json!({
            "language": lang.language,
            "rootId": lang.root_id,
            "provider": lang.provider,
            "state": lang.state,
            "occurrenceCount": lang.occurrence_count,
            "partialReasons": lang.partial_reasons,
        })).collect::<Vec<_>>(),
    })
}

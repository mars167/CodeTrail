use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    generation_manifest::{
        hash_config_proof, hash_provider_version, hash_source_proof, mark_fresh, mark_missing,
        mark_partial, new_manifest, GenerationManifest, ManifestState, ProofHashes,
    },
    index,
    output::VerboseLogger,
    project_graph::{
        discover_project_graph, ProjectGraph, ProjectLanguage, ProjectRoot, ProjectRootKind,
        SemanticFactPolicy,
    },
    scip,
    scip_index::native_db_path,
    scip_proto::proto,
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
use super::registry::{file_path_to_uri, resolve_server, uri_to_relative_path, ServerSpec};

const DEFAULT_SEMANTIC_BUDGET_MS: u64 = 60_000;
const MAX_REFERENCE_PROBES: usize = 200;
const MAX_HIGH_FANOUT_REFERENCE_PROBES: usize = 5_000;

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
    pub scip: Option<SemanticScipReport>,
    pub languages: Vec<SemanticLanguageReport>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticScipReport {
    pub generated: bool,
    pub imported: bool,
    pub source: String,
    pub path: Option<String>,
    pub document_count: usize,
    pub occurrence_count: usize,
    pub symbol_count: usize,
}

impl SemanticBuildReport {
    pub fn skipped(reason: &str) -> Self {
        Self {
            attempted: false,
            skipped: true,
            skip_reason: Some(reason.to_string()),
            scip: None,
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
        match fresh_occurrence_db_skip_reason(workspace) {
            Ok(Some(reason)) => {
                verbose.log("semantic: occurrence DB already fresh; skipping provider phase");
                return Ok(SemanticBuildReport {
                    attempted: false,
                    skipped: true,
                    skip_reason: Some(reason),
                    scip: None,
                    languages: Vec::new(),
                });
            }
            Ok(None) => verbose.log(
                "semantic: occurrence DB is fresh but generation manifest is not fresh; rerunning provider phase",
            ),
            Err(error) => verbose.log(format!(
                "semantic: occurrence DB is fresh but generation manifest could not be read ({error}); rerunning provider phase"
            )),
        }
    }

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

    let budget_ms = semantic_budget_ms(&graph);
    let deadline = Instant::now() + Duration::from_millis(budget_ms);
    verbose.log(format!(
        "semantic: starting semantic providers (budget={budget_ms}ms)"
    ));

    let file_contents = load_file_contents(workspace, records);
    let mut all_occurrences = Vec::new();
    let mut language_reports = Vec::new();
    let mut manifests = Vec::new();
    let mut native_indexes = Vec::new();

    for (key, roots) in native_work_groups(workspace, &graph) {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            for (root, files) in roots {
                let requirement = crate::provider_help::requirement_for_language(&root.language);
                let report = semantic_report(
                    root,
                    Some(requirement.provider),
                    "partial",
                    0,
                    vec!["semantic_provider_failed: provider_timeout".to_string()],
                );
                language_reports.push(report.clone());
                manifests.push(build_manifest(workspace, root, &report, &files, records));
            }
            continue;
        };
        let run_root = roots
            .iter()
            .find(|(root, _)| root.language == ProjectLanguage::Kotlin)
            .map(|(root, _)| *root)
            .unwrap_or(roots[0].0);
        let run = crate::scip_provider::NativeProviderRun {
            build_root: key.build_root.clone(),
            output_stem: key.output_stem.clone(),
            build_tool: key.build_tool.clone(),
        };
        match crate::scip_provider::run_native_provider(
            workspace, run_root, &run, verbose, remaining,
        )? {
            crate::scip_provider::NativeScipOutcome::Generated {
                provider, index, ..
            } => {
                for (root, files) in &roots {
                    let occurrence_count = count_scip_occurrences_for_root(&index, root, files);
                    let (state, partial_reasons) = native_report_state(root, occurrence_count);
                    let report = semantic_report(
                        root,
                        Some(provider),
                        state,
                        occurrence_count,
                        partial_reasons,
                    );
                    language_reports.push(report.clone());
                    manifests.push(build_manifest(workspace, root, &report, files, records));
                }
                native_indexes.push(index);
            }
            crate::scip_provider::NativeScipOutcome::Missing { requirement } => {
                for (root, files) in roots {
                    let root_requirement =
                        crate::provider_help::requirement_for_language(&root.language);
                    let provider = if root_requirement.provider == requirement.provider {
                        requirement.provider
                    } else {
                        root_requirement.provider
                    };
                    let report = semantic_report(
                        root,
                        Some(provider),
                        "missing",
                        0,
                        vec!["semantic_provider_missing".to_string()],
                    );
                    language_reports.push(report.clone());
                    manifests.push(build_manifest(workspace, root, &report, &files, records));
                }
            }
            crate::scip_provider::NativeScipOutcome::Failed { provider, message } => {
                for (root, files) in roots {
                    let report = semantic_report(
                        root,
                        Some(provider),
                        "partial",
                        0,
                        vec![format!("semantic_provider_failed: {message}")],
                    );
                    language_reports.push(report.clone());
                    manifests.push(build_manifest(workspace, root, &report, &files, records));
                }
            }
            crate::scip_provider::NativeScipOutcome::NotNative => {}
        }
    }

    for (root, files, report) in skipped_lsp_roots(workspace, &graph, None) {
        language_reports.push(report.clone());
        manifests.push(build_manifest(workspace, root, &report, &files, records));
    }

    let groups = lsp_work_groups(workspace, &graph)
        .into_iter()
        .filter(|((language, _), _)| {
            crate::provider_help::requirement_for_language(language).kind
                == crate::provider_help::ProviderKind::LspBridge
        });
    for ((language, lsp_root_path), roots) in groups {
        if Instant::now() >= deadline {
            verbose.log("semantic: wall-clock budget exhausted");
            break;
        }

        let reports = index_lsp_group(
            LspGroupRequest {
                workspace,
                language,
                lsp_root_path: &lsp_root_path,
                roots: &roots,
                file_contents: &file_contents,
                deadline,
                verbose,
            },
            &mut all_occurrences,
        );
        for (root, files, report) in reports {
            language_reports.push(report.clone());
            manifests.push(build_manifest(workspace, root, &report, files, records));
        }
    }

    let native_index_count = native_indexes.len();
    let lsp_generated = !all_occurrences.is_empty();
    let mut indexes = native_indexes;
    if lsp_generated {
        indexes.push(write_scip_index(
            &all_occurrences,
            &workspace.root.to_string_lossy(),
        )?);
    }

    if indexes.is_empty() {
        scip::invalidate_db(&db_path)
            .with_context(|| "failed to invalidate empty occurrence database")?;
        write_generation_manifests(workspace, &manifests)?;
        return Ok(SemanticBuildReport {
            attempted: true,
            skipped: false,
            skip_reason: None,
            scip: Some(SemanticScipReport {
                generated: false,
                imported: false,
                source: scip_report_source(native_index_count, lsp_generated),
                path: Some(db_path.to_string_lossy().to_string()),
                document_count: 0,
                occurrence_count: 0,
                symbol_count: 0,
            }),
            languages: language_reports,
        });
    }

    let scip_index = crate::scip_provider::merge_native_indexes(indexes);
    let scip_report = scip_build_report(
        &scip_index,
        &db_path,
        scip_report_source(native_index_count, lsp_generated),
    );
    fs::create_dir_all(index::scip_root(workspace))?;
    scip::build_occurrences_db(
        &scip_index,
        &db_path,
        &workspace.snapshot_id,
        &workspace.root,
    )
    .with_context(|| "failed to build occurrence database from semantic provider facts")?;

    write_generation_manifests(workspace, &manifests)?;
    verbose.log(format!(
        "semantic: wrote {} occurrences to {}",
        scip_report.occurrence_count,
        db_path.display()
    ));

    Ok(SemanticBuildReport {
        attempted: true,
        skipped: false,
        skip_reason: None,
        scip: Some(scip_report),
        languages: language_reports,
    })
}

fn scip_build_report(index: &proto::Index, db_path: &Path, source: String) -> SemanticScipReport {
    SemanticScipReport {
        generated: true,
        imported: true,
        source,
        path: Some(db_path.to_string_lossy().to_string()),
        document_count: index.documents.len(),
        occurrence_count: index
            .documents
            .iter()
            .map(|document| document.occurrences.len())
            .sum(),
        symbol_count: index
            .documents
            .iter()
            .map(|document| document.symbols.len())
            .sum(),
    }
}

fn scip_report_source(native_index_count: usize, lsp_generated: bool) -> String {
    match (native_index_count > 0, lsp_generated) {
        (true, true) => "mixed_scip".to_string(),
        (true, false) => "native_scip".to_string(),
        (false, true) => "lsp_scip".to_string(),
        (false, false) => "semantic_scip".to_string(),
    }
}

fn semantic_budget_ms(graph: &ProjectGraph) -> u64 {
    if let Some(value) = std::env::var("CODETRAIL_SEMANTIC_BUDGET_MS")
        .ok()
        .and_then(|value| value.parse().ok())
    {
        return value;
    }

    adaptive_semantic_budget_ms(graph)
}

fn adaptive_semantic_budget_ms(graph: &ProjectGraph) -> u64 {
    let jvm_root_count = graph
        .roots
        .iter()
        .filter(|root| {
            matches!(
                root.language,
                ProjectLanguage::Java | ProjectLanguage::Kotlin
            )
        })
        .count() as u64;
    if jvm_root_count == 0 {
        return DEFAULT_SEMANTIC_BUDGET_MS;
    }

    let jvm_file_count = graph
        .source_owners
        .iter()
        .filter(|owner| {
            matches!(
                owner.language,
                ProjectLanguage::Java | ProjectLanguage::Kotlin
            ) && owner.semantic_fact_policy == SemanticFactPolicy::PreciseEligible
        })
        .count() as u64;

    (180_000 + (jvm_root_count * 60_000) + (jvm_file_count * 1_000))
        .clamp(DEFAULT_SEMANTIC_BUDGET_MS, 3_600_000)
}

fn fresh_occurrence_db_skip_reason(workspace: &Workspace) -> Result<Option<String>> {
    let manifests = read_generation_manifests(workspace)?;
    if generation_manifests_allow_occurrence_skip(&manifests) {
        Ok(Some("occurrence_db_fresh".to_string()))
    } else {
        Ok(None)
    }
}

fn generation_manifests_allow_occurrence_skip(manifests: &[GenerationManifest]) -> bool {
    manifests.is_empty()
        || manifests.iter().all(|manifest| {
            manifest.state == ManifestState::Fresh && manifest.partial_reasons.is_empty()
        })
}

pub fn generation_manifests_allow_precise_use(workspace: &Workspace) -> Result<bool> {
    let manifests = read_generation_manifests(workspace)?;
    Ok(manifests.is_empty()
        || manifests
            .iter()
            .all(|manifest| !manifest.state.blocks_precise()))
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct NativeGroupKey {
    provider: String,
    build_root: String,
    build_tool: Option<crate::scip_provider::NativeBuildTool>,
    output_stem: String,
    command: String,
    args: Vec<String>,
}

type NativeWorkGroups<'a> = BTreeMap<NativeGroupKey, Vec<(&'a ProjectRoot, Vec<String>)>>;

fn native_work_groups<'a>(workspace: &Workspace, graph: &'a ProjectGraph) -> NativeWorkGroups<'a> {
    let mut groups = BTreeMap::new();
    for root in &graph.roots {
        let requirement = crate::provider_help::requirement_for_language(&root.language);
        if requirement.kind != crate::provider_help::ProviderKind::NativeScip {
            continue;
        }
        let files = source_files_for_root(graph, root);
        if files.is_empty() {
            continue;
        }
        let (build_root, build_tool) = native_build_unit_for_root(workspace, root);
        let output_stem = native_output_stem(requirement.provider, &build_root, root);
        let key = NativeGroupKey {
            provider: requirement.provider.to_string(),
            build_root,
            build_tool,
            output_stem,
            command: requirement.command.to_string(),
            args: requirement
                .args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect(),
        };
        groups
            .entry(key)
            .or_insert_with(Vec::new)
            .push((root, files));
    }
    groups
}

fn native_build_unit_for_root(
    workspace: &Workspace,
    root: &ProjectRoot,
) -> (String, Option<crate::scip_provider::NativeBuildTool>) {
    use crate::scip_provider::NativeBuildTool;

    match root.kind {
        ProjectRootKind::JavaMaven => (
            maven_build_root(workspace, &root.path),
            Some(NativeBuildTool::Maven),
        ),
        ProjectRootKind::JavaGradle | ProjectRootKind::KotlinGradle => (
            gradle_build_root(workspace, &root.path),
            Some(NativeBuildTool::Gradle),
        ),
        _ => (root.path.clone(), None),
    }
}

fn native_output_stem(provider: &str, build_root: &str, root: &ProjectRoot) -> String {
    if provider == "scip-java" {
        let suffix = if build_root == "." {
            "root"
        } else {
            build_root
        };
        format!("java-{suffix}")
    } else {
        root.id.clone()
    }
}

fn maven_build_root(workspace: &Workspace, root_path: &str) -> String {
    for candidate in ancestor_roots(root_path) {
        let pom_path = workspace
            .root
            .join(rel_path_to_fs_path(&candidate))
            .join("pom.xml");
        if !pom_path.exists() {
            continue;
        }
        if candidate == root_path {
            return candidate;
        }
        if maven_modules_cover_root(&pom_path, &candidate, root_path) {
            return candidate;
        }
    }
    root_path.to_string()
}

fn maven_modules_cover_root(pom_path: &Path, pom_root: &str, root_path: &str) -> bool {
    let modules = read_maven_modules(pom_path);
    modules.iter().any(|module| {
        let module_path = join_rel_path(pom_root, module);
        root_path == module_path || root_path.starts_with(&format!("{module_path}/"))
    })
}

fn read_maven_modules(pom_path: &Path) -> Vec<String> {
    let Ok(source) = fs::read_to_string(pom_path) else {
        return Vec::new();
    };
    let mut reader = Reader::from_str(&source);
    reader.config_mut().trim_text(true);
    reader.config_mut().check_end_names = true;
    let mut buf = Vec::new();
    let mut in_modules = false;
    let mut in_module = false;
    let mut modules = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(element)) => {
                if tag_name_is(&element, b"modules") {
                    in_modules = true;
                } else if in_modules && tag_name_is(&element, b"module") {
                    in_module = true;
                }
            }
            Ok(Event::End(element)) => {
                let name = element.local_name();
                if name.as_ref() == b"module" {
                    in_module = false;
                } else if name.as_ref() == b"modules" {
                    in_modules = false;
                }
            }
            Ok(Event::Text(text)) if in_modules && in_module => {
                if let Ok(value) = text.decode() {
                    let value = normalize_rel_path(value.trim());
                    if !value.is_empty() {
                        modules.push(value);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Vec::new(),
        }
        buf.clear();
    }
    modules
}

fn tag_name_is(element: &BytesStart<'_>, expected: &[u8]) -> bool {
    element.local_name().as_ref() == expected
}

fn gradle_build_root(workspace: &Workspace, root_path: &str) -> String {
    ancestor_roots(root_path)
        .into_iter()
        .filter(|candidate| {
            let dir = workspace.root.join(rel_path_to_fs_path(candidate));
            dir.join("settings.gradle").exists() || dir.join("settings.gradle.kts").exists()
        })
        .next()
        .unwrap_or_else(|| root_path.to_string())
}

fn ancestor_roots(root_path: &str) -> Vec<String> {
    if root_path == "." || root_path.is_empty() {
        return vec![".".to_string()];
    }
    let mut roots = vec![".".to_string()];
    let parts = root_path.split('/').collect::<Vec<_>>();
    for index in 1..=parts.len() {
        roots.push(parts[..index].join("/"));
    }
    roots
}

fn join_rel_path(parent: &str, child: &str) -> String {
    let child = normalize_rel_path(child);
    if parent == "." || parent.is_empty() {
        child
    } else if child.is_empty() {
        parent.to_string()
    } else {
        normalize_rel_path(&format!("{parent}/{child}"))
    }
}

fn normalize_rel_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value),
        }
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn rel_path_to_fs_path(path: &str) -> PathBuf {
    if path == "." {
        PathBuf::new()
    } else {
        path.split('/').collect()
    }
}

fn count_scip_occurrences_for_root(
    index: &proto::Index,
    root: &ProjectRoot,
    files: &[String],
) -> usize {
    let file_set = files.iter().collect::<BTreeSet<_>>();
    index
        .documents
        .iter()
        .filter(|document| file_set.contains(&document.relative_path))
        .filter(|document| scip_document_matches_root_language(document, root))
        .map(|document| document.occurrences.len())
        .sum()
}

fn scip_document_matches_root_language(document: &proto::Document, root: &ProjectRoot) -> bool {
    let expected = root.language.to_string();
    if document.language == expected {
        return true;
    }
    document.language.is_empty()
        && crate::workspace::language_for_path(Path::new(&document.relative_path)) == expected
}

fn native_report_state(root: &ProjectRoot, occurrence_count: usize) -> (&'static str, Vec<String>) {
    if root.language == ProjectLanguage::Kotlin && occurrence_count == 0 {
        (
            "partial",
            vec!["semantic_provider_partial: kotlin_no_occurrences".to_string()],
        )
    } else {
        ("fresh", Vec::new())
    }
}

type LspWorkGroups<'a> = BTreeMap<(ProjectLanguage, PathBuf), Vec<(&'a ProjectRoot, Vec<String>)>>;

struct LspGroupRequest<'a> {
    workspace: &'a Workspace,
    language: ProjectLanguage,
    lsp_root_path: &'a Path,
    roots: &'a [(&'a ProjectRoot, Vec<String>)],
    file_contents: &'a BTreeMap<String, String>,
    deadline: Instant,
    verbose: VerboseLogger,
}

struct RootIndexRequest<'a> {
    workspace: &'a Workspace,
    root: &'a ProjectRoot,
    files: &'a [String],
    file_contents: &'a BTreeMap<String, String>,
    deadline: Instant,
    client: &'a LspClient,
    provider_id: &'a str,
    provider_version: &'a SemanticProviderVersion,
    lsp_root_path: &'a Path,
    language_id: &'a str,
}

fn lsp_work_groups<'a>(workspace: &Workspace, graph: &'a ProjectGraph) -> LspWorkGroups<'a> {
    let mut groups = BTreeMap::new();
    for root in &graph.roots {
        if !lsp_root_ready(workspace, root) {
            continue;
        }
        let files = source_files_for_root(graph, root);
        if files.is_empty() {
            continue;
        }
        groups
            .entry((root.language.clone(), lsp_workspace_root(workspace, root)))
            .or_insert_with(Vec::new)
            .push((root, files));
    }
    groups
}

fn skipped_lsp_roots<'a>(
    workspace: &Workspace,
    graph: &'a ProjectGraph,
    language_filter: Option<&ProjectLanguage>,
) -> Vec<(&'a ProjectRoot, Vec<String>, SemanticLanguageReport)> {
    graph
        .roots
        .iter()
        .filter(|root| language_filter.is_none_or(|language| &root.language == language))
        .filter(|root| {
            crate::provider_help::requirement_for_language(&root.language).kind
                == crate::provider_help::ProviderKind::LspBridge
        })
        .filter(|root| !lsp_root_ready(workspace, root))
        .map(|root| {
            (
                root,
                source_files_for_root(graph, root),
                semantic_report(
                    root,
                    Some("sourcekit-lsp"),
                    "partial",
                    0,
                    vec![
                        "semantic_provider_missing_config: buildServer.json_or_compile_commands.json"
                            .to_string(),
                    ],
                ),
            )
        })
        .collect()
}

fn lsp_root_ready(workspace: &Workspace, root: &ProjectRoot) -> bool {
    match root.kind {
        ProjectRootKind::SwiftXcodeProject | ProjectRootKind::SwiftXcodeWorkspace => {
            let root_path = if root.path == "." {
                workspace.root.clone()
            } else {
                workspace.root.join(&root.path)
            };
            root_path.join("buildServer.json").exists()
                || root_path.join("compile_commands.json").exists()
        }
        _ => true,
    }
}

fn lsp_workspace_root(workspace: &Workspace, root: &ProjectRoot) -> PathBuf {
    if root.path == "." {
        workspace.root.clone()
    } else {
        workspace.root.join(&root.path)
    }
}

fn index_lsp_group<'a>(
    request: LspGroupRequest<'a>,
    occurrences: &mut Vec<SemanticOccurrence>,
) -> Vec<(&'a ProjectRoot, &'a [String], SemanticLanguageReport)> {
    let LspGroupRequest {
        workspace,
        language,
        lsp_root_path,
        roots,
        file_contents,
        deadline,
        verbose,
    } = request;

    let Some(spec) = resolve_server(&language) else {
        return roots
            .iter()
            .map(|(root, files)| {
                (
                    *root,
                    files.as_slice(),
                    semantic_report(
                        root,
                        None,
                        "missing",
                        0,
                        vec!["semantic_provider_missing".to_string()],
                    ),
                )
            })
            .collect();
    };

    verbose.log(format!(
        "semantic: starting LSP group language={} workspace={} roots={}",
        language,
        lsp_root_path.display(),
        roots.len()
    ));

    let mut client = match LspClient::spawn(&spec, lsp_root_path) {
        Ok(client) => client,
        Err(error) => {
            return group_failure_reports(
                roots,
                &spec,
                format!("semantic_provider_startup_failed: {error}"),
            );
        }
    };

    let root_uri = match file_path_to_uri(lsp_root_path) {
        Ok(uri) => uri,
        Err(error) => {
            let _ = client.shutdown();
            return group_failure_reports(
                roots,
                &spec,
                format!("semantic_provider_startup_failed: {error}"),
            );
        }
    };

    match client.initialize(&root_uri, &spec.readiness) {
        Ok(true) => {}
        Ok(false) => {
            let _ = client.shutdown();
            return group_failure_reports(
                roots,
                &spec,
                "semantic_provider_partial: readiness_timeout".to_string(),
            );
        }
        Err(error) => {
            let _ = client.shutdown();
            return group_failure_reports(
                roots,
                &spec,
                format!("semantic_provider_startup_failed: {error}"),
            );
        }
    }

    let language_id = lsp_language_id(&language);
    let provider_version = provider_version_from_client(&client);
    let mut reports = Vec::new();
    let root_count = roots.len();

    for (idx, (root, files)) in roots.iter().enumerate() {
        if Instant::now() >= deadline {
            reports.push((
                *root,
                files.as_slice(),
                semantic_report(
                    root,
                    Some(&spec.provider_id),
                    "partial",
                    0,
                    vec!["semantic_provider_partial: wall_clock_budget".to_string()],
                ),
            ));
            continue;
        }

        let started = Instant::now();
        verbose.log(format!(
            "semantic: indexing root {} ({}) via {}",
            root.id, language, spec.provider_id
        ));
        let report = index_root_with_client(
            RootIndexRequest {
                workspace,
                root,
                files,
                file_contents,
                deadline,
                client: &client,
                provider_id: &spec.provider_id,
                provider_version: &provider_version,
                lsp_root_path,
                language_id,
            },
            occurrences,
        );
        verbose.log(format!(
            "semantic: finished root {} state={} occurrences={} elapsed_ms={} remaining_roots={}",
            root.id,
            report.state,
            report.occurrence_count,
            started.elapsed().as_millis(),
            root_count.saturating_sub(idx + 1)
        ));
        reports.push((*root, files.as_slice(), report));
    }

    let _ = client.shutdown();
    reports
}

fn group_failure_reports<'a>(
    roots: &'a [(&'a ProjectRoot, Vec<String>)],
    spec: &ServerSpec,
    reason: String,
) -> Vec<(&'a ProjectRoot, &'a [String], SemanticLanguageReport)> {
    roots
        .iter()
        .map(|(root, files)| {
            (
                *root,
                files.as_slice(),
                semantic_report(
                    root,
                    Some(&spec.provider_id),
                    "partial",
                    0,
                    vec![reason.clone()],
                ),
            )
        })
        .collect()
}

fn semantic_report(
    root: &ProjectRoot,
    provider: Option<&str>,
    state: &str,
    occurrence_count: usize,
    partial_reasons: Vec<String>,
) -> SemanticLanguageReport {
    SemanticLanguageReport {
        language: root.language.to_string(),
        root_id: root.id.clone(),
        provider: provider.map(ToString::to_string),
        state: state.to_string(),
        occurrence_count,
        partial_reasons,
    }
}

fn index_root_with_client(
    request: RootIndexRequest<'_>,
    occurrences: &mut Vec<SemanticOccurrence>,
) -> SemanticLanguageReport {
    let RootIndexRequest {
        workspace,
        root,
        files,
        file_contents,
        deadline,
        client,
        provider_id,
        provider_version,
        lsp_root_path,
        language_id,
    } = request;

    let package = package_for_root(root);
    let ctx = OccurrenceBuildCtx {
        workspace,
        root,
        package: &package,
        provider_version,
        provider_id,
        encoding: client.position_encoding(),
    };
    let mut root_occurrences = Vec::new();
    let mut partial_reasons = Vec::new();

    for path in files {
        if Instant::now() >= deadline {
            partial_reasons.push("semantic_provider_partial: wall_clock_budget".to_string());
            break;
        }
        let Some(lsp_path) = lsp_relative_path(workspace, lsp_root_path, path) else {
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

    let mut reference_budget = reference_probe_limit_for_root(root, files);
    if Instant::now() < deadline && reference_budget > 0 {
        let probes = unique_probe_positions_from_occurrences(&root_occurrences, reference_budget);
        for probe in probes {
            if Instant::now() >= deadline || reference_budget == 0 {
                partial_reasons.push("semantic_provider_partial: reference_budget".to_string());
                break;
            }
            let Some(lsp_path) = lsp_relative_path(workspace, lsp_root_path, &probe.path) else {
                partial_reasons.push(format!(
                    "semantic_provider_partial: path_outside_root:{}",
                    probe.path
                ));
                continue;
            };
            let locations = collect_reference_locations(client, &lsp_path, &probe.position, 32);
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

    let count = root_occurrences.len();
    occurrences.append(&mut root_occurrences);

    let state = if partial_reasons.is_empty() {
        "fresh"
    } else {
        "partial"
    };
    semantic_report(root, Some(provider_id), state, count, partial_reasons)
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
        ProjectLanguage::Kotlin => "kotlin",
        ProjectLanguage::TypeScript => "typescript",
        ProjectLanguage::Ruby => "ruby",
        ProjectLanguage::Swift => "swift",
    }
}

fn configured_reference_probe_limit() -> Option<usize> {
    std::env::var("CODETRAIL_LSP_REFERENCE_PROBES")
        .ok()
        .and_then(|value| value.parse().ok())
}

fn reference_probe_limit_for_root(root: &ProjectRoot, files: &[String]) -> usize {
    if let Some(limit) = configured_reference_probe_limit() {
        return limit;
    }
    if root.language == ProjectLanguage::Java {
        return MAX_REFERENCE_PROBES.max(
            files
                .len()
                .saturating_mul(32)
                .min(MAX_HIGH_FANOUT_REFERENCE_PROBES),
        );
    }
    if root.language == ProjectLanguage::Swift {
        return MAX_REFERENCE_PROBES.max(
            files
                .len()
                .saturating_mul(64)
                .min(MAX_HIGH_FANOUT_REFERENCE_PROBES),
        );
    }
    MAX_REFERENCE_PROBES
}

fn lsp_relative_path(workspace: &Workspace, lsp_root_path: &Path, path: &str) -> Option<String> {
    let abs_path = workspace.abs_path(path);
    let relative = abs_path.strip_prefix(lsp_root_path).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
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
    let mut parents = Vec::new();
    flatten_symbol_occurrences_with_parents(ctx, path, symbols, content, &mut parents, out);
}

fn flatten_symbol_occurrences_with_parents(
    ctx: &OccurrenceBuildCtx<'_>,
    path: &str,
    symbols: &[DocumentSymbol],
    content: &str,
    parents: &mut Vec<SymbolDescriptor>,
    out: &mut Vec<SemanticOccurrence>,
) {
    for symbol in symbols {
        if let Some(occurrence) =
            definition_occurrence_from_lsp(ctx, path, symbol, content, parents)
        {
            out.push(occurrence);
        }
        parents.push(symbol_descriptor_from_lsp(symbol));
        flatten_symbol_occurrences_with_parents(ctx, path, &symbol.children, content, parents, out);
        parents.pop();
    }
}

fn definition_occurrence_from_lsp(
    ctx: &OccurrenceBuildCtx<'_>,
    path: &str,
    symbol: &DocumentSymbol,
    content: &str,
    parents: &[SymbolDescriptor],
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
        symbol: semantic_symbol_from_lsp(
            ctx.root,
            symbol,
            parents,
            ctx.package,
            ctx.provider_version,
        ),
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
    parents: &[SymbolDescriptor],
    package: &SymbolPackage,
    provider_version: &SemanticProviderVersion,
) -> SemanticSymbol {
    let kind = lsp_kind_to_symbol_kind(symbol.kind);
    let mut descriptors = parents.to_vec();
    descriptors.push(symbol_descriptor_from_lsp(symbol));
    SemanticSymbol {
        identity: SymbolIdentity {
            language: root.language.clone(),
            project_id: root.id.clone(),
            package: package.clone(),
            descriptors,
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

fn symbol_descriptor_from_lsp(symbol: &DocumentSymbol) -> SymbolDescriptor {
    let kind = lsp_kind_to_symbol_kind(symbol.kind);
    SymbolDescriptor {
        name: symbol.name.clone(),
        kind: SymbolDescriptorKind::from_symbol_kind(&kind),
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
        max_batch_size: reference_probe_limit_for_root(root, files),
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

pub fn generation_manifest_path_for_snapshot(
    workspace: &Workspace,
    snapshot_id: &str,
) -> std::path::PathBuf {
    index::scip_root_for_snapshot(workspace, snapshot_id).join("generation.json")
}

pub fn read_generation_manifests(workspace: &Workspace) -> Result<Vec<GenerationManifest>> {
    let path = generation_manifest_path(workspace);
    read_generation_manifests_at(&path)
}

pub fn read_generation_manifests_for_snapshot(
    workspace: &Workspace,
    snapshot_id: &str,
) -> Result<Vec<GenerationManifest>> {
    let path = generation_manifest_path_for_snapshot(workspace, snapshot_id);
    read_generation_manifests_at(&path)
}

fn read_generation_manifests_at(path: &Path) -> Result<Vec<GenerationManifest>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read(path)?;
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
        "scip": report.scip.as_ref().map(|scip| json!({
            "generated": scip.generated,
            "imported": scip.imported,
            "source": scip.source,
            "path": scip.path,
            "documentCount": scip.document_count,
            "occurrenceCount": scip.occurrence_count,
            "symbolCount": scip.symbol_count,
        })),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_graph::ProjectLanguage;

    fn manifest(state: ManifestState, partial_reasons: Vec<String>) -> GenerationManifest {
        GenerationManifest {
            schema_version: 1,
            generation_id: "test".to_string(),
            root_id: "java:app".to_string(),
            language: ProjectLanguage::Java,
            provider_name: "scip-java".to_string(),
            provider_version_hash: "provider".to_string(),
            environment_hash: "env".to_string(),
            source_proof_hash: "source".to_string(),
            config_proof_hash: "config".to_string(),
            state,
            partial_reasons,
            created_at_epoch_ms: 1,
            updated_at_epoch_ms: 1,
        }
    }

    #[test]
    fn occurrence_db_skip_requires_fresh_generation_manifests() {
        assert!(generation_manifests_allow_occurrence_skip(&[]));
        assert!(generation_manifests_allow_occurrence_skip(&[manifest(
            ManifestState::Fresh,
            Vec::new()
        )]));
        assert!(!generation_manifests_allow_occurrence_skip(&[manifest(
            ManifestState::Partial,
            vec!["semantic_provider_partial: wall_clock_budget".to_string()]
        )]));
        assert!(!generation_manifests_allow_occurrence_skip(&[manifest(
            ManifestState::Fresh,
            vec!["semantic_provider_partial: reference_budget".to_string()]
        )]));
        assert!(!generation_manifests_allow_occurrence_skip(&[manifest(
            ManifestState::Missing,
            vec!["semantic_provider_missing".to_string()]
        )]));
    }

    #[test]
    fn precise_use_blocks_missing_stale_and_updating_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::discover(dir.path()).unwrap();
        assert!(generation_manifests_allow_precise_use(&workspace).unwrap());

        let path = generation_manifest_path(&workspace);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for state in [
            ManifestState::Missing,
            ManifestState::Stale,
            ManifestState::Updating,
        ] {
            fs::write(
                &path,
                serde_json::to_vec_pretty(&vec![manifest(state, Vec::new())]).unwrap(),
            )
            .unwrap();
            assert!(!generation_manifests_allow_precise_use(&workspace).unwrap());
        }

        fs::write(
            &path,
            serde_json::to_vec_pretty(&vec![manifest(
                ManifestState::Partial,
                vec!["semantic_provider_partial: reference_budget".to_string()],
            )])
            .unwrap(),
        )
        .unwrap();
        assert!(generation_manifests_allow_precise_use(&workspace).unwrap());
    }
}

use std::collections::BTreeSet;
use std::path::Path;

use crate::project_graph::ProjectLanguage;
use crate::semantic_provider::{
    NormalizedSemanticFact, PartialReason, ProviderBudget, ProviderCapabilities, ProviderFailure,
    ProviderFailureReason, ProviderSession, ProviderSessionInput, SemanticBatchResult,
    SemanticProbe, SemanticProvider, SemanticProviderVersion,
};

use super::client::{LspClient, LspLocation, LspPosition};
use super::registry::resolve_server;

pub const LSP_PROVIDER_NAME: &str = "codetrail-lsp-bridge";
pub const LSP_PROTOCOL_VERSION: u32 = 1;

pub struct LspSemanticProvider {
    language: ProjectLanguage,
}

impl LspSemanticProvider {
    pub fn new(language: ProjectLanguage) -> Self {
        Self { language }
    }

    fn capabilities_for(language: &ProjectLanguage) -> ProviderCapabilities {
        ProviderCapabilities {
            language: language.clone(),
            provider_version: SemanticProviderVersion {
                name: LSP_PROVIDER_NAME.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                protocol_version: LSP_PROTOCOL_VERSION,
            },
            supports_batch_resolve: true,
            supports_import_graph: false,
            supports_workspace_symbols: true,
            max_batch_size: 500,
            partial_reasons: vec![
                PartialReason::ProviderMissing,
                PartialReason::StartupFailed,
                PartialReason::Timeout,
                PartialReason::ResourceLimited,
                PartialReason::ProviderPartial,
                PartialReason::ResolveFailed,
            ],
        }
    }
}

impl SemanticProvider for LspSemanticProvider {
    fn id(&self) -> &str {
        LSP_PROVIDER_NAME
    }

    fn language(&self) -> ProjectLanguage {
        self.language.clone()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        Self::capabilities_for(&self.language)
    }

    fn budget(&self) -> ProviderBudget {
        ProviderBudget::default()
    }

    fn start_session(
        &mut self,
        input: ProviderSessionInput,
    ) -> Result<ProviderSession, ProviderFailure> {
        let spec = resolve_server(&input.language).ok_or_else(|| ProviderFailure {
            root_id: input.root.id.clone(),
            provider_id: self.id().to_string(),
            reason: ProviderFailureReason::ProviderMissing,
            message: format!("no LSP server registered for {}", input.language),
        })?;
        let root_path = Path::new(&input.root.path);
        let workspace_root = if root_path.is_absolute() {
            root_path.to_path_buf()
        } else {
            Path::new(".").join(root_path)
        };
        let mut client =
            LspClient::spawn(&spec, &workspace_root).map_err(|error| ProviderFailure {
                root_id: input.root.id.clone(),
                provider_id: spec.provider_id.clone(),
                reason: ProviderFailureReason::StartupFailed,
                message: error.to_string(),
            })?;
        let root_uri = super::registry::file_path_to_uri(&workspace_root).map_err(|error| {
            ProviderFailure {
                root_id: input.root.id.clone(),
                provider_id: spec.provider_id.clone(),
                reason: ProviderFailureReason::StartupFailed,
                message: error.to_string(),
            }
        })?;
        let ready = client
            .initialize(&root_uri, &spec.readiness)
            .map_err(|error| ProviderFailure {
                root_id: input.root.id.clone(),
                provider_id: spec.provider_id.clone(),
                reason: ProviderFailureReason::StartupFailed,
                message: error.to_string(),
            })?;
        if !ready {
            return Err(ProviderFailure {
                root_id: input.root.id.clone(),
                provider_id: spec.provider_id.clone(),
                reason: ProviderFailureReason::StartupFailed,
                message: "semantic provider readiness timed out".to_string(),
            });
        }
        // Session state is managed by scip_gen; trait session is metadata only here.
        Ok(ProviderSession {
            root_id: input.root.id.clone(),
            provider_id: spec.provider_id.clone(),
            language: input.language.clone(),
            state: crate::semantic_provider::ProviderRootState::Ready,
        })
    }

    fn resolve_batch(
        &mut self,
        session: &ProviderSession,
        probes: &[SemanticProbe],
    ) -> SemanticBatchResult {
        SemanticBatchResult::partial(
            session,
            probes,
            PartialReason::UnsupportedCapability,
            "LSP bridge batch resolve is handled by scip_gen workspace indexing",
        )
    }

    fn shutdown_idle(&mut self, _root_id: &str) {}
}

pub fn index_files_with_client(
    client: &LspClient,
    language_id: &str,
    files: &[String],
    file_contents: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<NormalizedSemanticFact>, String> {
    let mut facts = Vec::new();
    for path in files {
        let content = file_contents.get(path).cloned().unwrap_or_default();
        if client.did_open(path, language_id, &content).is_err() {
            continue;
        }
        let symbols = client
            .document_symbol(path)
            .map_err(|error| error.to_string())?;
        flatten_symbols(path, &symbols, &mut facts, session_language_id(language_id));
    }
    Ok(facts)
}

fn session_language_id(language_id: &str) -> &str {
    language_id
}

fn flatten_symbols(
    path: &str,
    symbols: &[super::client::DocumentSymbol],
    facts: &mut Vec<NormalizedSemanticFact>,
    language: &str,
) {
    for symbol in symbols {
        facts.push(NormalizedSemanticFact {
            root_id: String::new(),
            language: language_from_id(language),
            file: path.to_string(),
            range: crate::semantic_provider::SemanticRange {
                start_line: symbol.selection_range.start.line,
                start_column: symbol.selection_range.start.character,
                end_line: symbol.selection_range.end.line,
                end_column: symbol.selection_range.end.character,
            },
            symbol: symbol.name.clone(),
            kind: crate::semantic_provider::SemanticProbeKind::Definition,
            provider_id: LSP_PROVIDER_NAME.to_string(),
        });
        flatten_symbols(path, &symbol.children, facts, language);
    }
}

fn language_from_id(language_id: &str) -> ProjectLanguage {
    match language_id {
        "go" => ProjectLanguage::Go,
        "rust" => ProjectLanguage::Rust,
        "java" => ProjectLanguage::Java,
        "typescript" | "javascript" => ProjectLanguage::TypeScript,
        "ruby" => ProjectLanguage::Ruby,
        _ => ProjectLanguage::Rust,
    }
}

pub fn collect_reference_locations(
    client: &LspClient,
    path: &str,
    position: &LspPosition,
    budget: usize,
) -> Vec<LspLocation> {
    if budget == 0 {
        return Vec::new();
    }
    client
        .references(path, position, false)
        .unwrap_or_default()
        .into_iter()
        .take(budget)
        .collect()
}

pub fn unique_probe_positions(
    facts: &[NormalizedSemanticFact],
    limit: usize,
) -> Vec<(String, LspPosition)> {
    let mut seen = BTreeSet::new();
    let mut probes = Vec::new();
    for fact in facts {
        if fact.kind != crate::semantic_provider::SemanticProbeKind::Definition {
            continue;
        }
        let key = format!(
            "{}:{}:{}",
            fact.file, fact.range.start_line, fact.range.start_column
        );
        if !seen.insert(key) {
            continue;
        }
        probes.push((
            fact.file.clone(),
            LspPosition {
                line: fact.range.start_line,
                character: fact.range.start_column,
            },
        ));
        if probes.len() >= limit {
            break;
        }
    }
    probes
}

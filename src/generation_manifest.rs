//! Generation manifest and freshness gate.
//!
//! After semantic providers, config facts, and polyglot project graphs land,
//! queries must never silently return stale precise results. This module defines
//! a per-root generation manifest, a state machine, and a query-time freshness
//! gate that blocks stale precise results and surfaces machine-readable caveats.
//!
//! Design contract:
//! - A generation manifest is scoped to one (root_id, language, provider_name)
//!   triple. Each triple has its own freshness lifecycle.
//! - The freshness gate runs *before* a query returns precise facts. If the
//!   manifest is stale, updating, or partial, the gate must reject precise-only
//!   results and force fallback or caveat-only output.
//! - Refresh is triggered by file events, config changes, provider environment
//!   changes, query-time proof checks, and periodic reconcile. Each trigger
//!   transitions one or more manifests through the state machine.
//! - The manifest records provider version, environment, source, and config
//!   hashes so that any change in any dimension is detectable.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    config_facts::ConfigFact,
    project_graph::{ProjectLanguage, ProjectRoot},
    semantic_provider::ProviderCapabilities,
};

// ── Manifest ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationManifest {
    pub schema_version: u32,
    pub generation_id: String,
    pub root_id: String,
    pub language: ProjectLanguage,
    pub provider_name: String,
    pub provider_version_hash: String,
    pub environment_hash: String,
    pub source_proof_hash: String,
    pub config_proof_hash: String,
    pub state: ManifestState,
    pub partial_reasons: Vec<String>,
    pub created_at_epoch_ms: u64,
    pub updated_at_epoch_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestState {
    /// Generation is current and precise results are safe to use.
    Fresh,
    /// At least one proof input has changed; precise results must not be used.
    Stale,
    /// A refresh is in progress; precise results are not yet available.
    Updating,
    /// Generation completed with partial results. Precise results marked partial
    /// may still be usable, but the consumer must check reason codes.
    Partial,
    /// Provider was never started or is unreachable. No precise results exist.
    Missing,
}

impl ManifestState {
    pub fn blocks_precise(&self) -> bool {
        matches!(self, Self::Stale | Self::Updating | Self::Missing)
    }

    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Fresh | Self::Partial)
    }
}

// ── Freshness Gate ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreshnessGate {
    manifests: BTreeMap<ManifestKey, GenerationManifest>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct ManifestKey {
    root_id: String,
    language: ProjectLanguage,
    provider_name: String,
}

impl FreshnessGate {
    pub fn new() -> Self {
        Self {
            manifests: BTreeMap::new(),
        }
    }

    pub fn from_manifests(manifests: Vec<GenerationManifest>) -> Self {
        let mut gate = Self::new();
        for manifest in manifests {
            gate.upsert(manifest);
        }
        gate
    }

    pub fn upsert(&mut self, manifest: GenerationManifest) {
        let key = key_for(&manifest);
        self.manifests.insert(key, manifest);
    }

    pub fn get(
        &self,
        root_id: &str,
        language: &ProjectLanguage,
        provider_name: &str,
    ) -> Option<&GenerationManifest> {
        let key = ManifestKey {
            root_id: root_id.to_string(),
            language: language.clone(),
            provider_name: provider_name.to_string(),
        };
        self.manifests.get(&key)
    }

    /// Returns the set of root ids whose precise results are blocked.
    pub fn blocked_root_ids(&self) -> BTreeSet<String> {
        self.manifests
            .values()
            .filter(|m| m.state.blocks_precise())
            .map(|m| m.root_id.clone())
            .collect()
    }

    /// Returns manifests queryable by root, language, or provider.
    pub fn query(
        &self,
        root_id: Option<&str>,
        language: Option<&ProjectLanguage>,
        provider_name: Option<&str>,
    ) -> Vec<&GenerationManifest> {
        self.manifests
            .values()
            .filter(|m| {
                root_id.is_none_or(|rid| m.root_id == rid)
                    && language.is_none_or(|lang| &m.language == lang)
                    && provider_name.is_none_or(|pn| m.provider_name == pn)
            })
            .collect()
    }

    pub fn manifests(&self) -> impl Iterator<Item = &GenerationManifest> {
        self.manifests.values()
    }

    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }
}

impl Default for FreshnessGate {
    fn default() -> Self {
        Self::new()
    }
}

fn key_for(manifest: &GenerationManifest) -> ManifestKey {
    ManifestKey {
        root_id: manifest.root_id.clone(),
        language: manifest.language.clone(),
        provider_name: manifest.provider_name.clone(),
    }
}

// ── State machine ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum RefreshEvent {
    SourceChanged {
        root_id: String,
    },
    ConfigChanged {
        root_id: String,
        config_path: String,
    },
    ProviderEnvChanged {
        root_id: String,
        language: ProjectLanguage,
    },
    PeriodicReconcile {
        root_id: String,
    },
}

#[derive(Clone, Debug)]
pub struct TransitionResult {
    pub root_id: String,
    pub previous_state: ManifestState,
    pub new_state: ManifestState,
    pub reason: String,
}

/// Advance a single manifest through the state machine.
pub fn apply_refresh_event(
    manifest: &mut GenerationManifest,
    event: &RefreshEvent,
) -> TransitionResult {
    let previous = manifest.state.clone();
    let now_ms = now_epoch_ms();

    let (new_state, reason) = match event {
        RefreshEvent::SourceChanged { .. } | RefreshEvent::ConfigChanged { .. } => {
            (ManifestState::Stale, "proof input changed".to_string())
        }
        RefreshEvent::ProviderEnvChanged { .. } => (
            ManifestState::Stale,
            "provider environment changed".to_string(),
        ),
        RefreshEvent::PeriodicReconcile { .. } if manifest.state == ManifestState::Stale => (
            ManifestState::Updating,
            "periodic reconcile triggered".to_string(),
        ),
        _ => {
            return TransitionResult {
                root_id: manifest.root_id.clone(),
                previous_state: previous,
                new_state: manifest.state.clone(),
                reason: "no-op: event did not match transition rule".to_string(),
            }
        }
    };

    manifest.state = new_state.clone();
    manifest.updated_at_epoch_ms = now_ms;

    TransitionResult {
        root_id: manifest.root_id.clone(),
        previous_state: previous,
        new_state,
        reason,
    }
}

/// Mark a manifest as fresh after a successful refresh.
pub fn mark_fresh(manifest: &mut GenerationManifest, hashes: &ProofHashes) {
    manifest.state = ManifestState::Fresh;
    manifest.provider_version_hash = hashes.provider_version_hash.clone();
    manifest.environment_hash = hashes.environment_hash.clone();
    manifest.source_proof_hash = hashes.source_proof_hash.clone();
    manifest.config_proof_hash = hashes.config_proof_hash.clone();
    manifest.partial_reasons.clear();
    manifest.updated_at_epoch_ms = now_epoch_ms();
}

/// Mark a manifest as partial with reasons.
pub fn mark_partial(manifest: &mut GenerationManifest, reasons: Vec<String>) {
    manifest.state = ManifestState::Partial;
    manifest.partial_reasons = reasons;
    manifest.updated_at_epoch_ms = now_epoch_ms();
}

/// Mark a manifest as updating (refresh in progress).
pub fn mark_updating(manifest: &mut GenerationManifest) {
    manifest.state = ManifestState::Updating;
    manifest.updated_at_epoch_ms = now_epoch_ms();
}

/// Mark a manifest as missing (provider never started).
pub fn mark_missing(manifest: &mut GenerationManifest) {
    manifest.state = ManifestState::Missing;
    manifest.updated_at_epoch_ms = now_epoch_ms();
}

// ── Proof hashes ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofHashes {
    pub provider_version_hash: String,
    pub environment_hash: String,
    pub source_proof_hash: String,
    pub config_proof_hash: String,
}

/// Hash provider capabilities into a stable fingerprint.
pub fn hash_provider_version(caps: &ProviderCapabilities) -> String {
    let payload = format!(
        "{}:{}:{}",
        caps.provider_version.name,
        caps.provider_version.version,
        caps.provider_version.protocol_version
    );
    blake3::hash(payload.as_bytes()).to_hex().to_string()
}

/// Hash a set of source file paths and their blake3 fingerprints.
pub fn hash_source_proof(file_proofs: &[(String, String)]) -> String {
    // file_proofs: (relative_path, blake3_hex)
    let mut items: Vec<_> = file_proofs.iter().collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let payload = items
        .iter()
        .map(|(path, hash)| format!("{path}:{hash}"))
        .collect::<Vec<_>>()
        .join("\n");
    blake3::hash(payload.as_bytes()).to_hex().to_string()
}

/// Hash config fact evidence into a stable fingerprint.
pub fn hash_config_proof(facts: &[ConfigFact]) -> String {
    // Hash the sorted set of (path, key_path, fact_kind) tuples.
    let mut items: Vec<_> = facts
        .iter()
        .map(|f| {
            (
                f.path.clone(),
                f.key_path.clone().unwrap_or_default(),
                format!("{:?}", f.fact_kind),
            )
        })
        .collect();
    items.sort();
    let payload = items
        .iter()
        .map(|(p, k, t)| format!("{p}:{k}:{t}"))
        .collect::<Vec<_>>()
        .join("\n");
    blake3::hash(payload.as_bytes()).to_hex().to_string()
}

/// Build a new manifest for a root/provider pair.
pub fn new_manifest(
    root: &ProjectRoot,
    provider_name: &str,
    hashes: &ProofHashes,
) -> GenerationManifest {
    let now = now_epoch_ms();
    let generation_id = blake3::hash(
        format!(
            "{}:{}:{}:{}",
            root.id, provider_name, hashes.source_proof_hash, now
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    GenerationManifest {
        schema_version: 1,
        generation_id,
        root_id: root.id.clone(),
        language: root.language.clone(),
        provider_name: provider_name.to_string(),
        provider_version_hash: hashes.provider_version_hash.clone(),
        environment_hash: hashes.environment_hash.clone(),
        source_proof_hash: hashes.source_proof_hash.clone(),
        config_proof_hash: hashes.config_proof_hash.clone(),
        state: ManifestState::Fresh,
        partial_reasons: Vec::new(),
        created_at_epoch_ms: now,
        updated_at_epoch_ms: now,
    }
}

/// Check whether a manifest is stale against current proof hashes.
pub fn is_stale(manifest: &GenerationManifest, hashes: &ProofHashes) -> bool {
    manifest.provider_version_hash != hashes.provider_version_hash
        || manifest.environment_hash != hashes.environment_hash
        || manifest.source_proof_hash != hashes.source_proof_hash
        || manifest.config_proof_hash != hashes.config_proof_hash
}

/// Build a freshness gate from project graph roots and provider capabilities.
pub fn gate_from_roots_and_providers(
    roots: &[ProjectRoot],
    providers: &[(&str, &ProviderCapabilities)],
    source_proofs: &BTreeMap<String, Vec<(String, String)>>,
    config_facts: &BTreeMap<String, Vec<ConfigFact>>,
) -> FreshnessGate {
    let provider_map: BTreeMap<(&str, &ProjectLanguage), &ProviderCapabilities> = providers
        .iter()
        .map(|(name, caps)| ((*name, &caps.language), *caps))
        .collect();

    let mut manifests = Vec::new();
    for root in roots {
        for ((provider_name, lang), caps) in &provider_map {
            if *lang != &root.language {
                continue;
            }
            let source_proof = source_proofs.get(&root.id).cloned().unwrap_or_default();
            let config_fact = config_facts.get(&root.id).cloned().unwrap_or_default();
            let hashes = ProofHashes {
                provider_version_hash: hash_provider_version(caps),
                environment_hash: String::from("00000000000000000000000000000000"),
                source_proof_hash: hash_source_proof(&source_proof),
                config_proof_hash: hash_config_proof(&config_fact),
            };
            manifests.push(new_manifest(root, provider_name, &hashes));
        }
    }
    FreshnessGate::from_manifests(manifests)
}

pub fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_graph::{ProjectLanguage, ProjectRootKind};

    fn test_root(id: &str, lang: ProjectLanguage) -> ProjectRoot {
        ProjectRoot {
            id: id.to_string(),
            path: id.to_string(),
            language: lang,
            kind: ProjectRootKind::GoModule,
            markers: Vec::new(),
        }
    }

    fn test_hashes() -> ProofHashes {
        ProofHashes {
            provider_version_hash: "abc123".to_string(),
            environment_hash: "def456".to_string(),
            source_proof_hash: "src789".to_string(),
            config_proof_hash: "cfg012".to_string(),
        }
    }

    #[test]
    fn fresh_manifest_does_not_block_precise() {
        let root = test_root("go:backend", ProjectLanguage::Go);
        let manifest = new_manifest(&root, "gopls", &test_hashes());
        assert_eq!(manifest.state, ManifestState::Fresh);
        assert!(!manifest.state.blocks_precise());
        assert!(manifest.state.is_usable());
    }

    #[test]
    fn source_change_transitions_to_stale() {
        let root = test_root("rust:lib", ProjectLanguage::Rust);
        let mut manifest = new_manifest(&root, "rust-analyzer", &test_hashes());
        assert_eq!(manifest.state, ManifestState::Fresh);

        let result = apply_refresh_event(
            &mut manifest,
            &RefreshEvent::SourceChanged {
                root_id: root.id.clone(),
            },
        );
        assert_eq!(result.new_state, ManifestState::Stale);
        assert!(manifest.state.blocks_precise());
    }

    #[test]
    fn config_change_transitions_to_stale() {
        let root = test_root("ts:frontend", ProjectLanguage::TypeScript);
        let mut manifest = new_manifest(&root, "tsserver", &test_hashes());
        let result = apply_refresh_event(
            &mut manifest,
            &RefreshEvent::ConfigChanged {
                root_id: root.id.clone(),
                config_path: "tsconfig.json".to_string(),
            },
        );
        assert_eq!(result.new_state, ManifestState::Stale);
    }

    #[test]
    fn provider_env_change_transitions_to_stale() {
        let root = test_root("go:svc", ProjectLanguage::Go);
        let mut manifest = new_manifest(&root, "gopls", &test_hashes());
        let result = apply_refresh_event(
            &mut manifest,
            &RefreshEvent::ProviderEnvChanged {
                root_id: root.id.clone(),
                language: ProjectLanguage::Go,
            },
        );
        assert_eq!(result.new_state, ManifestState::Stale);
    }

    #[test]
    fn stale_reconcile_transitions_to_updating() {
        let root = test_root("java:app", ProjectLanguage::Java);
        let mut manifest = new_manifest(&root, "jdtls", &test_hashes());
        manifest.state = ManifestState::Stale;

        let result = apply_refresh_event(
            &mut manifest,
            &RefreshEvent::PeriodicReconcile {
                root_id: root.id.clone(),
            },
        );
        assert_eq!(result.new_state, ManifestState::Updating);
        assert!(manifest.state.blocks_precise());
    }

    #[test]
    fn fresh_reconcile_is_noop() {
        let root = test_root("rust:crate", ProjectLanguage::Rust);
        let mut manifest = new_manifest(&root, "rust-analyzer", &test_hashes());
        let result = apply_refresh_event(
            &mut manifest,
            &RefreshEvent::PeriodicReconcile {
                root_id: root.id.clone(),
            },
        );
        assert_eq!(result.new_state, ManifestState::Fresh);
        assert_eq!(result.reason, "no-op: event did not match transition rule");
    }

    #[test]
    fn mark_fresh_restores_state_and_updates_hashes() {
        let root = test_root("python:ml", ProjectLanguage::TypeScript);
        let mut manifest = new_manifest(&root, "pyright", &test_hashes());
        manifest.state = ManifestState::Updating;

        let new_hashes = ProofHashes {
            provider_version_hash: "new_pv".to_string(),
            environment_hash: "new_env".to_string(),
            source_proof_hash: "new_src".to_string(),
            config_proof_hash: "new_cfg".to_string(),
        };
        mark_fresh(&mut manifest, &new_hashes);

        assert_eq!(manifest.state, ManifestState::Fresh);
        assert_eq!(manifest.provider_version_hash, "new_pv");
        assert!(manifest.partial_reasons.is_empty());
    }

    #[test]
    fn mark_partial_with_reasons() {
        let root = test_root("java:mod", ProjectLanguage::Java);
        let mut manifest = new_manifest(&root, "jdtls", &test_hashes());
        mark_partial(&mut manifest, vec!["proc_macro_disabled".to_string()]);

        assert_eq!(manifest.state, ManifestState::Partial);
        assert!(!manifest.state.blocks_precise());
        assert!(manifest.state.is_usable());
        assert_eq!(manifest.partial_reasons, vec!["proc_macro_disabled"]);
    }

    #[test]
    fn mark_missing_blocks_precise() {
        let root = test_root("ts:lib", ProjectLanguage::TypeScript);
        let mut manifest = new_manifest(&root, "tsserver", &test_hashes());
        mark_missing(&mut manifest);

        assert_eq!(manifest.state, ManifestState::Missing);
        assert!(manifest.state.blocks_precise());
        assert!(!manifest.state.is_usable());
    }

    #[test]
    fn freshness_gate_blocks_stale_roots() {
        let root_a = test_root("go:a", ProjectLanguage::Go);
        let root_b = test_root("rust:b", ProjectLanguage::Rust);

        let mut manifest_a = new_manifest(&root_a, "gopls", &test_hashes());
        let manifest_b = new_manifest(&root_b, "rust-analyzer", &test_hashes());
        manifest_a.state = ManifestState::Stale;

        let gate = FreshnessGate::from_manifests(vec![manifest_a, manifest_b]);
        let blocked = gate.blocked_root_ids();
        assert_eq!(blocked.len(), 1);
        assert!(blocked.contains("go:a"));
    }

    #[test]
    fn freshness_gate_query_filters_by_root_language_provider() {
        let root = test_root("go:srv", ProjectLanguage::Go);
        let manifest = new_manifest(&root, "gopls", &test_hashes());
        let gate = FreshnessGate::from_manifests(vec![manifest]);

        let by_root = gate.query(Some("go:srv"), None, None);
        assert_eq!(by_root.len(), 1);

        let by_lang = gate.query(None, Some(&ProjectLanguage::Go), None);
        assert_eq!(by_lang.len(), 1);

        let by_provider = gate.query(None, None, Some("gopls"));
        assert_eq!(by_provider.len(), 1);

        let no_match = gate.query(Some("rust:other"), None, None);
        assert!(no_match.is_empty());
    }

    #[test]
    fn is_stale_detects_hash_mismatch() {
        let root = test_root("python:app", ProjectLanguage::TypeScript);
        let manifest = new_manifest(&root, "pyright", &test_hashes());

        let same = ProofHashes {
            provider_version_hash: "abc123".to_string(),
            environment_hash: "def456".to_string(),
            source_proof_hash: "src789".to_string(),
            config_proof_hash: "cfg012".to_string(),
        };
        assert!(!is_stale(&manifest, &same));

        let different = ProofHashes {
            provider_version_hash: "changed".to_string(),
            environment_hash: "def456".to_string(),
            source_proof_hash: "src789".to_string(),
            config_proof_hash: "cfg012".to_string(),
        };
        assert!(is_stale(&manifest, &different));
    }

    #[test]
    fn hash_proof_deterministic() {
        let proofs_a = vec![
            ("a.rs".to_string(), "aaa".to_string()),
            ("b.rs".to_string(), "bbb".to_string()),
        ];
        let proofs_b = vec![
            ("b.rs".to_string(), "bbb".to_string()),
            ("a.rs".to_string(), "aaa".to_string()),
        ];
        assert_eq!(hash_source_proof(&proofs_a), hash_source_proof(&proofs_b));

        let different = vec![("a.rs".to_string(), "ccc".to_string())];
        assert_ne!(hash_source_proof(&proofs_a), hash_source_proof(&different));
    }
}

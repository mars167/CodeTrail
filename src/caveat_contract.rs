//! Machine-readable caveat contract and security boundaries.
//!
//! After semantic providers, config facts, freshness gates, and diff proof land,
//! the set of possible caveats grows beyond the initial text-search era. This
//! module defines the unified caveat code registry, severity/category mapping,
//! and security contract that every public JSON response must obey.
//!
//! Security contract (non-negotiable):
//! - No shell, build-script, or package-script execution.
//! - No default access to paths outside the workspace root.
//! - Secret-like values must be masked in previews.
//! - Provider stdout/stderr must be captured and never leaked to public output.
//! - Provider processes run with restricted cwd and env.
//! - HTML/JSON escaping on all user-controlled strings in public output.
//! - Dependency caches (node_modules, .m2, GOPATH, Cargo registry) are read-only.

use serde::{Deserialize, Serialize};

// ── Caveat code registry ────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaveatCode {
    // ── Capability (info / capability) ──────────────────────────────────────
    PreciseScipIndexUnavailable,
    ParserFact,
    RefsIdentifierBoundaryTextSearch,
    InferredCandidate,

    // ── Semantic provider (info / capability) ───────────────────────────────
    SemanticProviderMissing,
    SemanticProviderNotStarted,
    SemanticProviderPartial,
    SemanticProviderUnsupportedLanguage,

    // ── Freshness (warning / risk) ──────────────────────────────────────────
    SemanticGenerationStale,
    SemanticRefreshInProgress,
    SemanticIndexStale,
    LiveOverlayUsed,
    WatcherLagDetected,

    // ── Config / script (warning / risk) ────────────────────────────────────
    ConfigFactTruncated,
    ConfigParseFailure,
    ConfigEdgeUnresolved,
    SecretMasked,
    LargeFileTruncated,

    // ── Diff (warning / risk) ───────────────────────────────────────────────
    DiffProofTruncated,
    DiffProofPartial,

    // ── Budget / resource (warning / risk) ──────────────────────────────────
    ProviderResourceLimited,
    ProviderTimeout,
    CandidateBudgetExceeded,
    BroadQueryGuard,
    TruncatedOutput,
    BinaryFileNotDisplayed,

    // ── Safety / error ──────────────────────────────────────────────────────
    PathEscapesWorkspaceRoot,
    ReadFailed,
    InvalidLineRange,
    NoMatch,

    // ── MCP / tool (error / error) ──────────────────────────────────────────
    CliUsageError,
    UnknownTool,
    InvalidMcpArgument,
    UnsupportedMcpScope,

    // ── Generation (warning / risk) ─────────────────────────────────────────
    GeneratedSourceUnverified,
    PartialParseSyntaxErrors,
    AmbiguousResults,
    UnsupportedSearchMode,
    WorkspacePathResolveFailed,
    FailedToParseNativeScipIndex,
    SavedQuerySnapshotMismatch,

    // ── Fallback (custom metadata) ──────────────────────────────────────────
    SourceFactFallback,
}

impl CaveatCode {
    pub fn code_str(&self) -> &'static str {
        match self {
            Self::PreciseScipIndexUnavailable => "precise_scip_index_unavailable",
            Self::ParserFact => "parser_fact",
            Self::RefsIdentifierBoundaryTextSearch => {
                "refs_identifier_boundary_text_search_unless_a_precise_occurrence_index_is_available"
            }
            Self::InferredCandidate => "inferred_candidate",
            Self::SemanticProviderMissing => "semantic_provider_missing",
            Self::SemanticProviderNotStarted => "semantic_provider_not_started",
            Self::SemanticProviderPartial => "semantic_provider_partial",
            Self::SemanticProviderUnsupportedLanguage => {
                "semantic_provider_unsupported_language"
            }
            Self::SemanticGenerationStale => "semantic_generation_stale",
            Self::SemanticRefreshInProgress => "semantic_refresh_in_progress",
            Self::SemanticIndexStale => "semantic_index_stale",
            Self::LiveOverlayUsed => "live_overlay_used",
            Self::WatcherLagDetected => "watcher_lag_detected",
            Self::ConfigFactTruncated => "config_fact_truncated",
            Self::ConfigParseFailure => "config_parse_failure",
            Self::ConfigEdgeUnresolved => "config_edge_unresolved",
            Self::SecretMasked => "secret_masked",
            Self::LargeFileTruncated => "large_file_truncated",
            Self::DiffProofTruncated => "diff_proof_truncated",
            Self::DiffProofPartial => "diff_proof_partial",
            Self::ProviderResourceLimited => "provider_resource_limited",
            Self::ProviderTimeout => "provider_timeout",
            Self::CandidateBudgetExceeded => "candidate_budget_exceeded",
            Self::BroadQueryGuard => "broad_query_guard",
            Self::TruncatedOutput => "truncated_output",
            Self::BinaryFileNotDisplayed => "binary_file_not_displayed",
            Self::PathEscapesWorkspaceRoot => "path_escapes_workspace_root",
            Self::ReadFailed => "read_failed",
            Self::InvalidLineRange => "invalid_line_range",
            Self::NoMatch => "no_match",
            Self::CliUsageError => "cli_usage_error",
            Self::UnknownTool => "unknown_tool",
            Self::InvalidMcpArgument => "invalid_mcp_argument",
            Self::UnsupportedMcpScope => "unsupported_mcp_scope",
            Self::GeneratedSourceUnverified => "generated_source_unverified",
            Self::PartialParseSyntaxErrors => "partial_parse_syntax_errors",
            Self::AmbiguousResults => "ambiguous_results",
            Self::UnsupportedSearchMode => "unsupported_search_mode",
            Self::WorkspacePathResolveFailed => "workspace_path_resolve_failed",
            Self::FailedToParseNativeScipIndex => "failed_to_parse_native_scip_index",
            Self::SavedQuerySnapshotMismatch => "saved_query_snapshot_mismatch",
            Self::SourceFactFallback => "source_fact_fallback",
        }
    }

    pub fn severity(&self) -> &'static str {
        match self {
            Self::PreciseScipIndexUnavailable
            | Self::ParserFact
            | Self::RefsIdentifierBoundaryTextSearch
            | Self::InferredCandidate
            | Self::SemanticProviderMissing
            | Self::SemanticProviderNotStarted
            | Self::SemanticProviderPartial
            | Self::SemanticProviderUnsupportedLanguage => "info",

            Self::CliUsageError
            | Self::UnknownTool
            | Self::InvalidMcpArgument
            | Self::UnsupportedMcpScope
            | Self::PathEscapesWorkspaceRoot
            | Self::ReadFailed
            | Self::InvalidLineRange => "error",

            _ => "warning",
        }
    }

    pub fn category(&self) -> &'static str {
        match self {
            Self::PreciseScipIndexUnavailable
            | Self::ParserFact
            | Self::RefsIdentifierBoundaryTextSearch
            | Self::InferredCandidate
            | Self::SemanticProviderMissing
            | Self::SemanticProviderNotStarted
            | Self::SemanticProviderPartial
            | Self::SemanticProviderUnsupportedLanguage => "capability",

            Self::CliUsageError
            | Self::UnknownTool
            | Self::InvalidMcpArgument
            | Self::UnsupportedMcpScope
            | Self::PathEscapesWorkspaceRoot
            | Self::ReadFailed
            | Self::InvalidLineRange => "error",

            _ => "risk",
        }
    }

    /// Returns the recommended next action for an automated consumer (e.g., Codex agent).
    pub fn next_action(&self) -> &'static str {
        match self {
            Self::NoMatch => "refine_query_or_accept_absence_is_not_proof",
            Self::AmbiguousResults => "narrow_by_path_language_or_kind",
            Self::TruncatedOutput
            | Self::LargeFileTruncated
            | Self::ConfigFactTruncated
            | Self::DiffProofTruncated => "reduce_scope_or_paginate",
            Self::BroadQueryGuard => "narrow_query_or_increase_limit",
            Self::SemanticGenerationStale
            | Self::SemanticIndexStale
            | Self::SemanticRefreshInProgress => "wait_or_fallback_to_text",
            Self::SemanticProviderMissing | Self::SemanticProviderNotStarted => {
                "install_provider_or_use_parser_fallback"
            }
            Self::SemanticProviderPartial
            | Self::ProviderResourceLimited
            | Self::ProviderTimeout => "accept_partial_or_retry",
            Self::ConfigEdgeUnresolved => "conservatively_treat_affected_roots_as_stale",
            Self::ConfigParseFailure => "inspect_raw_source_for_config_errors",
            Self::SecretMasked => "do_not_use_masked_values_in_prompts_or_logs",
            Self::PathEscapesWorkspaceRoot => "reject_request_and_report",
            Self::GeneratedSourceUnverified => "treat_as_source_fact_not_precise",
            Self::SourceFactFallback => "accept_fallback_reliability",
            Self::ParserFact | Self::InferredCandidate | Self::RefsIdentifierBoundaryTextSearch => {
                "verify_with_read_before_editing"
            }
            Self::PreciseScipIndexUnavailable | Self::FailedToParseNativeScipIndex => {
                "generate_scip_index_or_accept_parser_fallback"
            }
            Self::CandidateBudgetExceeded => "narrow_scope_or_language",
            Self::PartialParseSyntaxErrors => "inspect_files_for_syntax_errors",
            Self::DiffProofPartial | Self::LiveOverlayUsed | Self::WatcherLagDetected => {
                "accept_may_be_stale_or_wait_for_reconcile"
            }
            Self::SemanticProviderUnsupportedLanguage => {
                "use_parser_or_text_fallback_for_this_language"
            }
            Self::BinaryFileNotDisplayed => "use_external_tool_for_binary_inspection",
            Self::UnsupportedSearchMode => "use_alternative_search_mode",
            Self::WorkspacePathResolveFailed => "verify_path_and_workspace_root",
            Self::SavedQuerySnapshotMismatch => "replay_query_against_current_snapshot",
            Self::ReadFailed => "verify_file_exists_and_is_readable",
            Self::InvalidLineRange => "correct_line_range_and_retry",
            Self::CliUsageError
            | Self::UnknownTool
            | Self::InvalidMcpArgument
            | Self::UnsupportedMcpScope => "fix_request_and_retry",
        }
    }
}

// ── Security contract ───────────────────────────────────────────────────────

/// Security boundaries that every code path must respect.
#[derive(Clone, Debug)]
pub struct SecurityContract {
    pub allow_shell_execution: bool,
    pub allow_outside_workspace: bool,
    pub allow_secret_in_preview: bool,
    pub allow_provider_stdout_leak: bool,
    pub allow_unrestricted_provider_env: bool,
}

impl Default for SecurityContract {
    fn default() -> Self {
        Self {
            allow_shell_execution: false,
            allow_outside_workspace: false,
            allow_secret_in_preview: false,
            allow_provider_stdout_leak: false,
            allow_unrestricted_provider_env: false,
        }
    }
}

impl SecurityContract {
    pub fn enforce() -> Self {
        Self::default()
    }

    pub fn validate_path_in_workspace(
        &self,
        resolved: &std::path::Path,
        workspace_root: &std::path::Path,
    ) -> Result<(), CaveatCode> {
        if !self.allow_outside_workspace && !resolved.starts_with(workspace_root) {
            return Err(CaveatCode::PathEscapesWorkspaceRoot);
        }
        Ok(())
    }
}

// ── Caveat builder ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaveatRecord {
    pub code: CaveatCode,
    pub message: String,
    pub severity: String,
    pub category: String,
    pub next_action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_root_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_symbol: Option<String>,
}

impl CaveatRecord {
    pub fn new(code: CaveatCode, message: impl Into<String>) -> Self {
        Self {
            severity: code.severity().to_string(),
            category: code.category().to_string(),
            next_action: code.next_action().to_string(),
            code,
            message: message.into(),
            affected_root_id: None,
            affected_file: None,
            affected_symbol: None,
        }
    }

    pub fn with_root(mut self, root_id: impl Into<String>) -> Self {
        self.affected_root_id = Some(root_id.into());
        self
    }

    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.affected_file = Some(file.into());
        self
    }

    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.affected_symbol = Some(symbol.into());
        self
    }
}

// ── All-caveat summary ──────────────────────────────────────────────────────

/// Return every defined caveat code for documentation and testing.
pub fn all_caveat_codes() -> Vec<CaveatCode> {
    use CaveatCode::*;
    vec![
        PreciseScipIndexUnavailable,
        ParserFact,
        RefsIdentifierBoundaryTextSearch,
        InferredCandidate,
        SemanticProviderMissing,
        SemanticProviderNotStarted,
        SemanticProviderPartial,
        SemanticProviderUnsupportedLanguage,
        SemanticGenerationStale,
        SemanticRefreshInProgress,
        SemanticIndexStale,
        LiveOverlayUsed,
        WatcherLagDetected,
        ConfigFactTruncated,
        ConfigParseFailure,
        ConfigEdgeUnresolved,
        SecretMasked,
        LargeFileTruncated,
        DiffProofTruncated,
        DiffProofPartial,
        ProviderResourceLimited,
        ProviderTimeout,
        CandidateBudgetExceeded,
        BroadQueryGuard,
        TruncatedOutput,
        BinaryFileNotDisplayed,
        PathEscapesWorkspaceRoot,
        ReadFailed,
        InvalidLineRange,
        NoMatch,
        CliUsageError,
        UnknownTool,
        InvalidMcpArgument,
        UnsupportedMcpScope,
        GeneratedSourceUnverified,
        PartialParseSyntaxErrors,
        AmbiguousResults,
        UnsupportedSearchMode,
        WorkspacePathResolveFailed,
        FailedToParseNativeScipIndex,
        SavedQuerySnapshotMismatch,
        SourceFactFallback,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_caveat_has_code_str() {
        for code in all_caveat_codes() {
            let s = code.code_str();
            assert!(!s.is_empty(), "missing code_str for {code:?}");
        }
    }

    #[test]
    fn every_caveat_has_severity_and_category() {
        for code in all_caveat_codes() {
            let sev = code.severity();
            assert!(
                matches!(sev, "info" | "warning" | "error"),
                "bad severity {sev} for {code:?}"
            );
            let cat = code.category();
            assert!(
                matches!(cat, "capability" | "risk" | "error"),
                "bad category {cat} for {code:?}"
            );
        }
    }

    #[test]
    fn every_caveat_has_next_action() {
        for code in all_caveat_codes() {
            let action = code.next_action();
            assert!(!action.is_empty(), "missing next_action for {code:?}");
        }
    }

    #[test]
    fn caveat_codes_are_unique() {
        let mut seen = BTreeSet::new();
        for code in all_caveat_codes() {
            let s = code.code_str();
            assert!(seen.insert(s.to_string()), "duplicate code_str: {s}");
        }
    }

    #[test]
    fn semantic_caveats_are_capability_info() {
        let semantic = [
            CaveatCode::SemanticProviderMissing,
            CaveatCode::SemanticProviderNotStarted,
            CaveatCode::SemanticProviderPartial,
            CaveatCode::SemanticProviderUnsupportedLanguage,
        ];
        for code in semantic {
            assert_eq!(code.severity(), "info");
            assert_eq!(code.category(), "capability");
        }
    }

    #[test]
    fn freshness_caveats_are_warning_risk() {
        let freshness = [
            CaveatCode::SemanticGenerationStale,
            CaveatCode::SemanticRefreshInProgress,
            CaveatCode::SemanticIndexStale,
        ];
        for code in freshness {
            assert_eq!(code.severity(), "warning");
            assert_eq!(code.category(), "risk");
        }
    }

    #[test]
    fn config_caveats_are_warning_risk() {
        let config = [
            CaveatCode::ConfigFactTruncated,
            CaveatCode::ConfigParseFailure,
            CaveatCode::ConfigEdgeUnresolved,
            CaveatCode::SecretMasked,
        ];
        for code in config {
            assert_eq!(code.severity(), "warning");
            assert_eq!(code.category(), "risk");
        }
    }

    #[test]
    fn error_caveats_are_error_category() {
        let errors = [
            CaveatCode::PathEscapesWorkspaceRoot,
            CaveatCode::ReadFailed,
            CaveatCode::InvalidLineRange,
            CaveatCode::CliUsageError,
            CaveatCode::UnknownTool,
            CaveatCode::InvalidMcpArgument,
            CaveatCode::UnsupportedMcpScope,
        ];
        for code in errors {
            assert_eq!(code.severity(), "error");
            assert_eq!(code.category(), "error");
        }
    }

    #[test]
    fn security_contract_defaults_deny_all() {
        let contract = SecurityContract::default();
        assert!(!contract.allow_shell_execution);
        assert!(!contract.allow_outside_workspace);
        assert!(!contract.allow_secret_in_preview);
        assert!(!contract.allow_provider_stdout_leak);
        assert!(!contract.allow_unrestricted_provider_env);
    }

    #[test]
    fn path_validation_rejects_escape() {
        let contract = SecurityContract::enforce();
        let workspace = std::path::Path::new("/workspace");
        let outside = std::path::Path::new("/etc/passwd");
        assert_eq!(
            contract.validate_path_in_workspace(outside, workspace),
            Err(CaveatCode::PathEscapesWorkspaceRoot)
        );
    }

    #[test]
    fn path_validation_allows_workspace_paths() {
        let contract = SecurityContract::enforce();
        let workspace = std::path::Path::new("/workspace");
        let inside = std::path::Path::new("/workspace/src/main.rs");
        assert!(contract
            .validate_path_in_workspace(inside, workspace)
            .is_ok());
    }

    #[test]
    fn caveat_record_builder_chains() {
        let record = CaveatRecord::new(
            CaveatCode::SemanticGenerationStale,
            "source changed since last generation",
        )
        .with_root("go:backend")
        .with_file("pkg/handler.go");

        assert_eq!(record.code, CaveatCode::SemanticGenerationStale);
        assert_eq!(record.affected_root_id.as_deref(), Some("go:backend"));
        assert_eq!(record.affected_file.as_deref(), Some("pkg/handler.go"));
        assert!(record.affected_symbol.is_none());
    }
}

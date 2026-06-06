//! Performance gate and benchmark suite design.
//!
//! Multi-language providers, Tree-sitter candidates, config facts, and
//! SCIP/store layers significantly increase time, memory, and disk costs.
//! This module defines the metrics schema, benchmark repository matrix,
//! resource budgets, and PR-gate versus full-benchmark strategies.

use serde::{Deserialize, Serialize};

// ── Benchmark repository matrix ─────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkRepository {
    pub name: String,
    pub description: String,
    pub language_distribution: Vec<LanguageShare>,
    pub source_file_count: usize,
    pub source_bytes: usize,
    pub config_file_count: usize,
    pub generated_source: bool,
    pub multi_module: bool,
    pub expected_cold_build_s: f64,
    pub expected_warm_update_s: f64,
    pub expected_store_mb: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageShare {
    pub language: String,
    pub percentage: u8,
}

/// Benchmark matrix covering real-world repository profiles.
pub fn benchmark_matrix() -> Vec<BenchmarkRepository> {
    vec![
        BenchmarkRepository {
            name: "single-go-medium".to_string(),
            description: "Single Go module, ~200 source files, ~50 config files".to_string(),
            language_distribution: vec![LanguageShare {
                language: "Go".to_string(),
                percentage: 100,
            }],
            source_file_count: 200,
            source_bytes: 2_000_000,
            config_file_count: 50,
            generated_source: false,
            multi_module: false,
            expected_cold_build_s: 5.0,
            expected_warm_update_s: 0.5,
            expected_store_mb: 15.0,
        },
        BenchmarkRepository {
            name: "polyglot-monorepo".to_string(),
            description: "Go + TypeScript + Rust, ~800 source files, ~120 config files".to_string(),
            language_distribution: vec![
                LanguageShare {
                    language: "Go".to_string(),
                    percentage: 40,
                },
                LanguageShare {
                    language: "TypeScript".to_string(),
                    percentage: 35,
                },
                LanguageShare {
                    language: "Rust".to_string(),
                    percentage: 25,
                },
            ],
            source_file_count: 800,
            source_bytes: 8_000_000,
            config_file_count: 120,
            generated_source: true,
            multi_module: true,
            expected_cold_build_s: 30.0,
            expected_warm_update_s: 3.0,
            expected_store_mb: 80.0,
        },
        BenchmarkRepository {
            name: "java-heavy".to_string(),
            description: "Multi-module Maven/Gradle, ~500 source files, generated sources"
                .to_string(),
            language_distribution: vec![LanguageShare {
                language: "Java".to_string(),
                percentage: 100,
            }],
            source_file_count: 500,
            source_bytes: 6_000_000,
            config_file_count: 30,
            generated_source: true,
            multi_module: true,
            expected_cold_build_s: 60.0,
            expected_warm_update_s: 10.0,
            expected_store_mb: 120.0,
        },
        BenchmarkRepository {
            name: "rust-macro-heavy".to_string(),
            description: "Rust workspace with proc macros and feature flags".to_string(),
            language_distribution: vec![LanguageShare {
                language: "Rust".to_string(),
                percentage: 100,
            }],
            source_file_count: 300,
            source_bytes: 4_000_000,
            config_file_count: 20,
            generated_source: false,
            multi_module: true,
            expected_cold_build_s: 20.0,
            expected_warm_update_s: 2.0,
            expected_store_mb: 50.0,
        },
        BenchmarkRepository {
            name: "config-ci-heavy".to_string(),
            description: "Docker, K8s, CI workflows, Makefiles, shell scripts".to_string(),
            language_distribution: vec![LanguageShare {
                language: "Go".to_string(),
                percentage: 60,
            }],
            source_file_count: 100,
            source_bytes: 1_000_000,
            config_file_count: 200,
            generated_source: false,
            multi_module: false,
            expected_cold_build_s: 3.0,
            expected_warm_update_s: 1.0,
            expected_store_mb: 10.0,
        },
    ]
}

// ── Metrics schema ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkMetrics {
    pub repo_name: String,
    pub scenario: BenchmarkScenario,
    // Time
    pub cold_build_total_ms: u64,
    pub warm_update_total_ms: u64,
    pub provider_startup_ms: BTreeMap<String, u64>,
    pub semantic_resolve_ms: u64,
    pub scip_write_ms: u64,
    pub store_import_ms: u64,
    pub query_latency_p50_us: u64,
    pub query_latency_p99_us: u64,
    // Space
    pub provider_peak_rss_mb: BTreeMap<String, f64>,
    pub store_size_mb: f64,
    pub index_size_mb: f64,
    // Quality
    pub fallback_rate: f64,
    pub partial_rate: f64,
    pub stale_prevention_hits: u64,
    pub candidate_count: u64,
    pub resolved_occurrence_count: u64,
    pub config_fact_count: u64,
    // Budget
    pub within_budget: bool,
    pub budget_violations: Vec<String>,
}

use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkScenario {
    ColdBuild,
    WarmUpdate,
    QueryHeavy,
    FailureRecovery,
}

// ── Resource budgets ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceBudget {
    pub max_provider_concurrency: usize,
    pub max_provider_rss_mb: f64,
    pub max_provider_timeout_ms: u64,
    pub max_store_size_mb: f64,
    pub max_total_rss_mb: f64,
    pub non_code_budget_percent: u8,
    pub store_pruning_max_snapshots: usize,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            max_provider_concurrency: 4,
            max_provider_rss_mb: 4096.0,
            max_provider_timeout_ms: 300_000,
            max_store_size_mb: 500.0,
            max_total_rss_mb: 8192.0,
            non_code_budget_percent: 10,
            store_pruning_max_snapshots: 5,
        }
    }
}

// ── PR gate vs full benchmark ───────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrGateConfig {
    pub repos: Vec<String>,
    pub scenarios: Vec<BenchmarkScenario>,
    pub max_cold_build_s: f64,
    pub max_store_growth_mb: f64,
    pub regression_threshold_percent: f64,
}

impl Default for PrGateConfig {
    fn default() -> Self {
        Self {
            repos: vec!["single-go-medium".to_string()],
            scenarios: vec![BenchmarkScenario::ColdBuild, BenchmarkScenario::WarmUpdate],
            max_cold_build_s: 60.0,
            max_store_growth_mb: 50.0,
            regression_threshold_percent: 20.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FullBenchmarkConfig {
    pub repos: Vec<String>,
    pub scenarios: Vec<BenchmarkScenario>,
    pub iterations: usize,
}

impl Default for FullBenchmarkConfig {
    fn default() -> Self {
        Self {
            repos: benchmark_matrix().into_iter().map(|r| r.name).collect(),
            scenarios: vec![
                BenchmarkScenario::ColdBuild,
                BenchmarkScenario::WarmUpdate,
                BenchmarkScenario::QueryHeavy,
                BenchmarkScenario::FailureRecovery,
            ],
            iterations: 3,
        }
    }
}

// ── Report output ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub gate_type: GateType,
    pub passed: bool,
    pub metrics: Vec<BenchmarkMetrics>,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateType {
    Pr,
    Full,
}

impl BenchmarkReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_matrix_has_five_repos() {
        let matrix = benchmark_matrix();
        assert_eq!(matrix.len(), 5);
    }

    #[test]
    fn resource_budget_defaults_are_sensible() {
        let budget = ResourceBudget::default();
        assert!(budget.max_provider_concurrency > 0);
        assert!(budget.max_provider_rss_mb > 0.0);
        assert!(budget.max_total_rss_mb > budget.max_provider_rss_mb);
    }

    #[test]
    fn pr_gate_default_uses_fast_repo() {
        let config = PrGateConfig::default();
        assert!(config.repos.contains(&"single-go-medium".to_string()));
        assert_eq!(config.scenarios.len(), 2);
    }

    #[test]
    fn full_benchmark_covers_all_scenarios() {
        let config = FullBenchmarkConfig::default();
        assert_eq!(config.scenarios.len(), 4);
        assert!(config.iterations >= 2);
    }

    #[test]
    fn benchmark_report_serializes() {
        let report = BenchmarkReport {
            schema_version: 1,
            gate_type: GateType::Pr,
            passed: true,
            metrics: vec![],
            summary: "all passed".to_string(),
        };
        let json = report.to_json();
        assert!(json.contains("passed"));
        assert!(json.contains("pr"));
    }
}

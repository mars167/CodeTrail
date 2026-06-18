use std::collections::BTreeMap;

use serde::Serialize;

use crate::{lsp::scip_gen::SemanticBuildReport, project_graph::ProjectLanguage};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    NativeScip,
    LspBridge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackMode {
    TreeSitterParser,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallHelp {
    pub macos: &'static [&'static str],
    pub linux: &'static [&'static str],
    pub windows: &'static [&'static str],
    pub notes: &'static [&'static str],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRequirement {
    pub language: ProjectLanguage,
    pub provider: &'static str,
    pub kind: ProviderKind,
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub env_key: &'static str,
    pub install: InstallHelp,
    pub fallback: FallbackMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstallHelp {
    pub language: String,
    pub provider: &'static str,
    pub kind: ProviderKind,
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub env_key: &'static str,
    pub install: InstallHelp,
    pub fallback: FallbackMode,
    pub reason: String,
}

pub fn requirement_for_language(language: &ProjectLanguage) -> ProviderRequirement {
    match language {
        ProjectLanguage::Go => ProviderRequirement {
            language: ProjectLanguage::Go,
            provider: "scip-go",
            kind: ProviderKind::NativeScip,
            command: "scip-go",
            args: &[],
            env_key: "CODETRAIL_SCIP_GO",
            install: InstallHelp {
                macos: &["go install github.com/scip-code/scip-go/cmd/scip-go@latest"],
                linux: &["go install github.com/scip-code/scip-go/cmd/scip-go@latest"],
                windows: &["go install github.com/scip-code/scip-go/cmd/scip-go@latest"],
                notes: &["Ensure GOPATH/bin or GOBIN is on PATH, or set CODETRAIL_SCIP_GO."],
            },
            fallback: FallbackMode::TreeSitterParser,
        },
        ProjectLanguage::Rust => ProviderRequirement {
            language: ProjectLanguage::Rust,
            provider: "rust-analyzer-scip",
            kind: ProviderKind::NativeScip,
            command: "rust-analyzer",
            args: &["scip", "."],
            env_key: "CODETRAIL_SCIP_RUST",
            install: InstallHelp {
                macos: &["rustup component add rust-analyzer"],
                linux: &["rustup component add rust-analyzer"],
                windows: &["rustup component add rust-analyzer"],
                notes: &["Set CODETRAIL_SCIP_RUST to override the rust-analyzer command."],
            },
            fallback: FallbackMode::TreeSitterParser,
        },
        ProjectLanguage::Java => ProviderRequirement {
            language: ProjectLanguage::Java,
            provider: "scip-java",
            kind: ProviderKind::NativeScip,
            command: "scip-java",
            args: &["index"],
            env_key: "CODETRAIL_SCIP_JAVA",
            install: InstallHelp {
                macos: &[
                    "brew install coursier/formulas/coursier",
                    "coursier bootstrap --standalone -o scip-java com.sourcegraph:scip-java_2.13:0.12.3 --main com.sourcegraph.scip_java.ScipJava",
                ],
                linux: &[
                    "curl -fLo coursier https://git.io/coursier-cli",
                    "chmod +x coursier",
                    "./coursier bootstrap --standalone -o scip-java com.sourcegraph:scip-java_2.13:0.12.3 --main com.sourcegraph.scip_java.ScipJava",
                ],
                windows: &[
                    "bitsadmin /transfer downloadCoursierCli https://git.io/coursier-cli \"%cd%\\coursier\"",
                    "bitsadmin /transfer downloadCoursierBat https://git.io/coursier-bat \"%cd%\\coursier.bat\"",
                    "coursier bootstrap --standalone -o scip-java com.sourcegraph:scip-java_2.13:0.12.3 --main com.sourcegraph.scip_java.ScipJava",
                ],
                notes: &[
                    "Set CODETRAIL_SCIP_JAVA to override the scip-java command.",
                    "scip-java supports Gradle, Maven, sbt, Bazel, and Mill workflows with different setup levels.",
                ],
            },
            fallback: FallbackMode::TreeSitterParser,
        },
        ProjectLanguage::TypeScript => ProviderRequirement {
            language: ProjectLanguage::TypeScript,
            provider: "scip-typescript",
            kind: ProviderKind::NativeScip,
            command: "scip-typescript",
            args: &["index"],
            env_key: "CODETRAIL_SCIP_TYPESCRIPT",
            install: InstallHelp {
                macos: &["npm install -g @sourcegraph/scip-typescript"],
                linux: &["npm install -g @sourcegraph/scip-typescript"],
                windows: &["npm install -g @sourcegraph/scip-typescript"],
                notes: &[
                    "Run project package installation before indexing.",
                    "Use scip-typescript index --infer-tsconfig for JavaScript projects without tsconfig.json.",
                ],
            },
            fallback: FallbackMode::TreeSitterParser,
        },
        ProjectLanguage::Ruby => ProviderRequirement {
            language: ProjectLanguage::Ruby,
            provider: "scip-ruby",
            kind: ProviderKind::NativeScip,
            command: "scip-ruby",
            args: &["."],
            env_key: "CODETRAIL_SCIP_RUBY",
            install: InstallHelp {
                macos: &["bundle add scip-ruby --group development"],
                linux: &["bundle add scip-ruby --group development"],
                windows: &["No stable upstream binary is available; use parser fallback on Windows."],
                notes: &[
                    "Set CODETRAIL_SCIP_RUBY=\"bundle exec scip-ruby\" when scip-ruby is only available through Bundler.",
                    "Sorbet adoption improves scip-ruby navigation quality.",
                ],
            },
            fallback: FallbackMode::TreeSitterParser,
        },
        ProjectLanguage::Swift => ProviderRequirement {
            language: ProjectLanguage::Swift,
            provider: "sourcekit-lsp",
            kind: ProviderKind::LspBridge,
            command: "sourcekit-lsp",
            args: &[],
            env_key: "CODETRAIL_LSP_SWIFT",
            install: InstallHelp {
                macos: &["Install Xcode or a Swift toolchain that includes sourcekit-lsp."],
                linux: &["Install the Swift toolchain and ensure sourcekit-lsp is on PATH."],
                windows: &["Install the Swift toolchain and ensure sourcekit-lsp.exe is on PATH."],
                notes: &[
                    "Set CODETRAIL_LSP_SWIFT to override the sourcekit-lsp command.",
                    "Swift requires a recent build or background index for complete cross-module results.",
                ],
            },
            fallback: FallbackMode::TreeSitterParser,
        },
    }
}

pub fn install_help_for_semantic_report(report: &SemanticBuildReport) -> Vec<ProviderInstallHelp> {
    let mut by_language = BTreeMap::<String, ProviderInstallHelp>::new();
    for language in &report.languages {
        let Some(reason) = install_help_reason(language) else {
            continue;
        };
        let Some(project_language) = parse_project_language(&language.language) else {
            continue;
        };
        let requirement = requirement_for_language(&project_language);
        by_language
            .entry(language.language.clone())
            .or_insert_with(|| ProviderInstallHelp {
                language: language.language.clone(),
                provider: requirement.provider,
                kind: requirement.kind,
                command: requirement.command,
                args: requirement.args,
                env_key: requirement.env_key,
                install: requirement.install,
                fallback: requirement.fallback,
                reason,
            });
    }
    by_language.into_values().collect()
}

pub fn tty_install_lines(help: &[ProviderInstallHelp]) -> Vec<String> {
    help.iter()
        .map(|item| {
            let args = if item.args.is_empty() {
                String::new()
            } else {
                format!(" {}", item.args.join(" "))
            };
            format!(
                "codetrail: semantic provider missing for {} ({}). Install: {}. Command: {}{}. Fallback: tree-sitter parser. Override with {}.",
                item.language,
                item.provider,
                current_platform_install(&item.install).unwrap_or(item.command),
                item.command,
                args,
                item.env_key
            )
        })
        .collect()
}

fn install_help_reason(language: &crate::lsp::scip_gen::SemanticLanguageReport) -> Option<String> {
    if language.state == "missing" {
        return Some("semantic_provider_missing".to_string());
    }
    language.partial_reasons.iter().find_map(|reason| {
        (reason == "semantic_provider_missing"
            || reason.starts_with("semantic_provider_failed")
            || reason.starts_with("semantic_provider_startup_failed"))
        .then(|| reason.clone())
    })
}

fn current_platform_install(install: &InstallHelp) -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    let commands = install.macos;
    #[cfg(target_os = "linux")]
    let commands = install.linux;
    #[cfg(target_os = "windows")]
    let commands = install.windows;
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let commands = install.macos;

    commands.first().copied()
}

fn parse_project_language(language: &str) -> Option<ProjectLanguage> {
    match language {
        "go" => Some(ProjectLanguage::Go),
        "rust" => Some(ProjectLanguage::Rust),
        "java" => Some(ProjectLanguage::Java),
        "typescript" => Some(ProjectLanguage::TypeScript),
        "ruby" => Some(ProjectLanguage::Ruby),
        "swift" => Some(ProjectLanguage::Swift),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_requirements_match_target_strategy() {
        let cases: &[(ProjectLanguage, &str, &str, &[&str], &str, ProviderKind)] = &[
            (
                ProjectLanguage::Go,
                "scip-go",
                "scip-go",
                &[],
                "CODETRAIL_SCIP_GO",
                ProviderKind::NativeScip,
            ),
            (
                ProjectLanguage::Rust,
                "rust-analyzer-scip",
                "rust-analyzer",
                &["scip", "."],
                "CODETRAIL_SCIP_RUST",
                ProviderKind::NativeScip,
            ),
            (
                ProjectLanguage::Java,
                "scip-java",
                "scip-java",
                &["index"],
                "CODETRAIL_SCIP_JAVA",
                ProviderKind::NativeScip,
            ),
            (
                ProjectLanguage::TypeScript,
                "scip-typescript",
                "scip-typescript",
                &["index"],
                "CODETRAIL_SCIP_TYPESCRIPT",
                ProviderKind::NativeScip,
            ),
            (
                ProjectLanguage::Ruby,
                "scip-ruby",
                "scip-ruby",
                &["."],
                "CODETRAIL_SCIP_RUBY",
                ProviderKind::NativeScip,
            ),
            (
                ProjectLanguage::Swift,
                "sourcekit-lsp",
                "sourcekit-lsp",
                &[],
                "CODETRAIL_LSP_SWIFT",
                ProviderKind::LspBridge,
            ),
        ];

        for (language, provider, command, args, env_key, kind) in cases {
            let requirement = requirement_for_language(language);
            assert_eq!(requirement.provider, *provider);
            assert_eq!(requirement.command, *command);
            assert_eq!(requirement.args, *args);
            assert_eq!(requirement.env_key, *env_key);
            assert_eq!(requirement.kind, *kind);
            assert_eq!(requirement.fallback, FallbackMode::TreeSitterParser);
        }
    }

    #[test]
    fn java_provider_is_native_scip_with_parser_fallback() {
        let requirement = requirement_for_language(&ProjectLanguage::Java);
        assert_eq!(requirement.provider, "scip-java");
        assert_eq!(requirement.env_key, "CODETRAIL_SCIP_JAVA");
        assert_eq!(requirement.kind, ProviderKind::NativeScip);
        assert_eq!(requirement.fallback, FallbackMode::TreeSitterParser);
        assert!(requirement
            .install
            .macos
            .iter()
            .any(|line| line.contains("scip-java_2.13")));
    }

    #[test]
    fn semantic_report_missing_java_gets_scip_java_install_help() {
        let report = crate::lsp::scip_gen::SemanticBuildReport {
            attempted: true,
            skipped: false,
            skip_reason: None,
            scip: None,
            languages: vec![crate::lsp::scip_gen::SemanticLanguageReport {
                language: "java".to_string(),
                root_id: "java:.".to_string(),
                provider: Some("scip-java".to_string()),
                state: "missing".to_string(),
                occurrence_count: 0,
                partial_reasons: vec!["semantic_provider_missing".to_string()],
            }],
        };
        let help = install_help_for_semantic_report(&report);
        assert_eq!(help.len(), 1);
        assert_eq!(help[0].language, "java");
        assert_eq!(help[0].provider, "scip-java");
        assert_eq!(help[0].env_key, "CODETRAIL_SCIP_JAVA");
        assert_eq!(help[0].fallback, FallbackMode::TreeSitterParser);
    }
}

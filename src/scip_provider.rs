use std::{
    env, fs,
    io::Read,
    path::PathBuf,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

use crate::{
    index,
    lsp::registry::resolve_binary,
    output::VerboseLogger,
    project_graph::{ProjectLanguage, ProjectRoot},
    provider_help::{requirement_for_language, ProviderKind, ProviderRequirement},
    scip,
    scip_proto::proto,
    workspace::Workspace,
};

#[derive(Clone, Debug, PartialEq)]
pub enum NativeScipOutcome {
    Generated {
        provider: &'static str,
        index_path: PathBuf,
        index: proto::Index,
    },
    Missing {
        requirement: ProviderRequirement,
    },
    Failed {
        provider: &'static str,
        message: String,
    },
    NotNative,
}

pub fn run_native_provider(
    workspace: &Workspace,
    root: &ProjectRoot,
    verbose: VerboseLogger,
    timeout: Duration,
) -> Result<NativeScipOutcome> {
    let requirement = requirement_for_language(&root.language);
    if requirement.kind != ProviderKind::NativeScip {
        return Ok(NativeScipOutcome::NotNative);
    }

    let Some(mut provider_command) = resolve_provider_command(&requirement) else {
        return Ok(NativeScipOutcome::Missing { requirement });
    };

    let root_dir = if root.path == "." {
        workspace.root.clone()
    } else {
        workspace.root.join(&root.path)
    };
    let output_path = provider_output_path(workspace, root, &requirement);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let stderr_path = output_path.with_extension("stderr.log");
    let _ = fs::remove_file(&output_path);
    let _ = fs::remove_file(&stderr_path);
    provider_command
        .args
        .extend(output_args(&root.language, &output_path));

    verbose.log(format!(
        "semantic: indexing root {} via {}",
        root.id, requirement.provider
    ));

    let output = match run_provider_command(&provider_command, &root_dir, &stderr_path, timeout) {
        Ok(output) => output,
        Err(error) => {
            verbose.log(format!(
                "semantic: provider {} failed to start: {error}",
                requirement.provider
            ));
            return Ok(NativeScipOutcome::Failed {
                provider: requirement.provider,
                message: "provider_start_failed".to_string(),
            });
        }
    };

    if output.timed_out {
        verbose.log(format!(
            "semantic: provider {} timed out after {}ms",
            requirement.provider,
            timeout.as_millis()
        ));
        return Ok(NativeScipOutcome::Failed {
            provider: requirement.provider,
            message: "provider_timeout".to_string(),
        });
    }

    if !output.status.is_success() {
        let detail = provider_stderr_summary(&output.stderr);
        verbose.log(format!(
            "semantic: provider {} exited with status {}{}",
            requirement.provider, output.status, detail
        ));
        return Ok(NativeScipOutcome::Failed {
            provider: requirement.provider,
            message: "provider_failed".to_string(),
        });
    }

    if !output_path.exists() {
        verbose.log(format!(
            "semantic: provider {} did not create {}",
            requirement.provider,
            output_path.display()
        ));
        return Ok(NativeScipOutcome::Failed {
            provider: requirement.provider,
            message: "provider_output_missing".to_string(),
        });
    }

    let native_index = match scip::parse_native_scip(&output_path)
        .with_context(|| format!("failed to parse {}", output_path.display()))
    {
        Ok(index) => index,
        Err(error) => {
            verbose.log(format!(
                "semantic: provider {} wrote invalid SCIP output: {error}",
                requirement.provider
            ));
            return Ok(NativeScipOutcome::Failed {
                provider: requirement.provider,
                message: "provider_output_invalid".to_string(),
            });
        }
    };
    let native_index = normalize_index_paths(native_index, &root.path);

    Ok(NativeScipOutcome::Generated {
        provider: requirement.provider,
        index_path: output_path,
        index: native_index,
    })
}

struct ProviderCommand {
    program: String,
    args: Vec<String>,
}

struct ProviderProcessOutput {
    status: ProviderExitStatus,
    stderr: String,
    timed_out: bool,
}

#[derive(Clone, Copy)]
enum ProviderExitStatus {
    Exited(ExitStatus),
    TimedOut,
}

impl ProviderExitStatus {
    fn is_success(self) -> bool {
        match self {
            Self::Exited(status) => status.success(),
            Self::TimedOut => false,
        }
    }
}

impl std::fmt::Display for ProviderExitStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exited(status) => write!(f, "{status}"),
            Self::TimedOut => write!(f, "timeout"),
        }
    }
}

pub fn merge_native_indexes(indexes: Vec<proto::Index>) -> proto::Index {
    let mut indexes = indexes.into_iter();
    let mut merged = indexes.next().unwrap_or_default();
    for mut index in indexes {
        merged.documents.append(&mut index.documents);
        merged.external_symbols.append(&mut index.external_symbols);
    }
    merged
}

fn resolve_provider_command(requirement: &ProviderRequirement) -> Option<ProviderCommand> {
    if let Some(value) = env::var(requirement.env_key)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        let mut words = shell_words(&value).into_iter();
        let program = resolve_binary(&words.next()?)?;
        let mut args = words.collect::<Vec<_>>();
        args.extend(requirement.args.iter().map(|arg| (*arg).to_string()));
        return Some(ProviderCommand { program, args });
    }

    Some(ProviderCommand {
        program: resolve_binary(requirement.command)?,
        args: requirement
            .args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect(),
    })
}

fn provider_output_path(
    workspace: &Workspace,
    root: &ProjectRoot,
    requirement: &ProviderRequirement,
) -> PathBuf {
    index::scip_root(workspace)
        .join("provider-output")
        .join(format!(
            "{}-{}.scip",
            safe_fragment(&root.id),
            safe_fragment(requirement.provider)
        ))
}

fn safe_fragment(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn output_args(language: &ProjectLanguage, output_path: &std::path::Path) -> Vec<String> {
    let output = output_path.to_string_lossy().to_string();
    match language {
        ProjectLanguage::Ruby => vec!["--index-file".to_string(), output],
        _ => vec!["--output".to_string(), output],
    }
}

fn run_provider_command(
    provider_command: &ProviderCommand,
    root_dir: &std::path::Path,
    stderr_path: &std::path::Path,
    timeout: Duration,
) -> std::io::Result<ProviderProcessOutput> {
    let stderr_file = fs::File::create(stderr_path)?;
    let mut child = Command::new(&provider_command.program)
        .args(&provider_command.args)
        .current_dir(root_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()?;

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(ProviderProcessOutput {
                status: ProviderExitStatus::Exited(status),
                stderr: read_stderr(stderr_path),
                timed_out: false,
            });
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(ProviderProcessOutput {
                status: ProviderExitStatus::TimedOut,
                stderr: read_stderr(stderr_path),
                timed_out: true,
            });
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

fn read_stderr(stderr_path: &std::path::Path) -> String {
    let mut stderr = String::new();
    if let Ok(file) = fs::File::open(stderr_path) {
        let _ = file.take(16 * 1024).read_to_string(&mut stderr);
    }
    stderr
}

fn provider_stderr_summary(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        let mut summary = trimmed.replace(['\r', '\n'], " ");
        summary.truncate(500);
        format!(": {summary}")
    }
}

fn shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut word_started = false;
    let mut quote = None;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            Some(active_quote) if ch == active_quote => {
                quote = None;
            }
            Some(active_quote) if ch == '\\' => {
                if let Some(&next) = chars.peek() {
                    if next == active_quote || next == '\\' {
                        current.push(chars.next().unwrap());
                    } else {
                        current.push(ch);
                    }
                } else {
                    current.push(ch);
                }
            }
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => {
                quote = Some(ch);
                word_started = true;
            }
            None if ch.is_whitespace() => {
                if word_started {
                    words.push(std::mem::take(&mut current));
                    word_started = false;
                }
            }
            None if ch == '\\' => {
                if let Some(&next) = chars.peek() {
                    if next.is_whitespace() || next == '\'' || next == '"' || next == '\\' {
                        current.push(chars.next().unwrap());
                    } else {
                        current.push(ch);
                    }
                } else {
                    current.push(ch);
                }
                word_started = true;
            }
            None => {
                current.push(ch);
                word_started = true;
            }
        }
    }

    if word_started {
        words.push(current);
    }
    words
}

fn normalize_index_paths(mut index: proto::Index, root_path: &str) -> proto::Index {
    for document in &mut index.documents {
        document.relative_path = normalize_document_path(root_path, &document.relative_path);
    }
    index
}

fn normalize_document_path(root_path: &str, relative_path: &str) -> String {
    let path = relative_path
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string();
    let root_path = root_path.replace('\\', "/");
    let root_path = root_path.trim_matches('/');
    if root_path.is_empty()
        || root_path == "."
        || path == root_path
        || path.starts_with(&format!("{root_path}/"))
    {
        path
    } else {
        format!("{root_path}/{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_index_paths_are_workspace_relative() {
        let index = proto::Index {
            documents: vec![proto::Document {
                relative_path: "src/main/java/example/App.java".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let normalized = normalize_index_paths(index, "java");

        assert_eq!(
            normalized.documents[0].relative_path,
            "java/src/main/java/example/App.java"
        );
    }

    #[test]
    fn merge_native_indexes_keeps_all_documents() {
        let first = proto::Index {
            documents: vec![proto::Document {
                relative_path: "a.go".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let second = proto::Index {
            documents: vec![proto::Document {
                relative_path: "b.rs".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let merged = merge_native_indexes(vec![first, second]);

        let paths = merged
            .documents
            .iter()
            .map(|document| document.relative_path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["a.go", "b.rs"]);
    }

    #[test]
    fn provider_output_path_stays_under_codetrail_storage() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::discover(dir.path()).unwrap();
        let root = ProjectRoot {
            id: "java:service/api".to_string(),
            path: "service/api".to_string(),
            language: ProjectLanguage::Java,
            kind: crate::project_graph::ProjectRootKind::JavaMaven,
            markers: Vec::new(),
        };
        let requirement = requirement_for_language(&ProjectLanguage::Java);

        let output_path = provider_output_path(&workspace, &root, &requirement);

        assert!(output_path.starts_with(index::scip_root(&workspace)));
        assert!(output_path.ends_with("java-service-api-scip-java.scip"));
        assert!(!output_path.starts_with(dir.path().join("service/api")));
    }

    #[test]
    fn provider_commands_append_owned_output_flags() {
        let dir = tempfile::tempdir().unwrap();
        let ruby_output = dir.path().join("ruby.scip");
        let java_output = dir.path().join("java.scip");

        assert_eq!(
            output_args(&ProjectLanguage::Ruby, &ruby_output),
            vec![
                "--index-file".to_string(),
                ruby_output.to_string_lossy().to_string()
            ]
        );
        assert_eq!(
            output_args(&ProjectLanguage::Java, &java_output),
            vec![
                "--output".to_string(),
                java_output.to_string_lossy().to_string()
            ]
        );
    }

    #[test]
    fn shell_words_preserve_quoted_env_override() {
        assert_eq!(
            shell_words("\"/tmp/scip ruby\" --flag value"),
            vec!["/tmp/scip ruby", "--flag", "value"]
        );
    }
}

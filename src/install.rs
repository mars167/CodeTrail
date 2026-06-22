use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::{
    cli::SkillScope,
    lsp::registry::resolve_binary,
    project_graph::{discover_project_graph, ProjectLanguage},
    provider_help::{
        current_platform_install_commands, env_keys_for_requirement, requirement_for_language,
        requirement_for_language_name, ProviderRequirement,
    },
    workspace::Workspace,
};

pub struct IndexProviderInstallOptions {
    pub languages: Vec<String>,
    pub dry_run: bool,
    pub force: bool,
}

pub struct SkillInstallOptions {
    pub target: String,
    pub scope: SkillScope,
    pub path: Option<String>,
    pub dry_run: bool,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillTargetOption {
    pub id: &'static str,
    pub label: &'static str,
}

pub fn skill_target_options() -> Vec<SkillTargetOption> {
    SKILL_TARGETS
        .iter()
        .map(|target| SkillTargetOption {
            id: target.id,
            label: target.label,
        })
        .collect()
}

pub fn install_index_providers(
    workspace: &Workspace,
    options: &IndexProviderInstallOptions,
) -> Result<(Value, i32)> {
    let requirements = provider_requirements(workspace, &options.languages)?;
    let mut results = Vec::new();
    let mut exit_code = 0;

    for requirement in requirements {
        let available_before = provider_available(&requirement);
        let commands = current_platform_install_commands(&requirement.install);
        let mut command_results = Vec::new();
        let mut status = if available_before && !options.force {
            "skipped_available"
        } else if options.dry_run {
            "planned"
        } else {
            "installed"
        };

        if !available_before || options.force {
            if options.dry_run {
                command_results = commands
                    .iter()
                    .map(|command| {
                        json!({
                            "command": command,
                            "status": "planned",
                            "exitCode": Value::Null,
                        })
                    })
                    .collect();
            } else {
                for command in commands {
                    let command_result = run_shell_command(command, &workspace.root);
                    if !command_result.success {
                        status = "failed";
                        exit_code = 1;
                    }
                    command_results.push(command_result.to_json(command));
                    if status == "failed" {
                        break;
                    }
                }
            }
        }

        let available_after = provider_available(&requirement);
        if status == "installed" && !available_after {
            status = "installed_not_on_path";
        }
        if status == "installed_not_on_path" {
            exit_code = 1;
        }

        results.push(json!({
            "language": requirement.language.to_string(),
            "provider": requirement.provider,
            "command": requirement.command,
            "args": requirement.args,
            "envKey": requirement.env_key,
            "status": status,
            "dryRun": options.dry_run,
            "force": options.force,
            "availableBefore": available_before,
            "availableAfter": available_after,
            "installCommands": commands,
            "steps": command_results,
        }));
    }

    Ok((Value::Array(results), exit_code))
}

pub fn install_skill(workspace: &Workspace, options: &SkillInstallOptions) -> Result<Value> {
    let target = skill_target(&options.target)?;
    let base = skill_base_dir(workspace, options)?;
    let mut files = Vec::new();
    let mut changed = false;

    for asset in target.files {
        let destination = join_segments(
            &base,
            match options.scope {
                SkillScope::User => asset.user_path,
                SkillScope::Project => asset.project_path,
            },
        );
        let existed = destination.exists();
        let mut file_status = if existed && !options.force {
            "skipped_exists"
        } else if options.dry_run {
            "planned"
        } else {
            "installed"
        };

        if !options.dry_run && !(existed && !options.force) {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create skill destination {}", parent.display())
                })?;
            }
            fs::write(&destination, asset.content)
                .with_context(|| format!("failed to write skill file {}", destination.display()))?;
            changed = true;
            file_status = if existed { "overwritten" } else { "installed" };
        }

        files.push(json!({
            "target": target.id,
            "label": target.label,
            "scope": skill_scope_name(options.scope),
            "destination": destination,
            "status": file_status,
            "existed": existed,
        }));
    }

    Ok(json!([{
        "target": target.id,
        "label": target.label,
        "scope": skill_scope_name(options.scope),
        "baseDir": base,
        "dryRun": options.dry_run,
        "force": options.force,
        "changed": changed,
        "files": files,
    }]))
}

fn provider_requirements(
    workspace: &Workspace,
    language_args: &[String],
) -> Result<Vec<ProviderRequirement>> {
    let mut by_language = BTreeMap::<ProjectLanguage, ProviderRequirement>::new();
    if language_args.is_empty() {
        let graph = discover_project_graph(&workspace.root)?;
        for root in graph.roots {
            by_language
                .entry(root.language.clone())
                .or_insert_with(|| requirement_for_language(&root.language));
        }
    } else {
        for language in language_args {
            if language == "all" {
                for requirement in all_provider_requirements() {
                    by_language
                        .entry(requirement.language.clone())
                        .or_insert(requirement);
                }
                continue;
            }
            let requirement = requirement_for_language_name(language)
                .ok_or_else(|| anyhow!("unsupported index provider language: {language}"))?;
            by_language
                .entry(requirement.language.clone())
                .or_insert(requirement);
        }
    }

    Ok(by_language.into_values().collect())
}

fn all_provider_requirements() -> Vec<ProviderRequirement> {
    [
        ProjectLanguage::Go,
        ProjectLanguage::Rust,
        ProjectLanguage::Java,
        ProjectLanguage::Kotlin,
        ProjectLanguage::TypeScript,
        ProjectLanguage::Ruby,
        ProjectLanguage::Swift,
    ]
    .iter()
    .map(requirement_for_language)
    .collect()
}

fn provider_available(requirement: &ProviderRequirement) -> bool {
    let Some(program) = provider_program(requirement) else {
        return false;
    };
    resolve_binary(&program).is_some()
}

fn provider_program(requirement: &ProviderRequirement) -> Option<String> {
    for key in env_keys_for_requirement(requirement) {
        if let Some(value) = env::var(key).ok().filter(|value| !value.trim().is_empty()) {
            return first_shell_word(&value);
        }
    }
    Some(requirement.command.to_string())
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

struct CommandResult {
    success: bool,
    exit_code: Option<i32>,
}

impl CommandResult {
    fn to_json(&self, command: &str) -> Value {
        json!({
            "command": command,
            "status": if self.success { "ok" } else { "failed" },
            "exitCode": self.exit_code,
        })
    }
}

fn run_shell_command(command: &str, cwd: &Path) -> CommandResult {
    let output = shell_command(command).current_dir(cwd).output();
    match output {
        Ok(output) => {
            forward_command_output(&output.stdout);
            forward_command_output(&output.stderr);
            CommandResult {
                success: output.status.success(),
                exit_code: output.status.code(),
            }
        }
        Err(_) => CommandResult {
            success: false,
            exit_code: None,
        },
    }
}

fn forward_command_output(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(bytes);
    if !bytes.ends_with(b"\n") {
        let _ = stderr.write_all(b"\n");
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut process = Command::new("cmd");
        process.args(["/C", command]);
        process
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut process = Command::new("sh");
        process.args(["-c", command]);
        process
    }
}

fn skill_base_dir(workspace: &Workspace, options: &SkillInstallOptions) -> Result<PathBuf> {
    match options.scope {
        SkillScope::Project => Ok(options
            .path
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.root.clone())),
        SkillScope::User => home_dir(),
    }
}

fn join_segments(base: &Path, segments: &[&str]) -> PathBuf {
    let mut path = base.to_path_buf();
    for segment in segments {
        path.push(segment);
    }
    path
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("cannot determine user home directory"))
}

fn skill_scope_name(scope: SkillScope) -> &'static str {
    match scope {
        SkillScope::User => "user",
        SkillScope::Project => "project",
    }
}

struct SkillTarget {
    id: &'static str,
    label: &'static str,
    files: &'static [SkillAsset],
}

struct SkillAsset {
    content: &'static str,
    user_path: &'static [&'static str],
    project_path: &'static [&'static str],
}

const SKILL: &str = include_str!("../skills/codetrail/SKILL.md");

const CODEX_FILES: &[SkillAsset] = &[SkillAsset {
    content: SKILL,
    user_path: &[".codex", "skills", "codetrail", "SKILL.md"],
    project_path: &[".codex", "skills", "codetrail", "SKILL.md"],
}];

const CLAUDE_FILES: &[SkillAsset] = &[SkillAsset {
    content: SKILL,
    user_path: &[".claude", "skills", "codetrail", "SKILL.md"],
    project_path: &[".claude", "skills", "codetrail", "SKILL.md"],
}];

const CURSOR_FILES: &[SkillAsset] = &[SkillAsset {
    content: SKILL,
    user_path: &[".cursor", "rules", "codetrail.mdc"],
    project_path: &[".cursor", "rules", "codetrail.mdc"],
}];

const CONTINUE_FILES: &[SkillAsset] = &[SkillAsset {
    content: SKILL,
    user_path: &[".continue", "rules", "codetrail.md"],
    project_path: &[".continue", "rules", "codetrail.md"],
}];

const CLINE_FILES: &[SkillAsset] = &[SkillAsset {
    content: SKILL,
    user_path: &[".cline", "rules", "codetrail.md"],
    project_path: &[".clinerules", "codetrail.md"],
}];

const ROO_FILES: &[SkillAsset] = &[SkillAsset {
    content: SKILL,
    user_path: &[".roo", "rules", "codetrail.md"],
    project_path: &[".roo", "rules", "codetrail.md"],
}];

const SKILL_TARGETS: &[SkillTarget] = &[
    SkillTarget {
        id: "codex",
        label: "Codex",
        files: CODEX_FILES,
    },
    SkillTarget {
        id: "claude",
        label: "Claude Code",
        files: CLAUDE_FILES,
    },
    SkillTarget {
        id: "cursor",
        label: "Cursor",
        files: CURSOR_FILES,
    },
    SkillTarget {
        id: "continue",
        label: "Continue",
        files: CONTINUE_FILES,
    },
    SkillTarget {
        id: "cline",
        label: "Cline",
        files: CLINE_FILES,
    },
    SkillTarget {
        id: "roo",
        label: "Roo",
        files: ROO_FILES,
    },
];

fn skill_target(id: &str) -> Result<&'static SkillTarget> {
    SKILL_TARGETS
        .iter()
        .find(|target| target.id == id)
        .ok_or_else(|| anyhow!("unknown skill target: {id}"))
}

use std::io::{self, Write};

use serde_json::Value;

use crate::provider_help::{current_platform_install_commands, requirement_for_language_name};

pub(super) fn render_text_status_like(
    command: &str,
    results: &[Value],
    out: &mut dyn Write,
) -> io::Result<()> {
    for result in results {
        match command {
            "status" => {
                let root = result.get("root").and_then(Value::as_str).unwrap_or("");
                if !root.is_empty() {
                    writeln!(out, "Workspace: {root}")?;
                }
                if let Some(head) = result.get("head").and_then(Value::as_str) {
                    writeln!(out, "Head: {head}")?;
                }
                let dirty = result
                    .get("dirty")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let staged = result
                    .get("stagedCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let worktree = result
                    .get("worktreeCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                writeln!(out, "Dirty: {dirty} (staged {staged}, worktree {worktree})")?;
            }
            "index status" | "index verify" => render_index_status_result(result, out)?,
            "index build" | "index update" => render_index_build_result(result, out)?,
            "index skipped" => render_index_skipped_result(result, out)?,
            "index pack" => render_index_pack_result(result, out)?,
            "index unpack" => render_index_unpack_result(result, out)?,
            "index clean" => render_index_clean_result(result, out)?,
            "index-provider install" => render_index_provider_install_result(result, out)?,
            "skill install" => render_skill_install_result(result, out)?,
            "hooks install" | "hooks uninstall" | "hooks status" => {
                render_hook_result(command, result, out)?
            }
            _ => writeln!(out, "{}", one_line_json(result))?,
        }
    }
    Ok(())
}

fn render_index_skipped_result(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let count = result.get("count").and_then(Value::as_u64).unwrap_or(0);
    writeln!(out, "Skipped files: {count}")?;
    if let Some(path) = result.get("path").and_then(Value::as_str) {
        writeln!(out, "Path: {path}")?;
    }
    let Some(items) = result.get("items").and_then(Value::as_array) else {
        return Ok(());
    };
    for item in items {
        let path = item.get("path").and_then(Value::as_str).unwrap_or("");
        let reason = item
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("skipped");
        let stage = item.get("stage").and_then(Value::as_str).unwrap_or("scan");
        if let Some(message) = item.get("message").and_then(Value::as_str) {
            writeln!(out, "{path}  {reason} ({stage}): {message}")?;
        } else {
            writeln!(out, "{path}  {reason} ({stage})")?;
        }
    }
    Ok(())
}

fn render_index_status_result(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let exists = result.get("exists").and_then(Value::as_bool);
    let fresh = result.get("fresh").and_then(Value::as_bool);
    if let Some(exists) = exists {
        writeln!(out, "Index exists: {exists}")?;
    }
    if let Some(fresh) = fresh {
        writeln!(out, "Index fresh: {fresh}")?;
    }
    if let Some(path) = result.get("path").and_then(Value::as_str) {
        writeln!(out, "Path: {path}")?;
    }
    if let Some(file_count) = result
        .pointer("/manifest/fileCount")
        .and_then(Value::as_u64)
    {
        writeln!(out, "Files: {file_count}")?;
    }
    if let Some(reason) = result.get("reason").and_then(Value::as_str) {
        writeln!(out, "Reason: {reason}")?;
    }
    render_indexed_languages(result, out)?;
    render_semantic_status(result, out)?;
    Ok(())
}

fn render_indexed_languages(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let Some(languages) = result.get("indexedLanguages").and_then(Value::as_array) else {
        return Ok(());
    };
    let rendered = language_counts(languages, "fileCount");
    if !rendered.is_empty() {
        writeln!(out, "Indexed languages: {}", rendered.join(", "))?;
    }
    Ok(())
}

fn render_semantic_status(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let Some(status) = result.get("semanticStatus") else {
        return Ok(());
    };
    if let Some(scip_index) = status.get("scipIndex") {
        let state = scip_index
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let enabled = scip_index
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let usable = scip_index
            .get("usable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        writeln!(
            out,
            "SCIP index: {state} (enabled: {enabled}, usable: {usable})"
        )?;
        let languages = scip_index
            .get("languages")
            .and_then(Value::as_array)
            .map(|languages| language_counts(languages, "symbolCount"))
            .unwrap_or_default();
        if !languages.is_empty() {
            writeln!(out, "SCIP languages: {}", languages.join(", "))?;
        }
    }

    let (servers, label) =
        if let Some(providers) = status.get("semanticProviders").and_then(Value::as_array) {
            (providers, "Semantic providers")
        } else if let Some(servers) = status.get("languageServers").and_then(Value::as_array) {
            (servers, "Language servers")
        } else {
            return Ok(());
        };
    if servers.is_empty() {
        writeln!(out, "{label}: none required by discovered roots")?;
        return Ok(());
    }
    writeln!(out, "{label}:")?;
    for server in servers {
        let language = server
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let status = server
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let default_command = server
            .get("defaultCommand")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let missing = server
            .get("missingDependencies")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        if missing.is_empty() {
            writeln!(out, "  {language}: {status} ({default_command})")?;
        } else {
            writeln!(
                out,
                "  {language}: {status} ({default_command}; missing: {missing})"
            )?;
            render_provider_install_help(language, out)?;
        }
    }
    Ok(())
}

fn render_provider_install_help(language: &str, out: &mut dyn Write) -> io::Result<()> {
    let Some(requirement) = requirement_for_language_name(language) else {
        return Ok(());
    };
    let commands = current_platform_install_commands(&requirement.install);
    if !commands.is_empty() {
        writeln!(out, "    Install:")?;
        for command in commands {
            writeln!(out, "      {command}")?;
        }
    }
    let args = if requirement.args.is_empty() {
        String::new()
    } else {
        format!(" {}", requirement.args.join(" "))
    };
    writeln!(out, "    Command: {}{args}", requirement.command)?;
    writeln!(out, "    Override: {}", requirement.env_key)?;
    writeln!(out, "    Fallback: tree-sitter parser")?;
    Ok(())
}

fn language_counts(languages: &[Value], count_field: &str) -> Vec<String> {
    languages
        .iter()
        .filter_map(|language| {
            let name = language.get("language").and_then(Value::as_str)?;
            let count = language.get(count_field).and_then(Value::as_u64)?;
            Some(format!("{name}={count}"))
        })
        .collect()
}

fn render_index_build_result(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let index = result.get("index").unwrap_or(result);
    if result.get("updated").and_then(Value::as_bool) == Some(false) {
        writeln!(out, "Index already fresh")?;
    }
    if let Some(file_count) = index.get("fileCount").and_then(Value::as_u64) {
        writeln!(out, "Indexed {file_count} files")?;
    }
    if let Some(storage) = index.get("storageBackend").and_then(Value::as_str) {
        writeln!(out, "Backend: {storage}")?;
    }
    if let Some(path) = index.get("path").and_then(Value::as_str) {
        writeln!(out, "Path: {path}")?;
    }
    if index.get("fileCount").is_none() {
        writeln!(out, "{}", one_line_json(result))?;
    }
    Ok(())
}

fn render_index_pack_result(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let output_path = result
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or("archive");
    let entry_count = result
        .get("entryCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let archive_size = result
        .get("archiveSize")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    writeln!(out, "Packed index to {output_path}")?;
    if entry_count > 0 || archive_size > 0 {
        writeln!(out, "Entries: {entry_count}, bytes: {archive_size}")?;
    }
    Ok(())
}

fn render_index_unpack_result(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    if let Some(snapshot_id) = result.get("remote_snapshot_id").and_then(Value::as_str) {
        writeln!(out, "Unpacked remote snapshot {snapshot_id}")?;
    } else {
        writeln!(out, "Unpacked remote snapshot")?;
    }
    if let Some(remote_dir) = result.get("remoteDir").and_then(Value::as_str) {
        writeln!(out, "Path: {remote_dir}")?;
    }
    if let Some(entry_count) = result.get("entryCount").and_then(Value::as_u64) {
        writeln!(out, "Entries: {entry_count}")?;
    }
    Ok(())
}

fn render_index_clean_result(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let cleaned = result
        .get("cleaned")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    writeln!(out, "Index cleaned: {cleaned}")?;
    if let Some(path) = result.get("path").and_then(Value::as_str) {
        writeln!(out, "Path: {path}")?;
    }
    Ok(())
}

fn render_index_provider_install_result(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let language = result
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let provider = result
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    writeln!(
        out,
        "{language}: {provider} ({})",
        display_index_provider_install_status(status)
    )?;
    if status == "skipped_available" {
        writeln!(
            out,
            "  Already available; use --force to run install commands anyway."
        )?;
    }
    if let Some(commands) = result.get("installCommands").and_then(Value::as_array) {
        if !commands.is_empty() {
            writeln!(out, "  Install commands:")?;
            for command in commands.iter().filter_map(Value::as_str) {
                writeln!(out, "    {command}")?;
            }
        }
    }
    if let Some(steps) = result.get("steps").and_then(Value::as_array) {
        if !steps.is_empty() {
            writeln!(out, "  Steps:")?;
            for step in steps {
                let command = step.get("command").and_then(Value::as_str).unwrap_or("");
                let step_status = step
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                writeln!(out, "    {step_status}: {command}")?;
            }
        }
    }
    Ok(())
}

fn display_index_provider_install_status(status: &str) -> &str {
    match status {
        "skipped_available" => "already available; skipped",
        other => other,
    }
}

fn render_skill_install_result(result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let target = result
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let scope = result
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let changed = result
        .get("changed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    writeln!(out, "Skill target: {target} ({scope}, changed: {changed})")?;
    if let Some(files) = result.get("files").and_then(Value::as_array) {
        for file in files {
            let destination = file
                .get("destination")
                .and_then(Value::as_str)
                .unwrap_or("");
            let status = file
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            writeln!(out, "  {status}: {destination}")?;
        }
    }
    Ok(())
}

fn render_hook_result(command: &str, result: &Value, out: &mut dyn Write) -> io::Result<()> {
    let hook = result
        .get("hook")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let state = result
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_else(|| hook_state_from_legacy_fields(command, result));
    let path = result.get("path").and_then(Value::as_str).unwrap_or("");
    let mut label = state.to_string();

    if command == "hooks install" {
        if let Some(previous_state) = result.get("previousState").and_then(Value::as_str) {
            if state != "unchanged" && previous_state != "missing" {
                label = format!("{state} (was {previous_state})");
            }
        }
    } else if command == "hooks status" && state == "unmanaged" {
        label = "unmanaged (not owned by codetrail)".to_string();
    }

    if path.is_empty() {
        writeln!(out, "{hook}: {label}")?;
    } else {
        writeln!(out, "{hook}: {label}  {path}")?;
    }
    Ok(())
}

fn hook_state_from_legacy_fields<'a>(command: &str, result: &'a Value) -> &'a str {
    match command {
        "hooks status" => {
            if result
                .get("installed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "installed"
            } else {
                "missing"
            }
        }
        "hooks uninstall" => {
            if result
                .get("removed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "removed"
            } else {
                "skipped"
            }
        }
        _ => "updated",
    }
}

pub(super) fn is_status_like(command: &str) -> bool {
    matches!(
        command,
        "status"
            | "index status"
            | "index verify"
            | "index build"
            | "index update"
            | "index skipped"
            | "index pack"
            | "index unpack"
            | "index clean"
            | "index-provider install"
            | "skill install"
            | "hooks install"
            | "hooks uninstall"
            | "hooks status"
    )
}

fn one_line_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

use std::io::{self, Write};

use serde_json::Value;

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

    let Some(servers) = status.get("languageServers").and_then(Value::as_array) else {
        return Ok(());
    };
    if servers.is_empty() {
        writeln!(out, "Language servers: none required by discovered roots")?;
        return Ok(());
    }
    writeln!(out, "Language servers:")?;
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
        }
    }
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
    )
}

fn one_line_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

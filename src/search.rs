use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::Path,
};

use anyhow::{anyhow, Context, Result};
use globset::Glob;
use regex::Regex;
use serde_json::{json, Value};

use crate::{
    index,
    workspace::{language_for_path, FileCatalogRecord, FileRecord, ScanOptions, Workspace},
};

const MAX_FULL_READ_BYTES: usize = 64 * 1024;

pub struct QueryOutput {
    pub results: Value,
    pub index: Value,
}

pub fn files(
    workspace: &Workspace,
    opts: &ScanOptions,
    pattern: &str,
    strict_glob: bool,
) -> Result<QueryOutput> {
    let mut results = Vec::new();
    let matcher = if strict_glob || has_glob_meta(pattern) {
        Some(Glob::new(pattern)?.compile_matcher())
    } else {
        None
    };

    let source = candidate_file_catalog(workspace, opts)?;
    for file in source.records {
        let matches = matcher
            .as_ref()
            .map(|glob| glob.is_match(&file.path))
            .unwrap_or_else(|| file.path.contains(pattern));
        if matches {
            results.push(json!({
                "path": file.path,
                "language": file.language,
                "size": file.size,
                "hash": file.hash,
                "producer": if source.index["used"].as_bool().unwrap_or(false) { "text_index_file_catalog" } else { "live_file_catalog" },
                "reliability": "source_fact",
                "exact": true
            }));
        }
        if opts.limit > 0 && results.len() >= opts.limit {
            break;
        }
    }
    Ok(QueryOutput {
        results: Value::Array(results),
        index: source.index,
    })
}

pub fn list(workspace: &Workspace, dir: Option<&str>, recursive: bool) -> Result<Value> {
    let rel_dir = dir.unwrap_or(".");
    let base = workspace.abs_path(rel_dir);
    if !base.exists() {
        return Err(anyhow!("directory does not exist: {rel_dir}"));
    }
    if !base.is_dir() {
        return Err(anyhow!("path is not a directory: {rel_dir}"));
    }

    let mut results = Vec::new();
    if recursive {
        collect_tree(workspace, &base, 0, None, &mut results)?;
    } else {
        let mut entries = Vec::new();
        for entry in fs::read_dir(&base)? {
            let entry = entry?;
            entries.push(entry.path());
        }
        entries.sort();
        for path in entries {
            if should_hide(&path) {
                continue;
            }
            let metadata = fs::metadata(&path)?;
            results.push(json!({
                "path": workspace.rel_path(&path),
                "kind": if metadata.is_dir() { "directory" } else { "file" },
                "size": if metadata.is_file() { metadata.len() } else { 0 },
                "language": if metadata.is_file() { language_for_path(&path) } else { "directory" },
                "producer": "filesystem",
                "reliability": "source_fact",
                "exact": true
            }));
        }
    }
    Ok(Value::Array(results))
}

pub fn tree(workspace: &Workspace, dir: Option<&str>, depth: Option<u8>) -> Result<Value> {
    let rel_dir = dir.unwrap_or(".");
    let base = workspace.abs_path(rel_dir);
    if !base.exists() {
        return Err(anyhow!("directory does not exist: {rel_dir}"));
    }
    let mut results = Vec::new();
    collect_tree(workspace, &base, 0, depth.map(usize::from), &mut results)?;
    Ok(Value::Array(results))
}

pub fn read(workspace: &Workspace, target: &str) -> Result<Value> {
    let request = ReadTarget::parse(target)?;
    let path = workspace.abs_path(&request.path);
    let canonical_path =
        fs::canonicalize(&path).with_context(|| format!("failed to read {}", request.path))?;
    if !canonical_path.starts_with(&workspace.root) {
        return Err(anyhow!("path escapes workspace root: {}", request.path));
    }

    let metadata = fs::metadata(&canonical_path)
        .with_context(|| format!("failed to read {}", request.path))?;
    if !metadata.is_file() {
        return Err(anyhow!("failed to read {}", request.path));
    }

    let file_facts = scan_file_facts(&canonical_path, &request.path)?;
    if file_facts.binary {
        return Ok(json!({
            "path": request.path,
            "range": {
                "start": { "line": 1, "column": 1 },
                "end": { "line": 1, "column": 1 }
            },
            "content": "",
            "binary": true,
            "truncated": false,
            "fileHash": file_facts.hash,
            "language": language_for_path(&path),
            "producer": "snapshot_store_live_read",
            "reliability": "source_fact",
            "exact": false,
            "warnings": ["binary_file_not_displayed"]
        }));
    }

    let mut warnings = Vec::new();
    let read_content = if request.has_explicit_range {
        read_line_range(
            &canonical_path,
            &request.path,
            request.start_line.unwrap_or(1),
            request.end_line.unwrap_or(1),
        )?
    } else if metadata.len() as usize > MAX_FULL_READ_BYTES {
        warnings.push("large_file_truncated");
        read_prefix(&canonical_path, &request.path, MAX_FULL_READ_BYTES)?
    } else {
        read_full_text(&canonical_path, &request.path)?
    };

    Ok(json!({
        "path": request.path,
        "range": {
            "start": { "line": read_content.start_line, "column": 1 },
            "end": { "line": read_content.end_line, "column": read_content.end_column }
        },
        "content": read_content.content,
        "binary": false,
        "truncated": read_content.truncated,
        "fileHash": file_facts.hash,
        "language": language_for_path(&path),
        "producer": "snapshot_store_live_read",
        "reliability": "source_fact",
        "exact": !read_content.truncated,
        "warnings": warnings
    }))
}

pub fn find(
    workspace: &Workspace,
    opts: &ScanOptions,
    pattern: &str,
    mode: &str,
    context: u16,
    refs_mode: bool,
) -> Result<QueryOutput> {
    let regex = match mode {
        "literal" => Regex::new(&regex::escape(pattern))?,
        "regex" => Regex::new(pattern)?,
        other => return Err(anyhow!("unsupported search mode: {other}")),
    };

    let source = candidate_text_files(workspace, opts, pattern, mode)?;
    let mut results = Vec::new();
    for file in source.records {
        let path = workspace.abs_path(&file.path);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        for mat in regex.find_iter(&content) {
            if refs_mode && !identifier_boundary(&content, mat.start(), mat.end()) {
                continue;
            }
            let range = byte_range_to_line_range(&content, mat.start(), mat.end());
            results.push(json!({
                "path": file.path,
                "range": range,
                "matchText": mat.as_str(),
                "preview": preview_line(&content, mat.start()),
                "context": context_lines(&content, range["start"]["line"].as_u64().unwrap_or(1) as usize, context),
                "fileHash": file.hash,
                "language": file.language,
                "producer": text_search_producer(refs_mode, source.index["used"].as_bool().unwrap_or(false)),
                "reliability": "source_fact",
                "exact": true
            }));
            if opts.limit > 0 && results.len() >= opts.limit {
                return Ok(QueryOutput {
                    results: Value::Array(results),
                    index: source.index,
                });
            }
        }
    }
    Ok(QueryOutput {
        results: Value::Array(results),
        index: source.index,
    })
}

pub fn changed(workspace: &Workspace) -> Result<Value> {
    Ok(serde_json::to_value(&workspace.changed)?)
}

pub fn status(workspace: &Workspace) -> Value {
    json!({
        "root": workspace.root,
        "gitRoot": workspace.git_root,
        "head": workspace.head,
        "dirty": workspace.dirty,
        "stagedCount": workspace.staged_count,
        "worktreeCount": workspace.worktree_count,
        "snapshot_id": workspace.snapshot_id,
        "producer": "git_status_filesystem",
        "reliability": "source_fact",
        "exact": true
    })
}

pub fn line_range_for_node(
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
) -> Value {
    json!({
        "start": { "line": start_row + 1, "column": start_col + 1 },
        "end": { "line": end_row + 1, "column": end_col + 1 }
    })
}

fn collect_tree(
    workspace: &Workspace,
    base: &Path,
    level: usize,
    max_depth: Option<usize>,
    results: &mut Vec<Value>,
) -> Result<()> {
    if let Some(max_depth) = max_depth {
        if level > max_depth {
            return Ok(());
        }
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        if should_hide(&entry.path()) {
            continue;
        }
        entries.push(entry.path());
    }
    entries.sort();

    for path in entries {
        let metadata = fs::metadata(&path)?;
        results.push(json!({
            "path": workspace.rel_path(&path),
            "kind": if metadata.is_dir() { "directory" } else { "file" },
            "depth": level,
            "size": if metadata.is_file() { metadata.len() } else { 0 },
            "language": if metadata.is_file() { language_for_path(&path) } else { "directory" },
            "producer": "filesystem",
            "reliability": "source_fact",
            "exact": true
        }));
        if metadata.is_dir() {
            collect_tree(workspace, &path, level + 1, max_depth, results)?;
        }
    }
    Ok(())
}

fn should_hide(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            matches!(
                name,
                ".git" | ".code-search" | "target" | "node_modules" | "dist"
            )
        })
        .unwrap_or(false)
}

fn has_glob_meta(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[') || pattern.contains('{')
}

struct CandidateFiles {
    records: Vec<FileRecord>,
    index: Value,
}

struct CandidateFileCatalog {
    records: Vec<FileEntry>,
    index: Value,
}

struct FileEntry {
    path: String,
    language: String,
    size: u64,
    hash: Option<String>,
}

impl From<FileRecord> for FileEntry {
    fn from(record: FileRecord) -> Self {
        Self {
            path: record.path,
            language: record.language,
            size: record.size,
            hash: Some(record.hash),
        }
    }
}

impl From<FileCatalogRecord> for FileEntry {
    fn from(record: FileCatalogRecord) -> Self {
        Self {
            path: record.path,
            language: record.language,
            size: record.size,
            hash: None,
        }
    }
}

fn candidate_file_catalog(
    workspace: &Workspace,
    opts: &ScanOptions,
) -> Result<CandidateFileCatalog> {
    if let Some((records, index)) = index::fresh_file_records(workspace, opts)? {
        return Ok(CandidateFileCatalog {
            records: filter_file_entries(records.into_iter().map(FileEntry::from).collect(), opts),
            index,
        });
    }

    let mut scan_opts = opts.clone();
    scan_opts.limit = 0;
    Ok(CandidateFileCatalog {
        records: workspace
            .scan_catalog(&scan_opts)?
            .into_iter()
            .map(FileEntry::from)
            .collect(),
        index: index::live_scan_index_meta("index_missing_or_stale"),
    })
}

fn candidate_text_files(
    workspace: &Workspace,
    opts: &ScanOptions,
    pattern: &str,
    mode: &str,
) -> Result<CandidateFiles> {
    if let Some((records, index)) = index::fresh_text_records(workspace, opts, pattern, mode)? {
        return Ok(CandidateFiles {
            records: filter_records(records, opts),
            index,
        });
    }

    let mut scan_opts = opts.clone();
    scan_opts.limit = 0;
    Ok(CandidateFiles {
        records: workspace.scan_files(&scan_opts)?,
        index: index::live_scan_index_meta("index_missing_or_stale"),
    })
}

fn filter_records(records: Vec<FileRecord>, opts: &ScanOptions) -> Vec<FileRecord> {
    records
        .into_iter()
        .filter(|record| {
            !opts
                .exclude
                .iter()
                .any(|pattern| record.path.contains(pattern))
                && (opts.include.is_empty()
                    || opts
                        .include
                        .iter()
                        .any(|pattern| record.path.contains(pattern)))
        })
        .collect()
}

fn filter_file_entries(records: Vec<FileEntry>, opts: &ScanOptions) -> Vec<FileEntry> {
    records
        .into_iter()
        .filter(|record| {
            !opts
                .exclude
                .iter()
                .any(|pattern| record.path.contains(pattern))
                && (opts.include.is_empty()
                    || opts
                        .include
                        .iter()
                        .any(|pattern| record.path.contains(pattern)))
        })
        .collect()
}

fn text_search_producer(refs_mode: bool, index_used: bool) -> &'static str {
    match (refs_mode, index_used) {
        (true, true) => "text_index_identifier_boundary_search",
        (true, false) => "identifier_boundary_text_search",
        (false, true) => "text_index_live_text_search",
        (false, false) => "live_text_search",
    }
}

fn preview_line(content: &str, byte: usize) -> String {
    let start = content[..byte].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let end = content[byte..]
        .find('\n')
        .map(|idx| byte + idx)
        .unwrap_or(content.len());
    content[start..end].trim_end().to_string()
}

fn context_lines(content: &str, line: usize, context: u16) -> Value {
    if context == 0 {
        return Value::Array(Vec::new());
    }
    let lines: Vec<&str> = content.lines().collect();
    let context = usize::from(context);
    let start = line.saturating_sub(context + 1);
    let end = (line + context).min(lines.len());
    let values = lines[start..end]
        .iter()
        .enumerate()
        .map(|(idx, text)| {
            json!({
                "line": start + idx + 1,
                "text": text
            })
        })
        .collect();
    Value::Array(values)
}

fn byte_range_to_line_range(content: &str, start: usize, end: usize) -> Value {
    let (start_line, start_col) = line_col(content, start);
    let (end_line, end_col) = line_col(content, end);
    json!({
        "start": { "line": start_line, "column": start_col },
        "end": { "line": end_line, "column": end_col }
    })
}

fn line_col(content: &str, byte: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (idx, ch) in content.char_indices() {
        if idx >= byte {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn line_end_column(line: &str) -> usize {
    line.chars().count() + 1
}

fn identifier_boundary(content: &str, start: usize, end: usize) -> bool {
    let before = content[..start].chars().next_back();
    let after = content[end..].chars().next();
    !is_ident_char(before) && !is_ident_char(after)
}

fn is_ident_char(value: Option<char>) -> bool {
    value
        .map(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        .unwrap_or(false)
}

struct ReadTarget {
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
    has_explicit_range: bool,
}

impl ReadTarget {
    fn parse(target: &str) -> Result<Self> {
        let Some((path, range)) = target.rsplit_once(':') else {
            return Ok(Self {
                path: target.to_string(),
                start_line: None,
                end_line: None,
                has_explicit_range: false,
            });
        };
        if path.is_empty() || !range.chars().all(|ch| ch.is_ascii_digit() || ch == '-') {
            return Ok(Self {
                path: target.to_string(),
                start_line: None,
                end_line: None,
                has_explicit_range: false,
            });
        }
        let (start_line, end_line) = range.split_once('-').map_or_else(
            || {
                let line = parse_line(range)?;
                Ok((line, line))
            },
            |(start, end)| {
                let start = parse_line(start)?;
                let end = parse_line(end)?;
                if start > end {
                    return Err(anyhow!("invalid line range: {start}-{end}"));
                }
                Ok((start, end))
            },
        )?;
        Ok(Self {
            path: path.to_string(),
            start_line: Some(start_line),
            end_line: Some(end_line),
            has_explicit_range: true,
        })
    }
}

fn parse_line(value: &str) -> Result<usize> {
    let line = value
        .parse::<usize>()
        .map_err(|_| anyhow!("invalid line range: {value}"))?;
    if line == 0 {
        return Err(anyhow!("invalid line range: {value}"));
    }
    Ok(line)
}

struct FileFacts {
    hash: String,
    binary: bool,
}

struct ReadContent {
    content: String,
    start_line: usize,
    end_line: usize,
    end_column: usize,
    truncated: bool,
}

fn scan_file_facts(path: &Path, display_path: &str) -> Result<FileFacts> {
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to read {display_path}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut binary = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read {display_path}"))?;
        if read == 0 {
            break;
        }
        if buffer[..read].iter().any(|byte| *byte == 0) {
            binary = true;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(FileFacts {
        hash: format!("blake3:{}", hasher.finalize().to_hex()),
        binary,
    })
}

fn read_full_text(path: &Path, display_path: &str) -> Result<ReadContent> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {display_path}"))?;
    let end_line = line_count_for_content(&content);
    let end_column = last_line_end_column(&content);
    Ok(ReadContent {
        content,
        start_line: 1,
        end_line,
        end_column,
        truncated: false,
    })
}

fn read_prefix(path: &Path, display_path: &str, max_bytes: usize) -> Result<ReadContent> {
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to read {display_path}"))?;
    let mut bytes = Vec::with_capacity(max_bytes);
    file.by_ref()
        .take(max_bytes as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {display_path}"))?;
    while std::str::from_utf8(&bytes).is_err() && !bytes.is_empty() {
        bytes.pop();
    }
    let content =
        String::from_utf8(bytes).with_context(|| format!("failed to read {display_path}"))?;
    let end_line = line_count_for_content(&content);
    let end_column = last_line_end_column(&content);
    Ok(ReadContent {
        content,
        start_line: 1,
        end_line,
        end_column,
        truncated: true,
    })
}

fn read_line_range(
    path: &Path,
    display_path: &str,
    start_line: usize,
    requested_end_line: usize,
) -> Result<ReadContent> {
    let file = fs::File::open(path).with_context(|| format!("failed to read {display_path}"))?;
    let reader = BufReader::new(file);
    let mut selected = Vec::new();
    let mut total_lines = 0;
    for (idx, line) in reader.lines().enumerate() {
        let line_no = idx + 1;
        let line = line.with_context(|| format!("failed to read {display_path}"))?;
        total_lines = line_no;
        if line_no >= start_line && line_no <= requested_end_line {
            selected.push(line);
        }
    }

    if selected.is_empty() && start_line > total_lines && total_lines > 0 {
        return Err(anyhow!(
            "invalid line range: {start_line}-{requested_end_line}"
        ));
    }

    let content = selected.join("\n");
    let end_line = if selected.is_empty() {
        start_line
    } else {
        start_line + selected.len() - 1
    };
    let end_column = selected
        .last()
        .map(|line| line_end_column(line))
        .unwrap_or(1);
    Ok(ReadContent {
        content,
        start_line,
        end_line,
        end_column,
        truncated: false,
    })
}

fn line_count_for_content(content: &str) -> usize {
    let count = content.lines().count();
    if count == 0 {
        1
    } else {
        count
    }
}

fn last_line_end_column(content: &str) -> usize {
    content.lines().last().map(line_end_column).unwrap_or(1)
}

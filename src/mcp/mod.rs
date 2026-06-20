//! MCP (Model Context Protocol) adapter that wraps the [`QueryService`] and
//! exposes codetrail operations to LLM agents (Claude, Cursor, etc.) over
//! stdio-based JSON-RPC 2.0.
//!
//! ## Protocol flow
//!
//! ```text
//! Client                          Server
//!   │                                │
//!   ├─ initialize ──────────────────>│
//!   │<─────── capabilities ─────────┤
//!   │                                │
//!   ├─ tools/list ──────────────────>│
//!   │<─────── tool definitions ─────┤
//!   │                                │
//!   ├─ tools/call {name, args} ─────>│
//!   │<─────── result (JSON) ────────┤
//!   │                                │
//! ```
//!
//! Tool results use the same public JSON projection as CLI `--output json`:
//! `results`, `page`, and `caveats`.

use std::io::{self, BufRead, Write};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::{
    code_context::{
        CodeContextOptions, DEFAULT_CODE_CONTEXT_LINES, DEFAULT_CODE_MAX_LINES, MAX_CODE_MAX_LINES,
    },
    output,
    query::{ExploreNodeOptions, QueryOptions, QueryService},
    query_input::InputMode,
    search_pattern::SearchPatternMode,
    workspace::RemoteMode,
};

mod protocol;

use crate::mcp::protocol::{
    ok_response, Envelope, InitializeResult, ServerInfo, ToolCallParams, ToolCallResult, ToolDef,
    ToolResultContent, ToolsListResult,
};
// ---------------------------------------------------------------------------
// MCP constants
// ---------------------------------------------------------------------------

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "codetrail";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn with_remote_query_schema(mut schema: Value) -> Value {
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert(
            "remoteMode".to_string(),
            json!({
                "type": "string",
                "enum": ["auto", "only"],
                "default": "auto",
                "description": "Remote snapshot selection mode. Use only to query remote text snapshots without local source reads."
            }),
        );
        properties.insert(
            "remoteSnapshot".to_string(),
            json!({
                "type": "string",
                "description": "Remote snapshot key or snapshot id to query when remoteMode is only or a specific remote snapshot is required."
            }),
        );
    }
    schema
}

fn with_code_context_schema(mut schema: Value) -> Value {
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert(
            "includeCode".to_string(),
            json!({
                "type": "boolean",
                "default": false,
                "description": "Attach current local source context and capped call relations to each result."
            }),
        );
        properties.insert(
            "codeContext".to_string(),
            json!({
                "type": "integer",
                "minimum": 0,
                "maximum": 65535,
                "default": DEFAULT_CODE_CONTEXT_LINES,
                "description": "Fallback context lines around an occurrence when a symbol body range is unavailable. Only valid when includeCode is true."
            }),
        );
        properties.insert(
            "codeMaxLines".to_string(),
            json!({
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_CODE_MAX_LINES,
                "default": DEFAULT_CODE_MAX_LINES,
                "description": "Maximum source lines returned per result. Only valid when includeCode is true."
            }),
        );
    }
    schema
}

// ---------------------------------------------------------------------------
// Tool definitions  (static so we can serve tools/list without I/O)
// ---------------------------------------------------------------------------

/// Build the list of all available tools.
fn tool_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "codetrail_find".to_string(),
            description:
                "Full-text / literal search across the codebase. Returns matching lines with file paths and line numbers."
                    .to_string(),
            input_schema: with_remote_query_schema(json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Literal text to search for" },
                    "mode": { "type": "string", "enum": ["literal", "regex", "wildcard"], "default": "literal", "description": "Content match mode" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before content search" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for content, path, and symbol matching" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include (AND filter)" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" },
                    "context": { "type": "integer", "minimum": 0, "maximum": 65535, "default": 0, "description": "Lines of context around each match" }
                },
                "required": ["text"]
            })),
        },
        ToolDef {
            name: "codetrail_grep".to_string(),
            description: "Regex search across the codebase. Returns matching lines with file paths and line numbers."
                .to_string(),
            input_schema: with_remote_query_schema(json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regular expression pattern" },
                    "mode": { "type": "string", "enum": ["literal", "regex", "wildcard"], "default": "regex", "description": "Content match mode" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before content search" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for content, path, and symbol matching" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" },
                    "context": { "type": "integer", "minimum": 0, "maximum": 65535, "default": 0, "description": "Lines of context around each match" }
                },
                "required": ["pattern"]
            })),
        },
        ToolDef {
            name: "codetrail_files".to_string(),
            description:
                "Find files whose path contains the given substring. Returns file metadata."
                    .to_string(),
            input_schema: with_remote_query_schema(json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Substring to match in file paths" },
                    "mode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "literal", "description": "Path match mode" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before path search" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for path matching" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" }
                },
                "required": ["pattern"]
            })),
        },
        ToolDef {
            name: "codetrail_glob".to_string(),
            description: "Find files matching a strict glob pattern (e.g. `**/*.rs`). Returns file metadata."
                .to_string(),
            input_schema: with_remote_query_schema(json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern (e.g. **/*.rs)" },
                    "mode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "glob", "description": "Path match mode" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before path search" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for path matching" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" }
                },
                "required": ["pattern"]
            })),
        },
        ToolDef {
            name: "codetrail_defs".to_string(),
            description:
                "Find definitions of a given identifier. Prefers SCIP precise index; falls back to tree-sitter parser."
                    .to_string(),
            input_schema: with_code_context_schema(json!({
                "type": "object",
                "properties": {
                    "identifier": { "type": "string", "description": "Identifier to find definitions for" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before symbol search" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for symbol input matching" },
                    "inputMode": { "type": "string", "enum": ["compatible", "strict"], "default": "compatible", "description": "Symbol input handling mode" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" }
                },
                "required": ["identifier"]
            })),
        },
        ToolDef {
            name: "codetrail_refs".to_string(),
            description:
                "Find references to a given identifier. Prefers SCIP precise index; falls back to text search."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "identifier": { "type": "string", "description": "Identifier to find references for" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before symbol search" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for symbol input matching" },
                    "inputMode": { "type": "string", "enum": ["compatible", "strict"], "default": "compatible", "description": "Symbol input handling mode" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" }
                },
                "required": ["identifier"]
            }),
        },
        ToolDef {
            name: "codetrail_symbols".to_string(),
            description:
                "Find symbols (functions, structs, classes, etc.) matching a query. Prefers SCIP; falls back to tree-sitter."
                    .to_string(),
            input_schema: with_code_context_schema(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Symbol name query (substring match)" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before symbol search" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for symbol input matching" },
                    "inputMode": { "type": "string", "enum": ["compatible", "strict"], "default": "compatible", "description": "Symbol input handling mode" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" }
                },
                "required": ["query"]
            })),
        },
        ToolDef {
            name: "codetrail_routes".to_string(),
            description:
                "Scan framework route declarations and return URL patterns with handler candidates."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Optional route path substring to match" },
                    "framework": { "type": "array", "items": { "type": "string" }, "description": "Framework names to include" },
                    "method": { "type": "array", "items": { "type": "string" }, "description": "HTTP methods to include" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before route scan" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for route path matching" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" }
                }
            }),
        },
        ToolDef {
            name: "codetrail_calls".to_string(),
            description:
                "Find outgoing calls from a given function/symbol. Results are inferred candidates due to limitations in static analysis."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "identifier": { "type": "string", "description": "Function/symbol name to query outgoing calls for" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before call search" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for symbol input matching" },
                    "inputMode": { "type": "string", "enum": ["compatible", "strict"], "default": "compatible", "description": "Symbol input handling mode" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" }
                },
                "required": ["identifier"]
            }),
        },
        ToolDef {
            name: "codetrail_callers".to_string(),
            description:
                "Find incoming callers of a given function/symbol. Results are inferred candidates due to limitations in static analysis."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "identifier": { "type": "string", "description": "Function/symbol name to query incoming callers for" },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before call search" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for symbol input matching" },
                    "inputMode": { "type": "string", "enum": ["compatible", "strict"], "default": "compatible", "description": "Symbol input handling mode" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "cursor": { "type": "string", "description": "Pagination cursor from a previous response" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad queries to return full paginated results" },
                    "limit": { "type": "integer", "minimum": 0, "default": 100, "description": "Max results" }
                },
                "required": ["identifier"]
            }),
        },
        ToolDef {
            name: "codetrail_explore_node".to_string(),
            description:
                "Explore a symbol or file node with bounded snippets and capped inferred relations. Tries defs, symbols, then files."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Symbol, type, method, or file name to explore" },
                    "maxCandidates": { "type": "integer", "minimum": 1, "maximum": 20, "default": 5 },
                    "snippetLines": { "type": "integer", "minimum": 1, "maximum": 80, "default": 12 },
                    "relationLimit": { "type": "integer", "minimum": 0, "maximum": 20, "default": 8 },
                    "dir": { "type": "array", "items": { "type": "string" }, "description": "Workspace-relative directories to search (OR filter)" },
                    "ext": { "type": "array", "items": { "type": "string" }, "description": "File extensions to search, with or without a leading dot" },
                    "filePattern": { "type": "array", "items": { "type": "string" }, "description": "Path patterns applied before exploration" },
                    "fileMode": { "type": "string", "enum": ["literal", "regex", "wildcard", "glob"], "default": "wildcard", "description": "Pattern mode for filePattern" },
                    "caseSensitive": { "type": "boolean", "default": false, "description": "Use exact case for symbol input matching" },
                    "inputMode": { "type": "string", "enum": ["compatible", "strict"], "default": "compatible", "description": "Symbol input handling mode" },
                    "include": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to include" },
                    "exclude": { "type": "array", "items": { "type": "string" }, "description": "Path substrings to exclude" },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "Languages to include" },
                    "changed": { "type": "boolean", "default": false, "description": "Restrict search to git changed files" },
                    "allowBroad": { "type": "boolean", "default": false, "description": "Allow broad path fallback" }
                },
                "required": ["query"]
            }),
        },
        ToolDef {
            name: "codetrail_changed".to_string(),
            description:
                "List changed (git-modified or untracked) files in the workspace."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDef {
            name: "codetrail_status".to_string(),
            description:
                "Return workspace status including snapshot_id, dirty flag, git root, and index information."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
    ]
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

/// Stdio-based MCP server that wraps [`QueryService`].
pub struct Server {
    service: QueryService,
}

impl Server {
    /// Create a new MCP server backed by the given workspace root.
    pub fn new(root: &std::path::Path) -> Result<Self> {
        let service = QueryService::new(root)?;
        Ok(Self { service })
    }

    /// Run the server loop: read JSON-RPC lines from stdin, dispatch, write responses to stdout.
    pub fn run(&self) -> Result<()> {
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut stdout = stdout.lock();

        for line in stdin.lock().lines() {
            let line = line.context("failed to read stdin line")?;
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            let envelope: Envelope = match serde_json::from_str(&line) {
                Ok(env) => env,
                Err(e) => {
                    let err_resp = protocol::ErrorResponse {
                        jsonrpc: "2.0".to_string(),
                        id: Value::Null,
                        error: protocol::RpcError {
                            code: protocol::RpcError::PARSE_ERROR,
                            message: format!("Parse error: {e}"),
                            data: None,
                        },
                    };
                    let resp_str = serde_json::to_string(&Envelope::ErrorResponse(err_resp))?;
                    writeln!(stdout, "{resp_str}")?;
                    stdout.flush()?;
                    continue;
                }
            };

            let response = self.dispatch(envelope);

            // Notifications have no id → no response.
            if let Some(resp) = response {
                let resp_str = serde_json::to_string(&resp)?;
                writeln!(stdout, "{resp_str}")?;
                stdout.flush()?;
            }
        }

        Ok(())
    }

    /// Dispatch a single JSON-RPC envelope to the appropriate handler.
    fn dispatch(&self, envelope: Envelope) -> Option<Envelope> {
        match envelope {
            Envelope::Request(req) => {
                if protocol::is_notification(&req.id) {
                    // Treat as notification — MCP clients may send initialized as notification.
                    return None;
                }
                self.handle_request(req)
            }
            Envelope::Notification(_notif) => {
                // MCP clients send `notifications/initialized` as a notification;
                // we silently acknowledge it.
                None
            }
            // Server shouldn't receive responses, but ignore them gracefully.
            _ => None,
        }
    }

    /// Handle a JSON-RPC request.
    fn handle_request(&self, req: protocol::Request) -> Option<Envelope> {
        let id = req.id.clone();
        let result = match req.method.as_str() {
            "initialize" => self.handle_initialize(),
            "tools/list" => self.handle_tools_list(),
            "tools/call" => {
                let params: ToolCallParams =
                    match serde_json::from_value(req.params.unwrap_or(Value::Null)) {
                        Ok(p) => p,
                        Err(e) => {
                            return Some(Envelope::ErrorResponse(
                                protocol::RpcError::invalid_params(
                                    id,
                                    format!("Invalid params: {e}"),
                                ),
                            ));
                        }
                    };
                self.handle_tool_call(&params)
            }
            _ => {
                return Some(Envelope::ErrorResponse(
                    protocol::RpcError::method_not_found(id),
                ));
            }
        };

        match result {
            Ok(value) => Some(ok_response(id, value)),
            Err(e) => Some(Envelope::ErrorResponse(protocol::RpcError::internal_error(
                id,
                e.to_string(),
            ))),
        }
    }

    // -- initialize ----------------------------------------------------

    fn handle_initialize(&self) -> Result<Value> {
        let init = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: json!({
                "tools": {}
            }),
            server_info: ServerInfo {
                name: SERVER_NAME.to_string(),
                version: SERVER_VERSION.to_string(),
            },
        };
        Ok(serde_json::to_value(init)?)
    }

    // -- tools/list ----------------------------------------------------

    fn handle_tools_list(&self) -> Result<Value> {
        let result = ToolsListResult {
            tools: tool_definitions(),
        };
        Ok(serde_json::to_value(result)?)
    }

    // -- tools/call ----------------------------------------------------

    fn handle_tool_call(&self, params: &ToolCallParams) -> Result<Value> {
        let result = self.execute_tool(&params.name, params.arguments.as_ref())?;
        Ok(serde_json::to_value(result)?)
    }

    /// Execute a named tool with optional arguments.
    fn execute_tool(&self, name: &str, args: Option<&Value>) -> Result<ToolCallResult> {
        match self.execute_tool_value(name, args) {
            Ok(query_result) => Ok(tool_result(query_result, false)),
            Err(error) => Ok(tool_result(output::error_response(error), true)),
        }
    }

    fn execute_tool_value(&self, name: &str, args: Option<&Value>) -> Result<Value> {
        let opts = parse_query_options(args)?;

        match name {
            "codetrail_find" => {
                let text = required_str(args, "text")?;
                let mode = optional_pattern_mode_arg(args, SearchPatternMode::Literal)?;
                self.service
                    .text_search("find", text, mode, opts.context, &opts)
            }
            "codetrail_grep" => {
                let pattern = required_str(args, "pattern")?;
                let mode = optional_pattern_mode_arg(args, SearchPatternMode::Regex)?;
                self.service
                    .text_search("grep", pattern, mode, opts.context, &opts)
            }
            "codetrail_files" => {
                let pattern = required_str(args, "pattern")?;
                let mode = optional_pattern_mode_arg(args, SearchPatternMode::Literal)?;
                self.service.files_with_mode("files", pattern, mode, &opts)
            }
            "codetrail_glob" => {
                let pattern = required_str(args, "pattern")?;
                let mode = optional_pattern_mode_arg(args, SearchPatternMode::Glob)?;
                self.service.files_with_mode("glob", pattern, mode, &opts)
            }
            "codetrail_defs" => {
                let identifier = required_str(args, "identifier")?;
                let code_options = parse_code_context_options(args)?;
                self.service
                    .defs_with_code(identifier, &opts, &code_options)
            }
            "codetrail_refs" => {
                let identifier = required_str(args, "identifier")?;
                self.service.refs(identifier, &opts)
            }
            "codetrail_symbols" => {
                let query = required_str(args, "query")?;
                let code_options = parse_code_context_options(args)?;
                self.service.symbols_with_code(query, &opts, &code_options)
            }
            "codetrail_routes" => {
                let pattern = optional_str(args, "pattern");
                let frameworks = args
                    .and_then(Value::as_object)
                    .map(|obj| extract_string_array(obj, "framework"))
                    .unwrap_or_default();
                let methods = args
                    .and_then(Value::as_object)
                    .map(|obj| extract_string_array(obj, "method"))
                    .unwrap_or_default();
                self.service.routes(pattern, &frameworks, &methods, &opts)
            }
            "codetrail_explore_node" => {
                let query = required_str(args, "query")?;
                self.service
                    .explore_node(query, &opts, parse_explore_node_options(args)?)
            }
            "codetrail_calls" => {
                let identifier = required_str(args, "identifier")?;
                self.service.calls(identifier, &opts)
            }
            "codetrail_callers" => {
                let identifier = required_str(args, "identifier")?;
                self.service.callers(identifier, &opts)
            }
            "codetrail_changed" => self.service.changed(),
            "codetrail_status" => self.service.status(),
            _ => Err(anyhow::anyhow!("unknown tool: {name}")),
        }
    }
}

fn tool_result(value: Value, is_error: bool) -> ToolCallResult {
    let public = output::public_response_value(&value);
    ToolCallResult {
        content: vec![ToolResultContent {
            content_type: "text".to_string(),
            text: public.to_string(),
        }],
        is_error,
    }
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

/// Extract a required string argument from the tool arguments JSON object.
fn required_str<'a>(args: Option<&'a Value>, field: &str) -> Result<&'a str> {
    let obj = args
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("missing arguments"))?;
    let value = obj
        .get(field)
        .ok_or_else(|| anyhow::anyhow!("missing required argument: {field}"))?;
    value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("argument '{field}' must be a string"))
}

fn optional_str<'a>(args: Option<&'a Value>, field: &str) -> Option<&'a str> {
    args.and_then(Value::as_object)?
        .get(field)
        .and_then(Value::as_str)
}

fn optional_pattern_mode_arg(
    args: Option<&Value>,
    default: SearchPatternMode,
) -> Result<SearchPatternMode> {
    let Some(obj) = args.and_then(Value::as_object) else {
        return Ok(default);
    };
    optional_pattern_mode(obj, &["mode"], default)
}

/// Parse [`QueryOptions`] from the tool arguments JSON object.
fn parse_query_options(args: Option<&Value>) -> Result<QueryOptions> {
    let obj = match args.and_then(|v| v.as_object()) {
        Some(o) => o,
        None => return Ok(QueryOptions::default()),
    };

    Ok(QueryOptions {
        dirs: extract_string_arrays(obj, &["dir", "dirs"]),
        extensions: extract_string_arrays(obj, &["ext", "extensions"]),
        file_patterns: extract_string_arrays(obj, &["filePattern", "filePatterns", "file_pattern"]),
        file_mode: optional_pattern_mode(
            obj,
            &["fileMode", "file_mode"],
            SearchPatternMode::Wildcard,
        )?,
        case_sensitive: obj
            .get("caseSensitive")
            .or_else(|| obj.get("case_sensitive"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        input_mode: optional_input_mode(obj, &["inputMode", "input_mode"], InputMode::Compatible)?,
        include: extract_string_array(obj, "include"),
        exclude: extract_string_array(obj, "exclude"),
        lang: extract_string_array(obj, "lang"),
        changed: obj
            .get("changed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        hidden: obj.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false),
        no_ignore: obj
            .get("noIgnore")
            .or_else(|| obj.get("no_ignore"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        cursor: obj
            .get("cursor")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        allow_broad: obj
            .get("allowBroad")
            .or_else(|| obj.get("allow_broad"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        limit: optional_usize_arg(obj, "limit", 100)?,
        context: optional_u16_arg(obj, "context", 0)?,
        remote_mode: optional_remote_mode(obj)?,
        remote_snapshot: obj
            .get("remoteSnapshot")
            .or_else(|| obj.get("remote_snapshot"))
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
    })
}

fn parse_code_context_options(args: Option<&Value>) -> Result<CodeContextOptions> {
    let obj = match args.and_then(Value::as_object) {
        Some(obj) => obj,
        None => return Ok(CodeContextOptions::default()),
    };
    let include_code = match obj.get("includeCode").or_else(|| obj.get("include_code")) {
        Some(value) => value.as_bool().ok_or_else(|| {
            anyhow::anyhow!("invalid_mcp_argument: includeCode must be a boolean")
        })?,
        None => false,
    };
    let has_code_context = obj.contains_key("codeContext") || obj.contains_key("code_context");
    let has_code_max_lines = obj.contains_key("codeMaxLines") || obj.contains_key("code_max_lines");
    if !include_code && (has_code_context || has_code_max_lines) {
        return Err(anyhow::anyhow!(
            "invalid_mcp_argument: codeContext and codeMaxLines require includeCode"
        ));
    }
    let code_context = optional_u16_fields(
        obj,
        &["codeContext", "code_context"],
        DEFAULT_CODE_CONTEXT_LINES,
    )?;
    let code_max_lines = optional_usize_fields(
        obj,
        &["codeMaxLines", "code_max_lines"],
        DEFAULT_CODE_MAX_LINES,
    )?;
    if !(1..=MAX_CODE_MAX_LINES).contains(&code_max_lines) {
        return Err(anyhow::anyhow!(
            "invalid_mcp_argument: codeMaxLines must be an integer between 1 and {MAX_CODE_MAX_LINES}"
        ));
    }
    Ok(CodeContextOptions {
        include_code,
        code_context,
        code_max_lines,
    })
}

fn parse_explore_node_options(args: Option<&Value>) -> Result<ExploreNodeOptions> {
    let obj = match args.and_then(Value::as_object) {
        Some(obj) => obj,
        None => {
            return Ok(ExploreNodeOptions::bounded(5, 12, 8));
        }
    };
    let max_candidates = optional_usize_fields(
        obj,
        &["maxCandidates", "max_candidates", "max-candidates"],
        5,
    )?;
    let snippet_lines =
        optional_usize_fields(obj, &["snippetLines", "snippet_lines", "snippet-lines"], 12)?;
    let relation_limit = optional_usize_fields(
        obj,
        &["relationLimit", "relation_limit", "relation-limit"],
        8,
    )?;
    validate_explore_bound("maxCandidates", max_candidates, 1, 20)?;
    validate_explore_bound("snippetLines", snippet_lines, 1, 80)?;
    validate_explore_bound("relationLimit", relation_limit, 0, 20)?;
    Ok(ExploreNodeOptions::bounded(
        max_candidates,
        snippet_lines,
        relation_limit,
    ))
}

fn validate_explore_bound(name: &str, value: usize, min: usize, max: usize) -> Result<()> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "invalid_mcp_argument: {name} must be between {min} and {max}"
        ))
    }
}

fn extract_string_array(obj: &serde_json::Map<String, Value>, field: &str) -> Vec<String> {
    let Some(value) = obj.get(field) else {
        return Vec::new();
    };
    if let Some(text) = value.as_str() {
        return vec![text.to_string()];
    }
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn extract_string_arrays(obj: &serde_json::Map<String, Value>, fields: &[&str]) -> Vec<String> {
    fields
        .iter()
        .flat_map(|field| extract_string_array(obj, field))
        .collect()
}

fn optional_usize_fields(
    obj: &serde_json::Map<String, Value>,
    fields: &[&str],
    default: usize,
) -> Result<usize> {
    for field in fields {
        if obj.contains_key(*field) {
            return optional_usize_arg(obj, field, default);
        }
    }
    Ok(default)
}

fn optional_usize_arg(
    obj: &serde_json::Map<String, Value>,
    field: &str,
    default: usize,
) -> Result<usize> {
    let Some(value) = obj.get(field) else {
        return Ok(default);
    };
    let Some(number) = value.as_u64() else {
        return Err(anyhow::anyhow!(
            "invalid_mcp_argument: {field} must be a non-negative integer"
        ));
    };
    usize::try_from(number).map_err(|_| {
        anyhow::anyhow!("invalid_mcp_argument: {field} must fit in the platform usize")
    })
}

fn optional_u16_fields(
    obj: &serde_json::Map<String, Value>,
    fields: &[&str],
    default: u16,
) -> Result<u16> {
    for field in fields {
        if obj.contains_key(*field) {
            return optional_u16_arg(obj, field, default);
        }
    }
    Ok(default)
}

fn optional_u16_arg(
    obj: &serde_json::Map<String, Value>,
    field: &str,
    default: u16,
) -> Result<u16> {
    let Some(value) = obj.get(field) else {
        return Ok(default);
    };
    let Some(number) = value.as_u64() else {
        return Err(anyhow::anyhow!(
            "invalid_mcp_argument: {field} must be an integer between 0 and 65535"
        ));
    };
    if number > u16::MAX as u64 {
        return Err(anyhow::anyhow!(
            "invalid_mcp_argument: {field} must be an integer between 0 and 65535"
        ));
    }
    Ok(number as u16)
}

fn optional_pattern_mode(
    obj: &serde_json::Map<String, Value>,
    fields: &[&str],
    default: SearchPatternMode,
) -> Result<SearchPatternMode> {
    for field in fields {
        if let Some(value) = obj.get(*field).and_then(Value::as_str) {
            return SearchPatternMode::parse(value);
        }
    }
    Ok(default)
}

fn optional_input_mode(
    obj: &serde_json::Map<String, Value>,
    fields: &[&str],
    default: InputMode,
) -> Result<InputMode> {
    let Some(value) = fields
        .iter()
        .find_map(|field| obj.get(*field).and_then(Value::as_str))
    else {
        return Ok(default);
    };
    match value {
        "compatible" => Ok(InputMode::Compatible),
        "strict" => Ok(InputMode::Strict),
        other => Err(anyhow::anyhow!(
            "invalid_mcp_argument: unsupported inputMode {other}"
        )),
    }
}

fn optional_remote_mode(obj: &serde_json::Map<String, Value>) -> Result<RemoteMode> {
    let Some(value) = obj
        .get("remoteMode")
        .or_else(|| obj.get("remote_mode"))
        .and_then(Value::as_str)
    else {
        return Ok(RemoteMode::Auto);
    };
    match value {
        "auto" => Ok(RemoteMode::Auto),
        "only" => Ok(RemoteMode::Only),
        other => Err(anyhow::anyhow!(
            "invalid_mcp_argument: unsupported remoteMode {other}"
        )),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn call_tool(server: &Server, name: &str, arguments: Value) -> ToolCallResult {
        server.execute_tool(name, Some(&arguments)).unwrap()
    }

    fn call_tool_json(server: &Server, name: &str, arguments: Value) -> Value {
        let result = call_tool(server, name, arguments);
        assert!(!result.is_error, "tool returned error: {result:?}");
        serde_json::from_str(&result.content[0].text).unwrap()
    }

    fn has_caveat(value: &Value, code: &str) -> bool {
        value["caveats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|caveat| caveat["code"] == code)
    }

    // ------------------------------------------------------------------
    //  Protocol-level tests  (unit)
    // ------------------------------------------------------------------

    #[test]
    fn server_handles_initialize() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();
        let server = Server::new(dir.path()).unwrap();

        let req = protocol::Request {
            jsonrpc: "2.0".to_string(),
            id: json!(1),
            method: "initialize".to_string(),
            params: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {}
            })),
        };

        let resp = server.handle_request(req).unwrap();
        match resp {
            Envelope::SuccessResponse(sr) => {
                let init: InitializeResult = serde_json::from_value(sr.result).unwrap();
                assert_eq!(init.protocol_version, "2024-11-05");
                assert_eq!(init.server_info.name, "codetrail");
                assert!(init.capabilities.get("tools").is_some());
            }
            _ => panic!("expected success response"),
        }
    }

    #[test]
    fn server_handles_tools_list() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();
        let server = Server::new(dir.path()).unwrap();

        let req = protocol::Request {
            jsonrpc: "2.0".to_string(),
            id: json!(2),
            method: "tools/list".to_string(),
            params: None,
        };

        let resp = server.handle_request(req).unwrap();
        match resp {
            Envelope::SuccessResponse(sr) => {
                let list: ToolsListResult = serde_json::from_value(sr.result).unwrap();
                let names: Vec<&str> = list.tools.iter().map(|t| t.name.as_str()).collect();
                assert!(names.contains(&"codetrail_find"));
                assert!(names.contains(&"codetrail_defs"));
                assert!(!names.contains(&"codetrail_list"));
                assert!(!names.contains(&"codetrail_tree"));
                assert!(!names.contains(&"codetrail_read"));
                assert!(names.contains(&"codetrail_routes"));
                assert!(names.contains(&"codetrail_explore_node"));
                assert!(names.contains(&"codetrail_status"));
                // All core CLI-backed tools should be present.
                assert_eq!(list.tools.len(), 13);
            }
            _ => panic!("expected success response"),
        }
    }

    #[test]
    fn tools_list_schema_includes_code_context_options() {
        let tools = tool_definitions();
        for name in ["codetrail_defs", "codetrail_symbols"] {
            let tool = tools
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("missing tool {name}"));
            let properties = tool.input_schema["properties"].as_object().unwrap();
            assert!(properties.contains_key("includeCode"), "{name}");
            assert!(properties.contains_key("codeContext"), "{name}");
            assert!(properties.contains_key("codeMaxLines"), "{name}");
            assert_eq!(
                properties["codeMaxLines"]["maximum"],
                json!(MAX_CODE_MAX_LINES)
            );
        }
    }

    #[test]
    fn server_handles_tools_call_find() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {\n    println!(\"needle\");\n}\n",
        )
        .unwrap();
        let server = Server::new(dir.path()).unwrap();

        let req = protocol::Request {
            jsonrpc: "2.0".to_string(),
            id: json!(3),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "codetrail_find",
                "arguments": { "text": "needle" }
            })),
        };

        let resp = server.handle_request(req).unwrap();
        match resp {
            Envelope::SuccessResponse(sr) => {
                let result: ToolCallResult = serde_json::from_value(sr.result).unwrap();
                assert!(!result.is_error);
                let text = &result.content[0].text;
                let parsed: Value = serde_json::from_str(text).unwrap();
                assert!(parsed.get("ok").is_none());
                assert!(parsed.get("reliability").is_none());
                assert_eq!(parsed["results"][0]["path"], "src/main.rs");
                assert!(parsed["caveats"].as_array().unwrap().is_empty());
            }
            _ => panic!("expected success response"),
        }
    }

    #[test]
    fn server_handles_tools_call_defs() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "fn alpha() {}\nfn beta() {}\n",
        )
        .unwrap();
        let server = Server::new(dir.path()).unwrap();

        let req = protocol::Request {
            jsonrpc: "2.0".to_string(),
            id: json!(4),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "codetrail_defs",
                "arguments": { "identifier": "alpha" }
            })),
        };

        let resp = server.handle_request(req).unwrap();
        match resp {
            Envelope::SuccessResponse(sr) => {
                let result: ToolCallResult = serde_json::from_value(sr.result).unwrap();
                assert!(!result.is_error);
                let text = &result.content[0].text;
                let parsed: Value = serde_json::from_str(text).unwrap();
                assert!(parsed.get("ok").is_none());
                assert!(parsed.get("reliability").is_none());
                let results = parsed["results"].as_array().unwrap();
                assert!(results.iter().any(|r| r["name"] == "alpha"));
            }
            _ => panic!("expected success response"),
        }
    }

    #[test]
    fn server_handles_tools_call_defs_include_code() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "fn alpha() {\n    beta();\n}\nfn beta() {}\n",
        )
        .unwrap();
        let server = Server::new(dir.path()).unwrap();

        let parsed = call_tool_json(
            &server,
            "codetrail_defs",
            json!({ "identifier": "alpha", "includeCode": true }),
        );
        let result = &parsed["results"][0];
        assert!(result["source"]["content"]
            .as_str()
            .unwrap()
            .contains("beta();"));
        assert_eq!(result["source"]["truncated"], false);
        assert!(result["relations"]["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|call| call["target"] == "beta"));
        assert!(result.get("sourceTarget").is_none());
        assert!(result.get("producer").is_none());
    }

    #[test]
    fn tools_call_defs_rejects_code_context_without_include_code() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "fn alpha() {}\n").unwrap();
        let server = Server::new(dir.path()).unwrap();

        let result = call_tool(
            &server,
            "codetrail_defs",
            json!({ "identifier": "alpha", "codeContext": 2 }),
        );
        assert!(result.is_error);
        let parsed: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(has_caveat(&parsed, "invalid_mcp_argument"));
    }

    #[test]
    fn server_returns_error_for_unknown_tool() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();
        let server = Server::new(dir.path()).unwrap();

        let req = protocol::Request {
            jsonrpc: "2.0".to_string(),
            id: json!(5),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "codetrail_nonexistent",
                "arguments": {}
            })),
        };

        let resp = server.handle_request(req).unwrap();
        match resp {
            Envelope::SuccessResponse(sr) => {
                let result: ToolCallResult = serde_json::from_value(sr.result).unwrap();
                assert!(result.is_error);
                let parsed: Value = serde_json::from_str(&result.content[0].text).unwrap();
                assert!(parsed["results"].as_array().unwrap().is_empty());
                assert!(has_caveat(&parsed, "unknown_tool"));
            }
            _ => panic!("expected success for unknown tool"),
        }
    }

    #[test]
    fn server_returns_error_for_unknown_method() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();
        let server = Server::new(dir.path()).unwrap();

        let req = protocol::Request {
            jsonrpc: "2.0".to_string(),
            id: json!(6),
            method: "unknown/method".to_string(),
            params: None,
        };

        let resp = server.handle_request(req).unwrap();
        match resp {
            Envelope::ErrorResponse(er) => {
                assert_eq!(er.error.code, protocol::RpcError::METHOD_NOT_FOUND);
            }
            _ => panic!("expected error response"),
        }
    }

    #[test]
    fn tools_call_changed_returns_results() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();
        let server = Server::new(dir.path()).unwrap();

        let req = protocol::Request {
            jsonrpc: "2.0".to_string(),
            id: json!(7),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "codetrail_changed",
                "arguments": {}
            })),
        };

        let resp = server.handle_request(req).unwrap();
        match resp {
            Envelope::SuccessResponse(sr) => {
                let result: ToolCallResult = serde_json::from_value(sr.result).unwrap();
                assert!(!result.is_error);
                let text = &result.content[0].text;
                let parsed: Value = serde_json::from_str(text).unwrap();
                assert!(parsed.get("ok").is_none());
                assert!(parsed["results"].as_array().is_some());
                assert!(parsed["page"].is_object());
            }
            _ => panic!("expected success response"),
        }
    }

    #[test]
    fn tools_call_status_returns_snapshot_id() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();
        let server = Server::new(dir.path()).unwrap();

        let req = protocol::Request {
            jsonrpc: "2.0".to_string(),
            id: json!(8),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "codetrail_status",
                "arguments": {}
            })),
        };

        let resp = server.handle_request(req).unwrap();
        match resp {
            Envelope::SuccessResponse(sr) => {
                let result: ToolCallResult = serde_json::from_value(sr.result).unwrap();
                assert!(!result.is_error);
                let text = &result.content[0].text;
                let parsed: Value = serde_json::from_str(text).unwrap();
                assert!(parsed.get("ok").is_none());
                let items = parsed["results"].as_array().unwrap();
                assert!(items[0]["snapshot_id"].as_str().is_some());
            }
            _ => panic!("expected success response"),
        }
    }

    #[test]
    fn tools_call_routes_scans_framework_routes() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/routes.rb"),
            "Rails.application.routes.draw do\n  get \"/health\", to: \"health#show\"\nend\n",
        )
        .unwrap();
        let server = Server::new(dir.path()).unwrap();

        let result = call_tool_json(
            &server,
            "codetrail_routes",
            json!({ "framework": ["rails"], "method": ["GET"] }),
        );
        assert!(result.get("ok").is_none());
        let routes = result["results"].as_array().unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0]["path"], "config/routes.rb");
        assert_eq!(routes[0]["framework"], "rails");
        assert_eq!(routes[0]["method"], "GET");
        assert_eq!(routes[0]["routePattern"], "/health");
        assert_eq!(routes[0]["handler"], "health#show");
    }

    #[test]
    fn tools_call_invalid_regex_returns_tool_error_envelope() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("sample.txt"), "hello\n").unwrap();
        let server = Server::new(dir.path()).unwrap();

        let result = call_tool(&server, "codetrail_grep", json!({ "pattern": "[" }));
        assert!(result.is_error);
        let parsed: Value = serde_json::from_str(&result.content[0].text).unwrap();
        assert!(!has_caveat(&parsed, "no_match"));
        assert!(parsed["caveats"][0]["code"].as_str().is_some());
    }

    #[test]
    fn tools_call_find_rejects_invalid_context_values() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("sample.txt"), "needle\n").unwrap();
        let server = Server::new(dir.path()).unwrap();

        for invalid_context in [json!(65536), json!(-1), json!(1.5)] {
            let result = call_tool(
                &server,
                "codetrail_find",
                json!({ "text": "needle", "context": invalid_context }),
            );
            assert!(result.is_error);
            let parsed: Value = serde_json::from_str(&result.content[0].text).unwrap();
            assert!(has_caveat(&parsed, "invalid_mcp_argument"));
        }
    }

    #[test]
    fn tools_call_find_returns_verifiable_source_range() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() {\n    let needle = 42;\n}\n",
        )
        .unwrap();
        let server = Server::new(dir.path()).unwrap();

        let found = call_tool_json(
            &server,
            "codetrail_find",
            json!({ "text": "needle", "context": 1 }),
        );
        assert!(found.get("ok").is_none());
        assert_eq!(found["results"][0]["path"], "src/main.rs");
        assert!(found["results"][0].get("readCommandArgv").is_none());
        assert!(found["results"][0].get("sourceTarget").is_none());
        assert_eq!(found["results"][0]["range"]["start"]["line"], 2);
    }

    #[test]
    fn tools_call_broad_query_uses_guarded_cli_contract() {
        let dir = tempdir().unwrap();
        for idx in 0..8 {
            fs::write(
                dir.path().join(format!("file{idx}.java")),
                "public class Sample {}\n",
            )
            .unwrap();
        }
        let server = Server::new(dir.path()).unwrap();

        let found = call_tool_json(&server, "codetrail_find", json!({ "text": "public" }));
        assert!(found["results"].as_array().unwrap().len() <= 5);
        assert!(has_caveat(&found, "broad_query_guard"));
        assert_eq!(found["caveats"].as_array().unwrap().len(), 1);
    }

    // ------------------------------------------------------------------
    //  CLI integration test  (E2E via process)
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    //  Argument parsing tests
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    //  Argument parsing tests
    // ------------------------------------------------------------------

    #[test]
    fn parse_query_options_extracts_include_exclude() {
        let args = json!({
            "include": ["src", "lib"],
            "exclude": ["test"],
            "lang": ["rust"],
            "changed": true,
            "cursor": "v1:abc:10",
            "allowBroad": true,
            "limit": 50,
            "context": 3
        });
        let opts = parse_query_options(Some(&args)).unwrap();
        assert_eq!(opts.include, vec!["src", "lib"]);
        assert_eq!(opts.exclude, vec!["test"]);
        assert_eq!(opts.lang, vec!["rust"]);
        assert!(opts.changed);
        assert_eq!(opts.cursor.as_deref(), Some("v1:abc:10"));
        assert!(opts.allow_broad);
        assert_eq!(opts.limit, 50);
        assert_eq!(opts.context, 3);
    }

    #[test]
    fn parse_query_options_uses_defaults_when_missing() {
        let args = json!({});
        let opts = parse_query_options(Some(&args)).unwrap();
        assert_eq!(opts.limit, 100);
        assert_eq!(opts.context, 0);
        assert!(opts.include.is_empty());
    }

    #[test]
    fn parse_query_options_rejects_invalid_numeric_values() {
        for args in [
            json!({ "context": 65536 }),
            json!({ "context": -1 }),
            json!({ "context": 1.5 }),
            json!({ "limit": -1 }),
            json!({ "limit": 1.5 }),
        ] {
            let error = parse_query_options(Some(&args)).unwrap_err();
            assert!(error.to_string().starts_with("invalid_mcp_argument:"));
        }
    }
}

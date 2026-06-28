//! Query service abstraction that wraps all codetrail operations into a
//! unified interface.  Each method delegates to the appropriate backend
//! (text index, SCIP, tree-sitter parser, filesystem, git status) and
//! returns a JSON value that carries reliability metadata.
//!
//! The outputs follow the same envelope convention as the CLI layer so that
//! both the CLI and MCP adapter can consume identical results.

use std::path::Path;

use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::{
    code_context::{self, CodeContextOptions},
    config_index, graph, java_semantic, output,
    query_input::{InputMode, InputPlan, SymbolMatchMode},
    routes, scip_index, search,
    search_pattern::SearchPatternMode,
    syntax,
    workspace::{RemoteMode, ScanOptions, Workspace},
};

// ---------------------------------------------------------------------------
// QueryOptions
// ---------------------------------------------------------------------------

/// Per-query filtering and display options.
#[derive(Clone, Debug)]
pub struct QueryOptions {
    pub dirs: Vec<String>,
    pub extensions: Vec<String>,
    pub file_patterns: Vec<String>,
    pub file_mode: SearchPatternMode,
    pub case_sensitive: bool,
    pub input_mode: InputMode,
    /// Path substrings that files must contain to be included.
    pub include: Vec<String>,
    /// Path substrings that exclude files.
    pub exclude: Vec<String>,
    /// Language names to include.
    pub lang: Vec<String>,
    /// Restrict to git changed files.
    pub changed: bool,
    /// Include hidden files/directories.
    pub hidden: bool,
    /// Ignore ignore files.
    pub no_ignore: bool,
    /// Pagination cursor.
    pub cursor: Option<String>,
    /// Allow broad queries to return full paginated results.
    pub allow_broad: bool,
    /// Maximum number of result items.
    pub limit: usize,
    /// Number of surrounding context lines (grep / find).
    pub context: u16,
    pub remote_mode: RemoteMode,
    pub remote_snapshot: Option<String>,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            include: vec![],
            exclude: vec![],
            dirs: vec![],
            extensions: vec![],
            file_patterns: vec![],
            file_mode: SearchPatternMode::Wildcard,
            case_sensitive: false,
            input_mode: InputMode::Compatible,
            lang: vec![],
            changed: false,
            hidden: false,
            no_ignore: false,
            cursor: None,
            allow_broad: false,
            limit: 100,
            context: 0,
            remote_mode: RemoteMode::Auto,
            remote_snapshot: None,
        }
    }
}

impl QueryOptions {
    pub fn from_scan_options(opts: &ScanOptions, context: u16) -> Self {
        Self {
            dirs: opts.dirs.clone(),
            extensions: opts.extensions.clone(),
            file_patterns: opts.file_patterns.clone(),
            file_mode: opts.file_mode,
            case_sensitive: opts.case_sensitive,
            input_mode: opts.input_mode,
            include: opts.include.clone(),
            exclude: opts.exclude.clone(),
            lang: opts.lang.clone(),
            changed: opts.changed,
            hidden: opts.hidden,
            no_ignore: opts.no_ignore,
            cursor: opts.cursor.clone(),
            allow_broad: opts.allow_broad,
            limit: opts.limit,
            context,
            remote_mode: opts.remote_mode,
            remote_snapshot: opts.remote_snapshot.clone(),
        }
    }

    fn to_scan_options(&self) -> ScanOptions {
        ScanOptions {
            dirs: self.dirs.clone(),
            extensions: self.extensions.clone(),
            file_patterns: self.file_patterns.clone(),
            file_mode: self.file_mode,
            case_sensitive: self.case_sensitive,
            input_mode: self.input_mode,
            include: self.include.clone(),
            exclude: self.exclude.clone(),
            lang: self.lang.clone(),
            changed: self.changed,
            cursor: self.cursor.clone(),
            allow_broad: self.allow_broad,
            hidden: self.hidden,
            no_ignore: self.no_ignore,
            limit: self.limit,
            remote_mode: self.remote_mode,
            remote_snapshot: self.remote_snapshot.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExploreNodeOptions {
    pub max_candidates: usize,
    pub snippet_lines: usize,
    pub relation_limit: usize,
    pub compact: bool,
    pub max_bytes: usize,
}

impl ExploreNodeOptions {
    pub fn bounded(max_candidates: usize, snippet_lines: usize, relation_limit: usize) -> Self {
        Self::with_budget(max_candidates, snippet_lines, relation_limit, false, 12_000)
    }

    pub fn with_budget(
        max_candidates: usize,
        snippet_lines: usize,
        relation_limit: usize,
        compact: bool,
        max_bytes: usize,
    ) -> Self {
        let max_candidates = max_candidates.clamp(1, 20);
        let snippet_lines = snippet_lines.clamp(1, 80);
        let relation_limit = relation_limit.min(20);
        Self {
            max_candidates: if compact {
                max_candidates.min(2)
            } else {
                max_candidates
            },
            snippet_lines: if compact {
                snippet_lines.min(8)
            } else {
                snippet_lines
            },
            relation_limit: if compact {
                relation_limit.min(4)
            } else {
                relation_limit
            },
            compact,
            max_bytes: max_bytes.clamp(1_000, 100_000),
        }
    }
}

// ---------------------------------------------------------------------------
// QueryService
// ---------------------------------------------------------------------------

/// Stable query-service facade that wraps [`Workspace`] and all backends.
pub struct QueryService {
    workspace: Workspace,
}

impl QueryService {
    /// Discover the workspace rooted at `root`.
    pub fn new(root: &Path) -> Result<Self> {
        let workspace = Workspace::discover(root)?;
        Ok(Self { workspace })
    }

    pub fn from_workspace(workspace: Workspace) -> Self {
        Self { workspace }
    }

    /// Expose the workspace snapshot id (used for reliability metadata).
    pub fn snapshot_id(&self) -> &str {
        &self.workspace.snapshot_id
    }

    fn finalize(&self, value: Value) -> Value {
        output::with_workspace_root(value, &self.workspace.root)
    }

    // ------------------------------------------------------------------
    //  Search operations
    // ------------------------------------------------------------------

    /// Full-text / literal search (delegates to `search::find`).
    pub fn find(&self, text: &str, opts: &QueryOptions) -> Result<Value> {
        self.text_search("find", text, SearchPatternMode::Literal, opts.context, opts)
    }

    /// Regex search (delegates to `search::find` with mode=regex).
    pub fn grep(&self, pattern: &str, opts: &QueryOptions) -> Result<Value> {
        self.text_search(
            "grep",
            pattern,
            SearchPatternMode::Regex,
            opts.context,
            opts,
        )
    }

    /// Full-text search with explicit command/mode metadata.
    pub fn text_search(
        &self,
        command: &str,
        pattern: &str,
        mode: SearchPatternMode,
        context: u16,
        opts: &QueryOptions,
    ) -> Result<Value> {
        let scan = opts.to_scan_options();
        let qo = search::find(&self.workspace, &scan, pattern, mode, context, false)?;
        let query = json!({
            "pattern": pattern,
            "mode": mode.as_str(),
            "caseSensitive": scan.case_sensitive,
            "context": context
        });
        let response = output::response_with_index(
            command,
            "find",
            scoped_query(query, &scan),
            &self.workspace.snapshot_id,
            output::source_fact(),
            output::IndexedResponseParts::new(
                qo.index.clone(),
                qo.results.clone(),
                remote_warnings(&qo.index, opts),
            ),
        );
        Ok(self.finalize(page_response(response, qo)))
    }

    /// Find files whose path contains `pattern` (substring match).
    pub fn files(&self, pattern: &str, opts: &QueryOptions) -> Result<Value> {
        self.files_with_mode("files", pattern, SearchPatternMode::Literal, opts)
    }

    pub fn files_with_mode(
        &self,
        command: &str,
        pattern: &str,
        mode: SearchPatternMode,
        opts: &QueryOptions,
    ) -> Result<Value> {
        let scan = opts.to_scan_options();
        let qo = search::files(&self.workspace, &scan, pattern, mode)?;
        let response = output::response_with_index(
            command,
            "files",
            scoped_query(
                json!({ "pattern": pattern, "mode": path_mode_label(command, mode), "caseSensitive": scan.case_sensitive }),
                &scan,
            ),
            &self.workspace.snapshot_id,
            output::source_fact(),
            output::IndexedResponseParts::new(
                qo.index.clone(),
                qo.results.clone(),
                remote_warnings(&qo.index, opts),
            ),
        );
        Ok(self.finalize(page_response(response, qo)))
    }

    /// Find files by strict glob pattern.
    pub fn glob(&self, pattern: &str, opts: &QueryOptions) -> Result<Value> {
        self.files_with_mode("glob", pattern, SearchPatternMode::Glob, opts)
    }

    // ------------------------------------------------------------------
    //  Navigation
    // ------------------------------------------------------------------

    /// Read file contents (optionally with a line-range like `path:1-10`).
    pub fn read_file(&self, target: &str) -> Result<Value> {
        let result = search::read(&self.workspace, target)?;
        let reliability = if result
            .get("exact")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            output::source_fact()
        } else {
            output::source_fact_inexact()
        };
        let warnings = result
            .get("warnings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
        Ok(self.finalize(output::response(
            "read",
            "read",
            json!({ "target": target }),
            &self.workspace.snapshot_id,
            reliability,
            json!([result]),
            warnings,
        )))
    }

    /// List directory contents.
    pub fn list(&self, dir: Option<&str>, recursive: bool, opts: &QueryOptions) -> Result<Value> {
        let scan = opts.to_scan_options();
        Ok(self.finalize(output::response(
            "list",
            "list",
            scoped_query(json!({ "dir": dir, "recursive": recursive }), &scan),
            &self.workspace.snapshot_id,
            output::source_fact(),
            search::list(&self.workspace, &scan, dir, recursive)?,
            Vec::new(),
        )))
    }

    /// List directory contents (non-recursive).
    pub fn list_dir(&self, dir: &str) -> Result<Value> {
        self.list(Some(dir), false, &QueryOptions::default())
    }

    /// Return a recursive tree view.
    pub fn tree(&self, dir: Option<&str>, depth: Option<u8>, opts: &QueryOptions) -> Result<Value> {
        let scan = opts.to_scan_options();
        Ok(self.finalize(output::response(
            "tree",
            "tree",
            scoped_query(json!({ "dir": dir, "depth": depth }), &scan),
            &self.workspace.snapshot_id,
            output::source_fact(),
            search::tree(&self.workspace, &scan, dir, depth)?,
            Vec::new(),
        )))
    }

    // ------------------------------------------------------------------
    //  Precise queries  (SCIP → parser fallback)
    // ------------------------------------------------------------------

    /// Find definitions of `identifier` — prefers SCIP; falls back to tree-sitter.
    pub fn defs(&self, identifier: &str, opts: &QueryOptions) -> Result<Value> {
        self.defs_with_code(identifier, opts, &CodeContextOptions::default())
    }

    pub fn defs_with_code(
        &self,
        identifier: &str,
        opts: &QueryOptions,
        code_options: &CodeContextOptions,
    ) -> Result<Value> {
        let scan = opts.to_scan_options();

        // 1. Try SCIP precise index first.
        let precise_empty = if let Some(precise) =
            scip_index::defs(&self.workspace, &scan, identifier)?
        {
            if has_results(&precise.results) {
                let mut results = precise.results;
                let mut index = precise.index;
                if let Some(config) = config_index::defs(&self.workspace, &scan, identifier)? {
                    append_results(&mut results, config.results);
                    append_config_index(&mut index, config.index);
                }
                search::rank_and_truncate_code_results(
                    &mut results,
                    identifier,
                    &scan,
                    SymbolMatchMode::Exact,
                );
                let response = output::response_with_index(
                    "defs",
                    "defs",
                    scoped_query(
                        code_context::query_with_code_options(
                            json!({ "identifier": identifier, "producer": "scip" }),
                            code_options,
                        ),
                        &scan,
                    ),
                    &self.workspace.snapshot_id,
                    output::precise_fact(),
                    output::IndexedResponseParts::new(index, results, Vec::new()),
                );
                let response =
                    code_context::enrich_response(&self.workspace, &scan, response, code_options)?;
                return Ok(self.finalize(response));
            }
            Some(precise)
        } else {
            None
        };

        // 2. Fall back to tree-sitter parser.
        let (mut results, warnings) = syntax::defs(&self.workspace, &scan, identifier)?;
        let parser_had_results = has_results(&results);
        if let Some(config) = config_index::defs(&self.workspace, &scan, identifier)? {
            append_results(&mut results, config.results);
        }
        search::rank_and_truncate_code_results(
            &mut results,
            identifier,
            &scan,
            SymbolMatchMode::Exact,
        );
        if !has_results(&results) {
            if let Some(precise) = precise_empty {
                let response = output::response_with_index(
                    "defs",
                    "defs",
                    scoped_query(
                        code_context::query_with_code_options(
                            json!({ "identifier": identifier, "producer": "scip" }),
                            code_options,
                        ),
                        &scan,
                    ),
                    &self.workspace.snapshot_id,
                    output::precise_fact(),
                    output::IndexedResponseParts::new(precise.index, precise.results, Vec::new()),
                );
                let response =
                    code_context::enrich_response(&self.workspace, &scan, response, code_options)?;
                return Ok(self.finalize(response));
            }
        }
        let response = output::response(
            "defs",
            "defs",
            scoped_query(
                code_context::query_with_code_options(
                    json!({ "identifier": identifier, "producer": "tree_sitter_parser_fallback", "fallbackReason": "precise_scip_index_unavailable" }),
                    code_options,
                ),
                &scan,
            ),
            &self.workspace.snapshot_id,
            parser_reliability(parser_had_results, &results),
            results,
            merge_warnings(
                warnings,
                vec![
                    "precise_scip_index_unavailable: using tree-sitter parser fallback".to_string(),
                ],
            ),
        );
        let response =
            code_context::enrich_response(&self.workspace, &scan, response, code_options)?;
        Ok(self.finalize(response))
    }

    /// Find references to `identifier` from a precise SCIP occurrence index.
    pub fn refs(&self, identifier: &str, opts: &QueryOptions) -> Result<Value> {
        let scan = opts.to_scan_options();

        if let Some(precise) = scip_index::refs(&self.workspace, &scan, identifier)? {
            if has_results(&precise.results) {
                let mut results = precise.results;
                let mut index = precise.index;
                if let Some(config) = config_index::refs(&self.workspace, &scan, identifier)? {
                    append_results(&mut results, config.results);
                    truncate_results_to_limit(&mut results, scan.limit);
                    append_config_index(&mut index, config.index);
                }
                return Ok(self.finalize(output::response_with_index(
                    "refs",
                    "refs",
                    scoped_query(
                        json!({ "identifier": identifier, "producer": "scip" }),
                        &scan,
                    ),
                    &self.workspace.snapshot_id,
                    output::precise_fact(),
                    output::IndexedResponseParts::new(index, results, Vec::new()),
                )));
            }

            return Ok(self.finalize(output::response_with_index(
                "refs",
                "refs",
                scoped_query(
                    json!({ "identifier": identifier, "producer": "scip" }),
                    &scan,
                ),
                &self.workspace.snapshot_id,
                output::precise_fact(),
                output::IndexedResponseParts::new(precise.index, precise.results, Vec::new()),
            )));
        }

        Ok(self.finalize(output::response_with_index(
            "refs",
            "refs",
            scoped_query(
                json!({ "identifier": identifier, "producer": "scip", "requires": "fresh_scip_occurrence_index" }),
                &scan,
            ),
            &self.workspace.snapshot_id,
            output::freshness(),
            output::IndexedResponseParts::new(
                output::live_scan_index(),
                json!([]),
                vec![
                    "precise_scip_index_unavailable: refs requires a fresh SCIP occurrence index; use ripgrep for textual matches"
                        .to_string(),
                ],
            ),
        )))
    }

    /// Find symbols matching `query` — prefers SCIP; falls back to tree-sitter.
    pub fn symbols(&self, query: &str, opts: &QueryOptions) -> Result<Value> {
        self.symbols_with_code(query, opts, &CodeContextOptions::default())
    }

    pub fn symbols_with_code(
        &self,
        query: &str,
        opts: &QueryOptions,
        code_options: &CodeContextOptions,
    ) -> Result<Value> {
        let scan = opts.to_scan_options();

        // 1. Try SCIP precise index first.
        let precise_empty = if let Some(precise) =
            scip_index::symbols(&self.workspace, &scan, query)?
        {
            if has_results(&precise.results) {
                let mut results = precise.results;
                let mut index = precise.index;
                if let Some(config) = config_index::symbols(&self.workspace, &scan, query)? {
                    append_results(&mut results, config.results);
                    append_config_index(&mut index, config.index);
                }
                let page = search::page_ranked_code_results(
                    results,
                    &scan,
                    "symbols",
                    code_context::query_with_code_options(
                        json!({ "query": query, "producer": "scip" }),
                        code_options,
                    ),
                    &self.workspace.snapshot_id,
                    query,
                    SymbolMatchMode::Contains,
                )?;
                let response = output::response_with_index(
                    "symbols",
                    "symbols",
                    scoped_query(
                        code_context::query_with_code_options(
                            json!({ "query": query, "producer": "scip" }),
                            code_options,
                        ),
                        &scan,
                    ),
                    &self.workspace.snapshot_id,
                    output::precise_fact(),
                    output::IndexedResponseParts::new(index, page.results.clone(), Vec::new()),
                );
                let response =
                    output::with_page_meta(response, page.truncated, page.next_cursor, page.facets);
                let response =
                    code_context::enrich_response(&self.workspace, &scan, response, code_options)?;
                return Ok(self.finalize(response));
            }
            Some(precise)
        } else {
            None
        };

        // 2. Fall back to tree-sitter.
        let (mut results, warnings) = syntax::symbols(&self.workspace, &scan, query)?;
        let parser_had_results = has_results(&results);
        if let Some(config) = config_index::symbols(&self.workspace, &scan, query)? {
            append_results(&mut results, config.results);
        }
        let fallback_had_results = has_results(&results);
        if !fallback_had_results {
            if let Some(precise) = precise_empty {
                let response = output::response_with_index(
                    "symbols",
                    "symbols",
                    scoped_query(
                        code_context::query_with_code_options(
                            json!({ "query": query, "producer": "scip" }),
                            code_options,
                        ),
                        &scan,
                    ),
                    &self.workspace.snapshot_id,
                    output::precise_fact(),
                    output::IndexedResponseParts::new(precise.index, precise.results, Vec::new()),
                );
                let response =
                    code_context::enrich_response(&self.workspace, &scan, response, code_options)?;
                return Ok(self.finalize(response));
            }
        }
        let page = search::page_ranked_code_results(
            results,
            &scan,
            "symbols",
            code_context::query_with_code_options(
                json!({ "query": query, "producer": "tree_sitter_parser" }),
                code_options,
            ),
            &self.workspace.snapshot_id,
            query,
            SymbolMatchMode::Contains,
        )?;
        let response = output::response(
            "symbols",
            "symbols",
            scoped_query(
                code_context::query_with_code_options(
                    json!({ "query": query, "producer": "tree_sitter_parser" }),
                    code_options,
                ),
                &scan,
            ),
            &self.workspace.snapshot_id,
            parser_reliability(parser_had_results, &page.results),
            page.results.clone(),
            merge_warnings(
                warnings,
                vec![
                    "precise_scip_index_unavailable: using tree-sitter parser fallback".to_string(),
                ],
            ),
        );
        let response =
            output::with_page_meta(response, page.truncated, page.next_cursor, page.facets);
        let response =
            code_context::enrich_response(&self.workspace, &scan, response, code_options)?;
        Ok(self.finalize(response))
    }

    pub fn routes(
        &self,
        pattern: Option<&str>,
        frameworks: &[String],
        methods: &[String],
        opts: &QueryOptions,
    ) -> Result<Value> {
        self.routes_with_mode(
            pattern,
            SearchPatternMode::Literal,
            frameworks,
            methods,
            opts,
        )
    }

    pub fn routes_with_mode(
        &self,
        pattern: Option<&str>,
        mode: SearchPatternMode,
        frameworks: &[String],
        methods: &[String],
        opts: &QueryOptions,
    ) -> Result<Value> {
        let scan = opts.to_scan_options();
        let output = routes::scan(&self.workspace, &scan, pattern, mode, frameworks, methods)?;
        let response = output::response_with_index(
            "routes",
            "routes",
            scoped_query(
                json!({
                    "pattern": pattern,
                    "mode": mode.as_str(),
                    "framework": frameworks,
                    "method": methods,
                    "producer": "framework_route_scanner"
                }),
                &scan,
            ),
            &self.workspace.snapshot_id,
            output::parser_fact(),
            output::IndexedResponseParts::new(
                output.index.clone(),
                output.results.clone(),
                Vec::new(),
            ),
        );
        Ok(self.finalize(page_response(response, output)))
    }

    pub fn explore_node(
        &self,
        query: &str,
        opts: &QueryOptions,
        explore: ExploreNodeOptions,
    ) -> Result<Value> {
        let explore = ExploreNodeOptions::with_budget(
            explore.max_candidates,
            explore.snippet_lines,
            explore.relation_limit,
            explore.compact,
            explore.max_bytes,
        );
        let mut query_opts = opts.clone();
        query_opts.limit = explore.max_candidates;
        query_opts.allow_broad = true;
        let code_options = CodeContextOptions {
            include_code: true,
            code_context: 0,
            code_max_lines: explore.snippet_lines,
        };

        let mut warnings = Vec::new();
        let mut producer = "defs";
        let mut response = self.defs_with_code(query, &query_opts, &code_options)?;
        if !has_results(&response["results"]) {
            warnings
                .push("explore_fallback: defs returned no candidates; tried symbols".to_string());
            producer = "symbols";
            response = self.symbols_with_code(query, &query_opts, &code_options)?;
        }
        if !has_results(&response["results"]) {
            warnings
                .push("explore_fallback: symbols returned no candidates; tried files".to_string());
            producer = "files";
            response = self.files(query, &query_opts)?;
        }

        warnings.extend(warning_strings(&response));
        let mut relation_seen = false;
        let mut relation_truncated = false;
        let mut source_truncated = false;
        let mut compact = Vec::new();
        for result in response["results"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .take(explore.max_candidates)
        {
            let (result_index, result) = result;
            let relation_limit = if explore.compact && result_index > 0 {
                0
            } else {
                explore.relation_limit
            };
            let (relations, relation_warnings) =
                self.explore_relations(result, query, &query_opts, relation_limit)?;
            warnings.extend(relation_warnings);
            let item = compact_explore_result(
                &self.workspace,
                result,
                producer,
                response
                    .pointer("/reliability/level")
                    .and_then(Value::as_str),
                explore.snippet_lines,
                Some(&relations),
                relation_limit,
            )?;
            relation_seen |= item
                .get("relations")
                .and_then(|relations| relation_has_items(relations))
                .unwrap_or(false);
            relation_truncated |= item
                .pointer("/relations/truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            source_truncated |= item
                .get("snippetTruncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            compact.push(item);
        }
        if apply_result_byte_budget(&mut compact, explore.max_bytes) {
            warnings.push(format!(
                "output_truncated: explore node results were capped at {} bytes",
                explore.max_bytes
            ));
        }

        if relation_seen {
            warnings.push(
                "inferred_candidate: explore node relations are candidate call graph evidence"
                    .to_string(),
            );
        }
        if relation_truncated {
            warnings.push("relations_truncated: explore node relations were capped".to_string());
        }
        if source_truncated {
            warnings.push("source_truncated: explore node snippets were capped".to_string());
        }
        if producer != "defs" {
            warnings.push(format!("explore_fallback: used {producer} fallback"));
        }

        let scan = query_opts.to_scan_options();
        Ok(self.finalize(output::response(
            "explore node",
            "explore node",
            scoped_query(
                json!({
                    "query": query,
                    "producer": producer,
                    "maxCandidates": explore.max_candidates,
                    "snippetLines": explore.snippet_lines,
                    "relationLimit": explore.relation_limit,
                    "compact": explore.compact,
                    "maxBytes": explore.max_bytes
                }),
                &scan,
            ),
            &self.workspace.snapshot_id,
            reliability_from_response(&response),
            Value::Array(compact),
            dedupe_warnings(warnings),
        )))
    }

    fn explore_relations(
        &self,
        result: &Value,
        fallback_query: &str,
        opts: &QueryOptions,
        relation_limit: usize,
    ) -> Result<(Value, Vec<String>)> {
        if relation_limit == 0 {
            return Ok((
                json!({ "calls": [], "callers": [], "truncated": false }),
                Vec::new(),
            ));
        }
        let identifier = relation_identifier(result).unwrap_or(fallback_query);
        let mut relation_opts = opts.clone();
        relation_opts.limit = relation_limit;
        let calls = self.calls(identifier, &relation_opts)?;
        let callers = self.callers(identifier, &relation_opts)?;
        let mut warnings = relation_warning_strings(&calls);
        warnings.extend(relation_warning_strings(&callers));
        let calls_results = calls.get("results").cloned().unwrap_or_else(|| json!([]));
        let callers_results = callers.get("results").cloned().unwrap_or_else(|| json!([]));
        let truncated = calls
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || callers
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        Ok((
            json!({
                "calls": calls_results,
                "callers": callers_results,
                "truncated": truncated
            }),
            warnings,
        ))
    }

    // ------------------------------------------------------------------
    //  Relation queries  (graph → tree-sitter fallback)
    // ------------------------------------------------------------------

    /// Find outgoing calls from `identifier`.
    pub fn calls(&self, identifier: &str, opts: &QueryOptions) -> Result<Value> {
        let scan = opts.to_scan_options();

        if let Some((index_meta, results)) =
            java_semantic::calls(&self.workspace, &scan, identifier)?
        {
            return Ok(self.finalize(output::response_with_index(
                "calls",
                "calls",
                scoped_query(
                    json!({ "identifier": identifier, "producer": "java_semantic" }),
                    &scan,
                ),
                &self.workspace.snapshot_id,
                output::inferred_candidate(),
                output::IndexedResponseParts::new(index_meta, results, Vec::new()),
            )));
        }

        // 1. Try graph backend first.
        let graph_store = graph::GraphStore::open(&self.workspace).ok();
        if let Some(ref store) = graph_store {
            if store.freshness_check().unwrap_or(false) {
                let plan = InputPlan::new(identifier, scan.input_mode);
                let results = store
                    .query_calls_with_input(&plan, scan.case_sensitive)
                    .and_then(|results| {
                        graph::filter_candidates_by_scan_scope(&self.workspace, &scan, results)
                    })
                    .unwrap_or_default();
                if !results.is_empty() {
                    let index_meta = store.index_meta(true);
                    return Ok(self.finalize(output::response_with_index(
                        "calls",
                        "calls",
                        scoped_query(
                            json!({ "identifier": identifier, "producer": "graph" }),
                            &scan,
                        ),
                        &self.workspace.snapshot_id,
                        output::inferred_candidate(),
                        output::IndexedResponseParts::new(index_meta, json!(results), Vec::new()),
                    )));
                }
            }
        }

        // 2. Fall back to tree-sitter heuristic.
        let (results, warnings) = syntax::calls(&self.workspace, &scan, identifier)?;
        Ok(self.finalize(output::response(
            "calls",
            "calls",
            scoped_query(
                json!({ "identifier": identifier, "producer": "tree_sitter_call_heuristic" }),
                &scan,
            ),
            &self.workspace.snapshot_id,
            output::inferred_candidate(),
            results,
            warnings,
        )))
    }

    /// Find incoming callers of `identifier`.
    pub fn callers(&self, identifier: &str, opts: &QueryOptions) -> Result<Value> {
        let scan = opts.to_scan_options();

        if let Some((index_meta, results)) =
            java_semantic::callers(&self.workspace, &scan, identifier)?
        {
            return Ok(self.finalize(output::response_with_index(
                "callers",
                "callers",
                scoped_query(
                    json!({ "identifier": identifier, "producer": "java_semantic" }),
                    &scan,
                ),
                &self.workspace.snapshot_id,
                output::inferred_candidate(),
                output::IndexedResponseParts::new(index_meta, results, Vec::new()),
            )));
        }

        // 1. Try graph backend first.
        let graph_store = graph::GraphStore::open(&self.workspace).ok();
        if let Some(ref store) = graph_store {
            if store.freshness_check().unwrap_or(false) {
                let plan = InputPlan::new(identifier, scan.input_mode);
                let results = store
                    .query_callers_with_input(&plan, scan.case_sensitive)
                    .and_then(|results| {
                        graph::filter_candidates_by_scan_scope(&self.workspace, &scan, results)
                    })
                    .unwrap_or_default();
                if !results.is_empty() {
                    let index_meta = store.index_meta(true);
                    return Ok(self.finalize(output::response_with_index(
                        "callers",
                        "callers",
                        scoped_query(
                            json!({ "identifier": identifier, "producer": "graph" }),
                            &scan,
                        ),
                        &self.workspace.snapshot_id,
                        output::inferred_candidate(),
                        output::IndexedResponseParts::new(index_meta, json!(results), Vec::new()),
                    )));
                }
            }
        }

        // 2. Fall back to tree-sitter heuristic.
        let (results, warnings) = syntax::callers(&self.workspace, &scan, identifier)?;
        Ok(self.finalize(output::response(
            "callers",
            "callers",
            scoped_query(
                json!({ "identifier": identifier, "producer": "tree_sitter_call_heuristic" }),
                &scan,
            ),
            &self.workspace.snapshot_id,
            output::inferred_candidate(),
            results,
            warnings,
        )))
    }

    pub fn call_hierarchy(
        &self,
        identifier: &str,
        opts: &QueryOptions,
        hierarchy_opts: java_semantic::CallHierarchyOptions,
    ) -> Result<Value> {
        let scan = opts.to_scan_options();
        if let Some((index_meta, results)) =
            java_semantic::query_call_hierarchy(&self.workspace, &scan, identifier, hierarchy_opts)?
        {
            return Ok(self.finalize(output::response_with_index(
                "call-hierarchy",
                "call-hierarchy",
                scoped_query(
                    json!({
                        "identifier": identifier,
                        "producer": "java_semantic",
                        "direction": hierarchy_opts.direction.as_str(),
                        "depth": hierarchy_opts.depth,
                        "includeOverrides": hierarchy_opts.include_overrides,
                    }),
                    &scan,
                ),
                &self.workspace.snapshot_id,
                output::inferred_candidate(),
                output::IndexedResponseParts::new(index_meta, results, Vec::new()),
            )));
        }

        if let Some((index_meta, results)) =
            self.graph_call_hierarchy(identifier, &scan, hierarchy_opts)?
        {
            return Ok(self.finalize(output::response_with_index(
                "call-hierarchy",
                "call-hierarchy",
                scoped_query(
                    json!({
                        "identifier": identifier,
                        "producer": "graph",
                        "direction": hierarchy_opts.direction.as_str(),
                        "depth": hierarchy_opts.depth,
                    }),
                    &scan,
                ),
                &self.workspace.snapshot_id,
                output::inferred_candidate(),
                output::IndexedResponseParts::new(index_meta, results, Vec::new()),
            )));
        }

        Ok(self.finalize(output::response_with_index(
            "call-hierarchy",
            "call-hierarchy",
            scoped_query(
                json!({
                        "identifier": identifier,
                        "producer": "graph",
                        "direction": hierarchy_opts.direction.as_str(),
                        "depth": hierarchy_opts.depth,
                        "requires": "fresh_call_hierarchy_index"
                    }),
                &scan,
            ),
            &self.workspace.snapshot_id,
            output::freshness(),
            output::IndexedResponseParts::new(
                output::live_scan_index(),
                json!([]),
                vec![
                    "Call hierarchy index unavailable; run `codetrail index build` to create call hierarchy data."
                        .to_string(),
                ],
            ),
        )))
    }

    fn graph_call_hierarchy(
        &self,
        identifier: &str,
        scan: &ScanOptions,
        hierarchy_opts: java_semantic::CallHierarchyOptions,
    ) -> Result<Option<(Value, Value)>> {
        let store = graph::GraphStore::open(&self.workspace)?;
        if !store.freshness_check().unwrap_or(false) {
            return Ok(None);
        }
        let direction = match hierarchy_opts.direction {
            java_semantic::CallHierarchyDirection::Incoming => {
                graph::schema::HierarchyDirection::Incoming
            }
            java_semantic::CallHierarchyDirection::Outgoing => {
                graph::schema::HierarchyDirection::Outgoing
            }
            java_semantic::CallHierarchyDirection::Both => graph::schema::HierarchyDirection::Both,
        };
        let results = store.query_call_hierarchy(
            &self.workspace,
            scan,
            identifier,
            direction,
            hierarchy_opts.depth,
        )?;
        Ok(Some((store.index_meta(true), Value::Array(results))))
    }

    // ------------------------------------------------------------------
    //  Status
    // ------------------------------------------------------------------

    /// Return a list of changed / dirty files (git-status porcelain).
    pub fn changed(&self) -> Result<Value> {
        Ok(self.finalize(output::with_summary_field(
            output::response(
                "changed",
                "changed",
                json!({}),
                &self.workspace.snapshot_id,
                output::source_fact(),
                search::changed(&self.workspace)?,
                Vec::new(),
            ),
            "changed",
            search::changed_summary(&self.workspace),
        )))
    }

    /// Return workspace status including snapshot_id, dirty flag, etc.
    pub fn status(&self) -> Result<Value> {
        Ok(self.finalize(output::response(
            "status",
            "status",
            json!({}),
            &self.workspace.snapshot_id,
            output::source_fact(),
            json!([search::status(&self.workspace)]),
            Vec::new(),
        )))
    }
}

fn scoped_query(mut query: Value, opts: &ScanOptions) -> Value {
    if let Some(object) = query.as_object_mut() {
        object.insert("scope".to_string(), search::scope_value(opts));
    }
    query
}

fn page_response(value: Value, page: search::QueryOutput) -> Value {
    let page_value = output::with_budget(
        output::with_guard(
            output::with_page_meta(
                value,
                page.truncated,
                page.next_cursor.clone(),
                page.facets.clone(),
            ),
            page.guard.clone(),
        ),
        page.budget.clone(),
    );
    search::attach_query_diagnostics(page_value, &page)
}

fn has_results(value: &Value) -> bool {
    value.as_array().is_some_and(|results| !results.is_empty())
}

fn append_results(target: &mut Value, extra: Value) {
    let Some(target_items) = target.as_array_mut() else {
        return;
    };
    if let Value::Array(mut extra_items) = extra {
        target_items.append(&mut extra_items);
    }
}

fn truncate_results_to_limit(results: &mut Value, limit: usize) {
    if limit == 0 {
        return;
    }
    if let Some(items) = results.as_array_mut() {
        items.truncate(limit);
    }
}

fn append_config_index(target: &mut Value, config_index: Value) {
    if let Some(object) = target.as_object_mut() {
        object.insert("configFacts".to_string(), config_index);
    }
}

fn parser_reliability(parser_had_results: bool, results: &Value) -> output::Reliability {
    if !parser_had_results && has_results(results) {
        output::config_fact()
    } else {
        output::parser_fact()
    }
}

fn path_mode_label(command: &str, mode: SearchPatternMode) -> &'static str {
    match (command, mode) {
        ("files" | "find-path", SearchPatternMode::Literal) => "path_substring",
        ("glob", SearchPatternMode::Glob) => "strict_glob",
        (_, mode) => mode.as_str(),
    }
}

fn merge_warnings(mut first: Vec<String>, second: Vec<String>) -> Vec<String> {
    first.extend(second);
    first
}

fn remote_warnings(index: &Value, opts: &QueryOptions) -> Vec<String> {
    if index.get("source").and_then(Value::as_str) != Some("text_index:remote") {
        return Vec::new();
    }
    let snapshot = index
        .get("remote_snapshot_key")
        .and_then(Value::as_str)
        .or_else(|| index.get("snapshotKey").and_then(Value::as_str))
        .unwrap_or("unknown");
    let mut warnings = Vec::new();
    if opts.remote_mode == RemoteMode::Only || opts.remote_snapshot.is_some() {
        warnings.push(format!(
            "remote_only: query used remote snapshot {snapshot}; results are not local edit facts"
        ));
    }
    if index
        .get("remote_verified")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        == false
    {
        warnings.push(format!(
            "remote_unverified: remote snapshot {snapshot} does not match current local files"
        ));
    }
    warnings
}

fn reliability_from_response(response: &Value) -> output::Reliability {
    match response
        .pointer("/reliability/level")
        .and_then(Value::as_str)
        .unwrap_or("parser_fact")
    {
        "precise_fact" => output::precise_fact(),
        "source_fact" => output::source_fact(),
        "inferred_candidate" => output::inferred_candidate(),
        "config_fact" => output::config_fact(),
        _ => output::parser_fact(),
    }
}

fn compact_explore_result(
    workspace: &Workspace,
    result: &Value,
    producer: &str,
    response_layer: Option<&str>,
    snippet_lines: usize,
    relations: Option<&Value>,
    relation_limit: usize,
) -> Result<Value> {
    let mut object = Map::new();
    copy_field(result, &mut object, "path");
    copy_field(result, &mut object, "range");
    copy_field(result, &mut object, "bodyRange");
    copy_field(result, &mut object, "language");
    copy_field(result, &mut object, "kind");
    copy_field(result, &mut object, "name");
    copy_field(result, &mut object, "symbolName");
    copy_field(result, &mut object, "qualifiedName");
    copy_field(result, &mut object, "signature");
    copy_field(result, &mut object, "container");
    copy_field(result, &mut object, "target");
    copy_field(result, &mut object, "targetDetail");
    copy_field(result, &mut object, "targetSignature");
    copy_field(result, &mut object, "targetSymbolId");
    copy_field(result, &mut object, "enclosingSymbol");
    copy_field(result, &mut object, "enclosingSymbolDetail");
    copy_field(result, &mut object, "enclosingSymbolSignature");
    copy_field(result, &mut object, "enclosingSymbolId");
    let layer = result
        .get("layer")
        .and_then(Value::as_str)
        .or(response_layer)
        .unwrap_or("parser_fact");
    object.insert("layer".to_string(), Value::String(layer.to_string()));

    if let Some(source) = result.get("source") {
        if let Some(content) = source.get("content").and_then(Value::as_str) {
            object.insert("snippet".to_string(), Value::String(content.to_string()));
        }
        if source.get("truncated").and_then(Value::as_bool) == Some(true) {
            object.insert("snippetTruncated".to_string(), Value::Bool(true));
        }
    } else if producer == "files" {
        if let Some(path) = result.get("path").and_then(Value::as_str) {
            if let Some((snippet, range, truncated)) =
                read_file_snippet(workspace, path, snippet_lines)?
            {
                object.insert("snippet".to_string(), Value::String(snippet));
                object.insert("range".to_string(), range);
                if truncated {
                    object.insert("snippetTruncated".to_string(), Value::Bool(true));
                }
            }
        }
    }

    object.insert(
        "relations".to_string(),
        compact_relations(
            relations.or_else(|| result.get("relations")),
            relation_limit,
        ),
    );
    if let (Some(path), Some(range)) = (
        object.get("path").and_then(Value::as_str),
        object.get("range"),
    ) {
        if let Some(target) = cite_target(path, range) {
            object.insert("citeTarget".to_string(), Value::String(target));
        }
    }
    Ok(Value::Object(object))
}

fn read_file_snippet(
    workspace: &Workspace,
    path: &str,
    snippet_lines: usize,
) -> Result<Option<(String, Value, bool)>> {
    let target = format!("{path}:1-{}", snippet_lines.max(1));
    let read = search::read(workspace, &target)?;
    if read.get("binary").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    let content = read
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let range = read.get("range").cloned().unwrap_or_else(|| {
        json!({
            "start": { "line": 1, "column": 1 },
            "end": { "line": snippet_lines.max(1), "column": 1 }
        })
    });
    let truncated = read
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(Some((content, range, truncated)))
}

fn relation_identifier(result: &Value) -> Option<&str> {
    result
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| result.get("symbolName").and_then(Value::as_str))
        .or_else(|| result.get("target").and_then(Value::as_str))
        .or_else(|| result.get("enclosingSymbol").and_then(Value::as_str))
}

fn compact_relations(relations: Option<&Value>, limit: usize) -> Value {
    let Some(relations) = relations else {
        return json!({ "calls": [], "callers": [], "truncated": false });
    };
    let calls_all = relations
        .get("calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let callers_all = relations
        .get("callers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut remaining = limit;
    let calls = take_compact_relations(&calls_all, &mut remaining);
    let callers = take_compact_relations(&callers_all, &mut remaining);
    let truncated = relations
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || calls_all.len() + callers_all.len() > calls.len() + callers.len();
    json!({
        "calls": calls,
        "callers": callers,
        "truncated": truncated,
    })
}

fn take_compact_relations(items: &[Value], remaining: &mut usize) -> Vec<Value> {
    let mut output = Vec::new();
    for item in items {
        if *remaining == 0 {
            break;
        }
        let mut object = Map::new();
        copy_field(item, &mut object, "path");
        copy_field(item, &mut object, "range");
        copy_field(item, &mut object, "language");
        copy_field(item, &mut object, "kind");
        copy_field(item, &mut object, "target");
        copy_field(item, &mut object, "targetDetail");
        copy_field(item, &mut object, "targetSignature");
        copy_field(item, &mut object, "targetSymbolId");
        copy_field(item, &mut object, "enclosingSymbol");
        copy_field(item, &mut object, "enclosingSymbolDetail");
        copy_field(item, &mut object, "enclosingSymbolSignature");
        copy_field(item, &mut object, "enclosingSymbolId");
        copy_field(item, &mut object, "layer");
        output.push(Value::Object(object));
        *remaining -= 1;
    }
    output
}

fn relation_has_items(relations: &Value) -> Option<bool> {
    Some(
        relations
            .get("calls")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
            || relations
                .get("callers")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty()),
    )
}

fn cite_target(path: &str, range: &Value) -> Option<String> {
    let start = range.pointer("/start/line").and_then(Value::as_u64)?;
    let end = range
        .pointer("/end/line")
        .and_then(Value::as_u64)
        .unwrap_or(start);
    if start == end {
        Some(format!("{path}:{start}"))
    } else {
        Some(format!("{path}:{start}-{end}"))
    }
}

fn apply_result_byte_budget(results: &mut Vec<Value>, max_bytes: usize) -> bool {
    if value_json_len(&Value::Array(results.clone())) <= max_bytes {
        return false;
    }
    let mut truncated = trim_snippets(results, 800);
    if value_json_len(&Value::Array(results.clone())) > max_bytes {
        truncated |= trim_snippets(results, 400);
    }
    if value_json_len(&Value::Array(results.clone())) > max_bytes {
        truncated |= trim_snippets(results, 160);
    }
    if value_json_len(&Value::Array(results.clone())) > max_bytes {
        for result in results.iter_mut() {
            if let Some(object) = result.as_object_mut() {
                object.insert(
                    "relations".to_string(),
                    json!({ "calls": [], "callers": [], "truncated": true }),
                );
            }
        }
        truncated = true;
    }
    while results.len() > 1 && value_json_len(&Value::Array(results.clone())) > max_bytes {
        results.pop();
        truncated = true;
    }
    truncated
}

fn trim_snippets(values: &mut [Value], max_chars: usize) -> bool {
    let mut changed = false;
    for value in values {
        changed |= trim_snippets_in_value(value, max_chars);
    }
    changed
}

fn trim_snippets_in_value(value: &mut Value, max_chars: usize) -> bool {
    match value {
        Value::Object(object) => {
            let mut changed = false;
            if let Some(snippet_value) = object.get_mut("snippet") {
                let trimmed = snippet_value.as_str().and_then(|snippet| {
                    (snippet.chars().count() > max_chars)
                        .then(|| snippet.chars().take(max_chars).collect::<String>())
                });
                if let Some(trimmed) = trimmed {
                    *snippet_value = Value::String(format!("{trimmed}\n..."));
                    object.insert("snippetTruncated".to_string(), Value::Bool(true));
                    changed = true;
                };
            }
            for value in object.values_mut() {
                changed |= trim_snippets_in_value(value, max_chars);
            }
            changed
        }
        Value::Array(values) => trim_snippets(values, max_chars),
        _ => false,
    }
}

fn value_json_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

fn warning_strings(response: &Value) -> Vec<String> {
    response
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|warning| {
            let code = warning.get("code").and_then(Value::as_str)?;
            let message = warning
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or(code);
            Some(format!("{code}: {message}"))
        })
        .collect()
}

fn relation_warning_strings(response: &Value) -> Vec<String> {
    warning_strings(response)
        .into_iter()
        .filter(|warning| !warning.starts_with("no_match:"))
        .collect()
}

fn dedupe_warnings(warnings: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    warnings
        .into_iter()
        .filter(|warning| seen.insert(warning.clone()))
        .collect()
}

fn copy_field(source: &Value, target: &mut Map<String, Value>, field: &str) {
    if let Some(value) = source.get(field).filter(|value| !value.is_null()) {
        target.insert(field.to_string(), value.clone());
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

    fn setup_with_file(name: &str, content: &str) -> (tempfile::TempDir, QueryService) {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join(name), content).unwrap();
        let svc = QueryService::new(dir.path()).unwrap();
        (dir, svc)
    }

    // -- QueryOptions → ScanOptions round-trip --------------------------

    #[test]
    fn query_options_default_is_sensible() {
        let opts = QueryOptions::default();
        assert_eq!(opts.limit, 100);
        assert_eq!(opts.context, 0);
        assert!(opts.include.is_empty());
        assert!(opts.exclude.is_empty());
    }

    #[test]
    fn query_options_to_scan_options_preserves_limit() {
        let opts = QueryOptions {
            limit: 42,
            ..Default::default()
        };
        let scan = opts.to_scan_options();
        assert_eq!(scan.limit, 42);
    }

    // -- find -----------------------------------------------------------

    #[test]
    fn find_returns_source_fact_reliability() {
        let (_dir, svc) =
            setup_with_file("src/main.rs", "fn main() {\n    println!(\"needle\");\n}\n");
        let result = svc.find("needle", &QueryOptions::default()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "source_fact");
        assert_eq!(result["results"][0]["path"], "src/main.rs");
        assert_eq!(result["results"][0]["range"]["start"]["line"], 2);
        assert!(result["snapshot_id"]
            .as_str()
            .unwrap()
            .contains("worktree:"));
    }

    #[test]
    fn find_includes_broad_guard_for_query_service_consumers() {
        let dir = tempdir().unwrap();
        for idx in 0..6 {
            fs::write(
                dir.path().join(format!("file{idx}.rs")),
                "pub fn sample() { println!(\"public\"); }\n",
            )
            .unwrap();
        }
        let svc = QueryService::new(dir.path()).unwrap();

        let result = svc.find("public", &QueryOptions::default()).unwrap();

        assert_eq!(result["guard"]["triggered"], true);
        assert_eq!(result["guard"]["reason"], "broad_literal_pattern");
        assert_eq!(result["results"].as_array().unwrap().len(), 5);
        assert!(result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "broad_query_guard_triggered"));
    }

    #[test]
    fn query_service_source_target_preserves_relative_path() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src dir")).unwrap();
        fs::write(dir.path().join("src dir/a b.rs"), "needle\n").unwrap();
        let svc = QueryService::new(dir.path()).unwrap();

        let result = svc.find("needle", &QueryOptions::default()).unwrap();
        assert_eq!(result["results"][0]["sourceTarget"], "src dir/a b.rs");
    }

    // -- grep -----------------------------------------------------------

    #[test]
    fn grep_returns_regex_matches() {
        let (_dir, svc) = setup_with_file("sample.txt", "foo\nbar\nbaz\n");
        let result = svc.grep("ba[rz]", &QueryOptions::default()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "source_fact");
        let paths: Vec<_> = result["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["matchText"].as_str().unwrap().to_string())
            .collect();
        assert!(paths.contains(&"bar".to_string()));
        assert!(paths.contains(&"baz".to_string()));
        assert_eq!(paths.len(), 2);
    }

    // -- files ----------------------------------------------------------

    #[test]
    fn files_returns_matching_paths() {
        let (_dir, svc) = setup_with_file("src/main.rs", "// empty\n");
        let result = svc.files("main", &QueryOptions::default()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "source_fact");
        assert_eq!(result["results"][0]["path"], "src/main.rs");
    }

    // -- glob -----------------------------------------------------------

    #[test]
    fn glob_strictly_matches_patterns() {
        let (_dir, svc) = setup_with_file("src/main.rs", "// empty\n");
        let result = svc.glob("**/main.rs", &QueryOptions::default()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["results"][0]["path"], "src/main.rs");
    }

    // -- read_file ------------------------------------------------------

    #[test]
    fn read_file_returns_content_with_reliability() {
        let (_dir, svc) = setup_with_file("sample.txt", "one\ntwo\nthree\n");
        let result = svc.read_file("sample.txt:2-3").unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["results"][0]["content"], "two\nthree");
        assert_eq!(result["results"][0]["exact"], true);
        assert_eq!(result["reliability"]["level"], "source_fact");
    }

    // -- list_dir -------------------------------------------------------

    #[test]
    fn list_dir_returns_directory_entries() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let svc = QueryService::new(dir.path()).unwrap();
        let result = svc.list_dir("src").unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "source_fact");
        let results = result["results"].as_array().unwrap();
        assert!(results.iter().any(|r| r["path"] == "src/main.rs"));
    }

    // -- defs (parser fallback) -----------------------------------------

    #[test]
    fn defs_falls_back_to_parser_when_no_scip_index() {
        let (_dir, svc) = setup_with_file("src/lib.rs", "fn alpha() {}\nfn beta() {}\n");
        let result = svc.defs("alpha", &QueryOptions::default()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "parser_fact");
        assert_eq!(result["reliability"]["exact"], false);
        let results = result["results"].as_array().unwrap();
        assert!(results.iter().any(|r| r["name"] == "alpha"));
    }

    // -- refs (precise SCIP only) ---------------------------------------

    #[test]
    fn refs_requires_precise_scip_index() {
        let (_dir, svc) = setup_with_file(
            "src/main.rs",
            "fn main() {\n    helper();\n}\nfn helper() {}\n",
        );
        let result = svc.refs("helper", &QueryOptions::default()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "freshness");
        assert!(result["results"].as_array().unwrap().is_empty());
        let warnings = result["warnings"].as_array().unwrap();
        assert!(warnings
            .iter()
            .any(|warning| warning["code"] == "precise_scip_index_unavailable"));
    }

    // -- symbols (parser fallback) --------------------------------------

    #[test]
    fn symbols_falls_back_to_parser() {
        let (_dir, svc) = setup_with_file("src/lib.rs", "fn alpha() {}\nstruct Beta {}\n");
        let result = svc.symbols("alpha", &QueryOptions::default()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "parser_fact");
        let results = result["results"].as_array().unwrap();
        assert!(results.iter().any(|r| r["name"] == "alpha"));
    }

    // -- calls / callers (tree-sitter fallback) -------------------------

    #[test]
    fn calls_returns_inferred_candidates() {
        let (_dir, svc) =
            setup_with_file("src/lib.rs", "fn alpha() {\n    beta();\n}\nfn beta() {}\n");
        let result = svc.calls("alpha", &QueryOptions::default()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "inferred_candidate");
        assert_eq!(result["reliability"]["exact"], false);
        let results = result["results"].as_array().unwrap();
        assert!(results.iter().any(|r| r["target"] == "beta"));
    }

    #[test]
    fn callers_returns_inferred_candidates() {
        let (_dir, svc) =
            setup_with_file("src/lib.rs", "fn alpha() {\n    beta();\n}\nfn beta() {}\n");
        let result = svc.callers("beta", &QueryOptions::default()).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "inferred_candidate");
        let results = result["results"].as_array().unwrap();
        assert!(results.iter().any(|r| r["enclosingSymbol"] == "alpha"));
    }

    // -- changed --------------------------------------------------------

    #[test]
    fn changed_returns_array_without_git() {
        let (_dir, svc) = setup_with_file("sample.txt", "hello\n");
        let result = svc.changed().unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "source_fact");
        // In a non-git dir this returns an empty array.
        assert!(result["results"].is_array());
    }

    // -- status ---------------------------------------------------------

    #[test]
    fn status_contains_snapshot_and_dirty() {
        let (_dir, svc) = setup_with_file("sample.txt", "hello\n");
        let result = svc.status().unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["reliability"]["level"], "source_fact");
        let items = result["results"].as_array().unwrap();
        let status_item = &items[0];
        assert!(status_item["snapshot_id"].as_str().is_some());
        assert!(status_item["dirty"].as_bool().is_some());
    }
}

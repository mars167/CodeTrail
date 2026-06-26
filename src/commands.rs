use std::io::{self, IsTerminal, Write};

use serde_json::{json, Value};

use crate::{
    cli::{
        Cli, Command, ExploreCommand, HooksCommand, IndexCommand, IndexProviderCommand,
        OutputFormat, QueryCommand, SkillCommand,
    },
    code_context::{self, CodeContextOptions},
    completions, config_index, graph, index,
    install::{
        IndexProviderInstallOptions, IndexProviderInstallReporter, IndexProviderInstallStep,
        SkillInstallOptions,
    },
    java_semantic::{self, CallHierarchyOptions},
    output,
    query::{ExploreNodeOptions, QueryOptions, QueryService},
    query_input::InputPlan,
    routes, saved_query, scip_index, search,
    search_pattern::SearchPatternMode,
    syntax,
    workspace::{ScanOptions, Workspace},
    AppResult,
};

pub fn run(cli: Cli) -> AppResult<i32> {
    let verbose = output::VerboseLogger::new(cli.verbose);
    verbose.log(format!("command={}", command_name(&cli.command)));
    verbose.log(format!("path={}", cli.path));

    let scan_opts = ScanOptions {
        dirs: cli.dir.clone(),
        extensions: cli.ext.clone(),
        file_patterns: cli.file_pattern.clone(),
        file_mode: cli.file_mode,
        case_sensitive: cli.case_sensitive,
        input_mode: cli.input_mode,
        include: cli.include.clone(),
        exclude: cli.exclude.clone(),
        hidden: cli.hidden,
        no_ignore: cli.no_ignore,
        lang: cli.lang.clone(),
        changed: cli.changed,
        cursor: cli.cursor.clone(),
        allow_broad: cli.allow_broad,
        limit: cli.limit,
        ..ScanOptions::default()
    };
    let mut exit_code = 0;

    if let Command::Completions { shell } = &cli.command {
        print!("{}", completions::script(shell));
        return Ok(0);
    }

    let workspace = Workspace::discover(&cli.path)?;
    verbose.log(format!(
        "workspace root={} snapshot_id={} dirty={} staged={} worktree={}",
        workspace.root.display(),
        workspace.snapshot_id,
        workspace.dirty,
        workspace.staged_count,
        workspace.worktree_count
    ));
    let scope_warnings = scope_warnings(&workspace, &scan_opts);

    let value = match &cli.command {
        Command::Find { text, mode } => {
            let query_output = search::find(
                &workspace,
                &scan_opts,
                text,
                (*mode).into(),
                cli.context,
                false,
            )?;
            exit_code = output::no_match_exit(&query_output.results);
            page_response(
                output::response_with_index(
                    "find",
                    "find",
                    scoped_query(
                        json!({ "pattern": text, "mode": mode.as_str(), "caseSensitive": scan_opts.case_sensitive, "context": cli.context }),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    output::source_fact(),
                    output::IndexedResponseParts::new(
                        query_output.index.clone(),
                        query_output.results.clone(),
                        scope_warnings.clone(),
                    ),
                ),
                query_output,
            )
        }
        Command::Grep {
            pattern,
            mode,
            context,
        } => {
            let context = context.unwrap_or(cli.context);
            let query_output = search::find(
                &workspace,
                &scan_opts,
                pattern,
                (*mode).into(),
                context,
                false,
            )?;
            exit_code = output::no_match_exit(&query_output.results);
            page_response(
                output::response_with_index(
                    "grep",
                    "find",
                    scoped_query(
                        json!({ "pattern": pattern, "mode": mode.as_str(), "caseSensitive": scan_opts.case_sensitive, "context": context }),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    output::source_fact(),
                    output::IndexedResponseParts::new(
                        query_output.index.clone(),
                        query_output.results.clone(),
                        scope_warnings.clone(),
                    ),
                ),
                query_output,
            )
        }
        Command::Files { pattern, mode } => {
            let query_output = search::files(&workspace, &scan_opts, pattern, *mode)?;
            exit_code = output::no_match_exit(&query_output.results);
            page_response(
                output::response_with_index(
                    "files",
                    "files",
                    scoped_query(
                        json!({ "pattern": pattern, "mode": path_mode_label("files", *mode), "caseSensitive": scan_opts.case_sensitive }),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    output::source_fact(),
                    output::IndexedResponseParts::new(
                        query_output.index.clone(),
                        query_output.results.clone(),
                        scope_warnings.clone(),
                    ),
                ),
                query_output,
            )
        }
        Command::FindPath { pattern, mode } => {
            let query_output = search::files(&workspace, &scan_opts, pattern, *mode)?;
            exit_code = output::no_match_exit(&query_output.results);
            page_response(
                output::response_with_index(
                    "find-path",
                    "files",
                    scoped_query(
                        json!({ "pattern": pattern, "mode": path_mode_label("find-path", *mode), "caseSensitive": scan_opts.case_sensitive }),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    output::source_fact(),
                    output::IndexedResponseParts::new(
                        query_output.index.clone(),
                        query_output.results.clone(),
                        scope_warnings.clone(),
                    ),
                ),
                query_output,
            )
        }
        Command::Glob { pattern, mode } => {
            let query_output = search::files(&workspace, &scan_opts, pattern, *mode)?;
            exit_code = output::no_match_exit(&query_output.results);
            page_response(
                output::response_with_index(
                    "glob",
                    "files",
                    scoped_query(
                        json!({ "pattern": pattern, "mode": path_mode_label("glob", *mode), "caseSensitive": scan_opts.case_sensitive }),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    output::source_fact(),
                    output::IndexedResponseParts::new(
                        query_output.index.clone(),
                        query_output.results.clone(),
                        scope_warnings.clone(),
                    ),
                ),
                query_output,
            )
        }
        Command::Refs { identifier } => {
            let precise_empty = if let Some(precise) =
                scip_index::refs(&workspace, &scan_opts, identifier)?
            {
                if has_results(&precise.results) {
                    let mut results = precise.results;
                    let mut index = precise.index;
                    if let Some(config) = config_index::refs(&workspace, &scan_opts, identifier)? {
                        append_results(&mut results, config.results);
                        truncate_results_to_limit(&mut results, scan_opts.limit);
                        append_config_index(&mut index, config.index);
                    }
                    return emit_response(
                        &cli.output,
                        output::response_with_index(
                            "refs",
                            "refs",
                            scoped_query(
                                json!({ "identifier": identifier, "producer": "scip" }),
                                &scan_opts,
                            ),
                            &workspace.snapshot_id,
                            output::precise_fact(),
                            output::IndexedResponseParts::new(
                                index,
                                results,
                                scope_warnings.clone(),
                            ),
                        ),
                        &workspace,
                        cli.save_query.as_deref(),
                    );
                }
                Some(precise)
            } else {
                None
            };
            if let Some(precise) = precise_empty {
                return emit_response(
                    &cli.output,
                    output::response_with_index(
                        "refs",
                        "refs",
                        scoped_query(
                            json!({ "identifier": identifier, "producer": "scip" }),
                            &scan_opts,
                        ),
                        &workspace.snapshot_id,
                        output::precise_fact(),
                        output::IndexedResponseParts::new(
                            precise.index,
                            precise.results,
                            scope_warnings.clone(),
                        ),
                    ),
                    &workspace,
                    cli.save_query.as_deref(),
                );
            }
            let results = json!([]);
            exit_code = output::no_match_exit(&results);
            output::response_with_index(
                "refs",
                "refs",
                scoped_query(
                    json!({ "identifier": identifier, "producer": "scip", "requires": "fresh_scip_occurrence_index" }),
                    &scan_opts,
                ),
                &workspace.snapshot_id,
                output::freshness(),
                output::IndexedResponseParts::new(
                    output::live_scan_index(),
                    results,
                    merge_warnings(
                        vec!["precise_scip_index_unavailable: refs requires a fresh SCIP occurrence index; use ripgrep for textual matches".to_string()],
                        scope_warnings.clone(),
                    ),
                ),
            )
        }
        Command::Symbols {
            query,
            include_code,
            code_context,
            code_max_lines,
        } => {
            let code_options =
                CodeContextOptions::new(*include_code, *code_context, *code_max_lines);
            let precise_empty = if let Some(precise) =
                scip_index::symbols(&workspace, &scan_opts, query)?
            {
                if has_results(&precise.results) {
                    let mut results = precise.results;
                    let mut index = precise.index;
                    if let Some(config) = config_index::symbols(&workspace, &scan_opts, query)? {
                        append_results(&mut results, config.results);
                        append_config_index(&mut index, config.index);
                    }
                    let page = search::page_results(
                        results,
                        &scan_opts,
                        "symbols",
                        code_context::query_with_code_options(
                            json!({ "query": query, "producer": "scip" }),
                            &code_options,
                        ),
                        &workspace.snapshot_id,
                    )?;
                    let response = code_context::enrich_response(
                        &workspace,
                        &scan_opts,
                        output::with_page_meta(
                            output::response_with_index(
                                "symbols",
                                "symbols",
                                scoped_query(
                                    code_context::query_with_code_options(
                                        json!({ "query": query, "producer": "scip" }),
                                        &code_options,
                                    ),
                                    &scan_opts,
                                ),
                                &workspace.snapshot_id,
                                output::precise_fact(),
                                output::IndexedResponseParts::new(
                                    index,
                                    page.results.clone(),
                                    scope_warnings.clone(),
                                ),
                            ),
                            page.truncated,
                            page.next_cursor,
                            page.facets,
                        ),
                        &code_options,
                    )?;
                    return emit_response(
                        &cli.output,
                        response,
                        &workspace,
                        cli.save_query.as_deref(),
                    );
                }
                Some(precise)
            } else {
                None
            };
            let (mut results, warnings) = syntax::symbols(&workspace, &scan_opts, query)?;
            let parser_had_results = has_results(&results);
            if let Some(config) = config_index::symbols(&workspace, &scan_opts, query)? {
                append_results(&mut results, config.results);
            }
            let fallback_had_results = has_results(&results);
            if !fallback_had_results {
                if let Some(precise) = precise_empty {
                    let response = code_context::enrich_response(
                        &workspace,
                        &scan_opts,
                        output::response_with_index(
                            "symbols",
                            "symbols",
                            scoped_query(
                                code_context::query_with_code_options(
                                    json!({ "query": query, "producer": "scip" }),
                                    &code_options,
                                ),
                                &scan_opts,
                            ),
                            &workspace.snapshot_id,
                            output::precise_fact(),
                            output::IndexedResponseParts::new(
                                precise.index,
                                precise.results,
                                scope_warnings.clone(),
                            ),
                        ),
                        &code_options,
                    )?;
                    return emit_response(
                        &cli.output,
                        response,
                        &workspace,
                        cli.save_query.as_deref(),
                    );
                }
            }
            let page = search::page_results(
                results,
                &scan_opts,
                "symbols",
                code_context::query_with_code_options(
                    json!({ "query": query, "producer": "tree_sitter_parser" }),
                    &code_options,
                ),
                &workspace.snapshot_id,
            )?;
            exit_code = output::no_match_exit(&page.results);
            code_context::enrich_response(
                &workspace,
                &scan_opts,
                output::with_page_meta(
                    output::response(
                        "symbols",
                        "symbols",
                        scoped_query(
                            code_context::query_with_code_options(
                                json!({ "query": query, "producer": "tree_sitter_parser" }),
                                &code_options,
                            ),
                            &scan_opts,
                        ),
                        &workspace.snapshot_id,
                        result_reliability(parser_had_results, &page.results),
                        page.results.clone(),
                        merge_warnings(
                            warnings,
                            merge_warnings(
                                vec![
                                    "precise_scip_index_unavailable: using tree-sitter parser fallback"
                                        .to_string(),
                                ],
                                scope_warnings.clone(),
                            ),
                        ),
                    ),
                    page.truncated,
                    page.next_cursor,
                    page.facets,
                ),
                &code_options,
            )?
        }
        Command::Defs {
            identifier,
            include_code,
            code_context,
            code_max_lines,
        } => {
            let code_options =
                CodeContextOptions::new(*include_code, *code_context, *code_max_lines);
            let precise_empty = if let Some(precise) =
                scip_index::defs(&workspace, &scan_opts, identifier)?
            {
                if has_results(&precise.results) {
                    let mut results = precise.results;
                    let mut index = precise.index;
                    if let Some(config) = config_index::defs(&workspace, &scan_opts, identifier)? {
                        append_results(&mut results, config.results);
                        truncate_results_to_limit(&mut results, scan_opts.limit);
                        append_config_index(&mut index, config.index);
                    }
                    let response = code_context::enrich_response(
                        &workspace,
                        &scan_opts,
                        output::response_with_index(
                            "defs",
                            "defs",
                            scoped_query(
                                code_context::query_with_code_options(
                                    json!({ "identifier": identifier, "producer": "scip" }),
                                    &code_options,
                                ),
                                &scan_opts,
                            ),
                            &workspace.snapshot_id,
                            output::precise_fact(),
                            output::IndexedResponseParts::new(
                                index,
                                results,
                                scope_warnings.clone(),
                            ),
                        ),
                        &code_options,
                    )?;
                    return emit_response(
                        &cli.output,
                        response,
                        &workspace,
                        cli.save_query.as_deref(),
                    );
                }
                Some(precise)
            } else {
                None
            };
            let (mut results, warnings) = syntax::defs(&workspace, &scan_opts, identifier)?;
            let parser_had_results = has_results(&results);
            if let Some(config) = config_index::defs(&workspace, &scan_opts, identifier)? {
                append_results(&mut results, config.results);
                truncate_results_to_limit(&mut results, scan_opts.limit);
            }
            if !has_results(&results) {
                if let Some(precise) = precise_empty {
                    let response = code_context::enrich_response(
                        &workspace,
                        &scan_opts,
                        output::response_with_index(
                            "defs",
                            "defs",
                            scoped_query(
                                code_context::query_with_code_options(
                                    json!({ "identifier": identifier, "producer": "scip" }),
                                    &code_options,
                                ),
                                &scan_opts,
                            ),
                            &workspace.snapshot_id,
                            output::precise_fact(),
                            output::IndexedResponseParts::new(
                                precise.index,
                                precise.results,
                                scope_warnings.clone(),
                            ),
                        ),
                        &code_options,
                    )?;
                    return emit_response(
                        &cli.output,
                        response,
                        &workspace,
                        cli.save_query.as_deref(),
                    );
                }
            }
            exit_code = output::no_match_exit(&results);
            code_context::enrich_response(
                &workspace,
                &scan_opts,
                output::response(
                    "defs",
                    "defs",
                    scoped_query(
                        code_context::query_with_code_options(
                            json!({ "identifier": identifier, "producer": "tree_sitter_parser_fallback", "fallbackReason": "precise_scip_index_unavailable" }),
                            &code_options,
                        ),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    result_reliability(parser_had_results, &results),
                    results,
                    merge_warnings(
                        warnings,
                        merge_warnings(
                            vec![
                                "precise_scip_index_unavailable: using tree-sitter parser fallback"
                                    .to_string(),
                            ],
                            scope_warnings.clone(),
                        ),
                    ),
                ),
                &code_options,
            )?
        }
        Command::Routes {
            pattern,
            mode,
            framework,
            method,
        } => {
            let query_output = routes::scan(
                &workspace,
                &scan_opts,
                pattern.as_deref(),
                (*mode).into(),
                framework,
                method,
            )?;
            exit_code = output::no_match_exit(&query_output.results);
            page_response(
                output::response_with_index(
                    "routes",
                    "routes",
                    scoped_query(
                        json!({
                            "pattern": pattern,
                            "mode": mode.as_str(),
                            "framework": framework,
                            "method": method,
                            "producer": "framework_route_scanner"
                        }),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    output::parser_fact(),
                    output::IndexedResponseParts::new(
                        query_output.index.clone(),
                        query_output.results.clone(),
                        scope_warnings.clone(),
                    ),
                ),
                query_output,
            )
        }
        Command::Calls { identifier } => {
            if let Some((index_meta, results)) =
                java_semantic::calls(&workspace, &scan_opts, identifier)?
            {
                return emit_response(
                    &cli.output,
                    output::response_with_index(
                        "calls",
                        "calls",
                        scoped_query(
                            json!({ "identifier": identifier, "producer": "java_semantic" }),
                            &scan_opts,
                        ),
                        &workspace.snapshot_id,
                        output::inferred_candidate(),
                        output::IndexedResponseParts::new(index_meta, results, Vec::new()),
                    ),
                    &workspace,
                    cli.save_query.as_deref(),
                );
            }
            // Try graph backend first (if built and fresh)
            let graph_store = graph::GraphStore::open(&workspace).ok();
            if let Some(ref store) = graph_store {
                if store.freshness_check().unwrap_or(false) {
                    let plan = InputPlan::new(identifier, scan_opts.input_mode);
                    let results = store
                        .query_calls_with_input(&plan, scan_opts.case_sensitive)
                        .and_then(|results| {
                            graph::filter_candidates_by_scan_scope(&workspace, &scan_opts, results)
                        })
                        .unwrap_or_default();
                    if !results.is_empty() {
                        let index_meta = store.index_meta(true);
                        let warnings: Vec<String> = Vec::new();
                        return emit_response(
                            &cli.output,
                            output::response_with_index(
                                "calls",
                                "calls",
                                scoped_query(
                                    json!({ "identifier": identifier, "producer": "graph" }),
                                    &scan_opts,
                                ),
                                &workspace.snapshot_id,
                                output::inferred_candidate(),
                                output::IndexedResponseParts::new(
                                    index_meta,
                                    json!(results),
                                    warnings,
                                ),
                            ),
                            &workspace,
                            cli.save_query.as_deref(),
                        );
                    }
                }
            }
            // Fall back to tree-sitter
            let (results, warnings) = syntax::calls(&workspace, &scan_opts, identifier)?;
            exit_code = output::no_match_exit(&results);
            output::response(
                "calls",
                "calls",
                scoped_query(
                    json!({ "identifier": identifier, "producer": "tree_sitter_call_heuristic" }),
                    &scan_opts,
                ),
                &workspace.snapshot_id,
                output::inferred_candidate(),
                results,
                warnings,
            )
        }
        Command::Callers { identifier } => {
            if let Some((index_meta, results)) =
                java_semantic::callers(&workspace, &scan_opts, identifier)?
            {
                return emit_response(
                    &cli.output,
                    output::response_with_index(
                        "callers",
                        "callers",
                        scoped_query(
                            json!({ "identifier": identifier, "producer": "java_semantic" }),
                            &scan_opts,
                        ),
                        &workspace.snapshot_id,
                        output::inferred_candidate(),
                        output::IndexedResponseParts::new(index_meta, results, Vec::new()),
                    ),
                    &workspace,
                    cli.save_query.as_deref(),
                );
            }
            // Try graph backend first (if built and fresh)
            let graph_store = graph::GraphStore::open(&workspace).ok();
            if let Some(ref store) = graph_store {
                if store.freshness_check().unwrap_or(false) {
                    let plan = InputPlan::new(identifier, scan_opts.input_mode);
                    let results = store
                        .query_callers_with_input(&plan, scan_opts.case_sensitive)
                        .and_then(|results| {
                            graph::filter_candidates_by_scan_scope(&workspace, &scan_opts, results)
                        })
                        .unwrap_or_default();
                    if !results.is_empty() {
                        let index_meta = store.index_meta(true);
                        let warnings: Vec<String> = Vec::new();
                        return emit_response(
                            &cli.output,
                            output::response_with_index(
                                "callers",
                                "callers",
                                scoped_query(
                                    json!({ "identifier": identifier, "producer": "graph" }),
                                    &scan_opts,
                                ),
                                &workspace.snapshot_id,
                                output::inferred_candidate(),
                                output::IndexedResponseParts::new(
                                    index_meta,
                                    json!(results),
                                    warnings,
                                ),
                            ),
                            &workspace,
                            cli.save_query.as_deref(),
                        );
                    }
                }
            }
            // Fall back to tree-sitter
            let (results, warnings) = syntax::callers(&workspace, &scan_opts, identifier)?;
            exit_code = output::no_match_exit(&results);
            output::response(
                "callers",
                "callers",
                scoped_query(
                    json!({ "identifier": identifier, "producer": "tree_sitter_call_heuristic" }),
                    &scan_opts,
                ),
                &workspace.snapshot_id,
                output::inferred_candidate(),
                results,
                warnings,
            )
        }
        Command::CallHierarchy {
            identifier,
            direction,
            depth,
            include_overrides,
        } => {
            let options = CallHierarchyOptions {
                direction: *direction,
                depth: *depth,
                include_overrides: *include_overrides,
            };
            if let Some((index_meta, results)) =
                java_semantic::query_call_hierarchy(&workspace, &scan_opts, identifier, options)?
            {
                exit_code = output::no_match_exit(&results);
                output::response_with_index(
                    "call-hierarchy",
                    "call-hierarchy",
                    scoped_query(
                        json!({
                            "identifier": identifier,
                            "producer": "java_semantic",
                            "direction": direction.as_str(),
                            "depth": depth,
                            "includeOverrides": include_overrides,
                        }),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    output::inferred_candidate(),
                    output::IndexedResponseParts::new(index_meta, results, Vec::new()),
                )
            } else if let Some((index_meta, results)) =
                graph_call_hierarchy(&workspace, &scan_opts, identifier, *direction, *depth)?
            {
                exit_code = output::no_match_exit(&results);
                output::response_with_index(
                    "call-hierarchy",
                    "call-hierarchy",
                    scoped_query(
                        json!({
                            "identifier": identifier,
                            "producer": "graph",
                            "direction": direction.as_str(),
                            "depth": depth,
                        }),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    output::inferred_candidate(),
                    output::IndexedResponseParts::new(index_meta, results, Vec::new()),
                )
            } else {
                exit_code = 1;
                output::response_with_index(
                    "call-hierarchy",
                    "call-hierarchy",
                    scoped_query(
                        json!({
                            "identifier": identifier,
                            "producer": "graph",
                            "direction": direction.as_str(),
                            "depth": depth,
                            "requires": "fresh_call_hierarchy_index"
                        }),
                        &scan_opts,
                    ),
                    &workspace.snapshot_id,
                    output::freshness(),
                    output::IndexedResponseParts::new(
                        output::live_scan_index(),
                        json!([]),
                        vec![
                            "Call hierarchy index unavailable; run `codetrail index build` to create call hierarchy data.".to_string(),
                        ],
                    ),
                )
            }
        }
        Command::Explore { command } => match command {
            ExploreCommand::Node {
                query,
                max_candidates,
                snippet_lines,
                relation_limit,
                compact,
                max_bytes,
            } => {
                let service = QueryService::from_workspace(workspace.clone());
                let opts = QueryOptions::from_scan_options(&scan_opts, cli.context);
                let response = service.explore_node(
                    query,
                    &opts,
                    ExploreNodeOptions {
                        max_candidates: *max_candidates,
                        snippet_lines: *snippet_lines,
                        relation_limit: *relation_limit,
                        compact: *compact,
                        max_bytes: *max_bytes,
                    },
                )?;
                exit_code = output::no_match_exit(&response["results"]);
                response
            }
        },
        Command::Changed => output::with_summary_field(
            output::response(
                "changed",
                "changed",
                json!({}),
                &workspace.snapshot_id,
                output::source_fact(),
                search::changed(&workspace)?,
                Vec::new(),
            ),
            "changed",
            search::changed_summary(&workspace),
        ),
        Command::Status => output::response(
            "status",
            "status",
            json!({}),
            &workspace.snapshot_id,
            output::source_fact(),
            json!([search::status(&workspace)]),
            Vec::new(),
        ),
        Command::Mcp => {
            let server = crate::mcp::Server::new(&workspace.root)?;
            server.run()?;
            return Ok(0);
        }
        Command::Watch { once, status } => {
            let mut watcher = crate::watcher::Watcher::start(&workspace.root)?;

            let results = if *once {
                // Run one reconcile pass, detect file changes against snapshot
                let reconcile_result = watcher.run_once()?;
                json!([serde_json::to_value(&reconcile_result)?])
            } else if *status {
                // Show watcher state (initialized but not running long-lived daemon)
                json!([watcher.status()])
            } else {
                // Default: show status with note about daemon mode
                json!([watcher.status()])
            };
            output::response(
                "watch",
                "watch",
                json!({ "once": once, "status": status }),
                &workspace.snapshot_id,
                output::freshness(),
                results,
                if !once && !status {
                    vec!["long-running watcher daemon mode is intentionally not started in non-interactive command execution; use watch --once for reconcile or watch --status for state".to_string()]
                } else {
                    Vec::new()
                },
            )
        }
        Command::Serve { no_watch } => {
            // Show query service status with optional watcher info
            let mut service_value = index::serve_status(&workspace, *no_watch);
            if !no_watch {
                // When watch is enabled, include watcher status
                if let Ok(watcher) = crate::watcher::Watcher::start(&workspace.root) {
                    if let Some(service) = service_value.get_mut("service") {
                        service["watcher"] = watcher.status();
                    }
                }
            }
            output::response(
                "serve",
                "serve",
                json!({ "noWatch": no_watch }),
                &workspace.snapshot_id,
                output::freshness(),
                json!([service_value]),
                Vec::new(),
            )
        }
        Command::Query { command } => match command {
            QueryCommand::Replay { name, snapshot } => {
                let value = saved_query::replay(&workspace, name, snapshot)?;
                exit_code = output::no_match_exit(&value["results"]);
                value
            }
            QueryCommand::Show { name } => output::response(
                "query show",
                "query",
                json!({ "name": name }),
                &workspace.snapshot_id,
                output::source_fact(),
                json!([saved_query::show(&workspace, name)?]),
                Vec::new(),
            ),
            QueryCommand::List => output::response(
                "query list",
                "query",
                json!({}),
                &workspace.snapshot_id,
                output::source_fact(),
                saved_query::list(&workspace)?,
                Vec::new(),
            ),
            QueryCommand::Delete { name } => output::response(
                "query delete",
                "query",
                json!({ "name": name }),
                &workspace.snapshot_id,
                output::source_fact(),
                saved_query::delete(&workspace, name)?,
                Vec::new(),
            ),
        },
        Command::Index { command } => match command {
            IndexCommand::Build {
                staged,
                changed,
                force,
                no_semantic,
            } => {
                let semantic_enabled = !*no_semantic;
                let started = std::time::Instant::now();
                let result = with_progress(&cli.output, "Building index", "", || {
                    index::build(
                        &workspace,
                        &scan_opts,
                        *staged,
                        *changed,
                        *force,
                        semantic_enabled,
                        verbose,
                    )
                })?;
                if cli.output == OutputFormat::Text && std::io::stderr().is_terminal() {
                    let stages = result
                        .get("index")
                        .and_then(|index| index.get("stages"))
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let finish_message = output::stage_summary_line(
                        "index build",
                        &[
                            (
                                "scan",
                                stages
                                    .get("scan")
                                    .and_then(Value::as_u64)
                                    .map(|value| value as usize),
                            ),
                            (
                                "proof",
                                stages
                                    .get("proof")
                                    .and_then(Value::as_u64)
                                    .map(|value| value as usize),
                            ),
                            (
                                "skipped",
                                stages
                                    .get("skipped")
                                    .and_then(Value::as_u64)
                                    .map(|value| value as usize),
                            ),
                            ("semantic", None),
                            ("graph", None),
                        ],
                        started.elapsed(),
                    );
                    eprintln!("{finish_message}");
                    for line in provider_install_lines_from_result(&result) {
                        eprintln!("{line}");
                    }
                }
                output::response(
                    "index build",
                    "index build",
                    json!({ "staged": staged, "changed": changed, "force": force, "noSemantic": no_semantic }),
                    &workspace.snapshot_id,
                    output::freshness(),
                    json!([result]),
                    Vec::new(),
                )
            }
            IndexCommand::Update => output::response(
                "index update",
                "index update",
                json!({}),
                &workspace.snapshot_id,
                output::freshness(),
                json!([with_progress(
                    &cli.output,
                    "Updating index",
                    "Index update complete",
                    || index::update(&workspace, &scan_opts, verbose)
                )?]),
                Vec::new(),
            ),
            IndexCommand::Status { summary } => output::response(
                "index status",
                "index status",
                json!({ "summary": summary }),
                &workspace.snapshot_id,
                output::freshness(),
                json!([if *summary {
                    index::status_summary(&workspace)?
                } else {
                    index::status(&workspace)?
                }]),
                Vec::new(),
            ),
            IndexCommand::Doctor => output::response(
                "index doctor",
                "index doctor",
                json!({}),
                &workspace.snapshot_id,
                output::freshness(),
                json!([index::doctor(&workspace)?]),
                Vec::new(),
            ),
            IndexCommand::Skipped { staged } => output::response(
                "index skipped",
                "index skipped",
                json!({ "staged": staged }),
                &workspace.snapshot_id,
                output::freshness(),
                json!([index::skipped(&workspace, *staged)?]),
                Vec::new(),
            ),
            IndexCommand::Verify => {
                let (result, code) = index::verify(&workspace)?;
                exit_code = code;
                output::response(
                    "index verify",
                    "index verify",
                    json!({}),
                    &workspace.snapshot_id,
                    output::freshness(),
                    json!([result]),
                    Vec::new(),
                )
            }
            IndexCommand::Clean => output::response(
                "index clean",
                "index clean",
                json!({}),
                &workspace.snapshot_id,
                output::freshness(),
                json!([index::clean(&workspace)?]),
                Vec::new(),
            ),
            IndexCommand::Pack { output } => {
                let value =
                    with_progress(&cli.output, "Packing index", "Index pack complete", || {
                        index::pack(&workspace, output)
                    })?;
                output::response(
                    "index pack",
                    "index pack",
                    json!({ "output": output }),
                    &workspace.snapshot_id,
                    output::freshness(),
                    value,
                    Vec::new(),
                )
            }
            IndexCommand::Unpack { path } => {
                let value = with_progress(
                    &cli.output,
                    "Unpacking index",
                    "Index unpack complete",
                    || index::unpack(&workspace, path),
                )?;
                output::response(
                    "index unpack",
                    "index unpack",
                    json!({ "path": path }),
                    &workspace.snapshot_id,
                    output::freshness(),
                    value,
                    Vec::new(),
                )
            }
        },
        Command::IndexProvider { command } => match command {
            IndexProviderCommand::Install {
                languages,
                dry_run,
                force,
            } => {
                let effective_languages = if languages.is_empty() {
                    cli.lang.clone()
                } else {
                    languages.clone()
                };
                let mut install_progress = TtyIndexProviderInstallProgress::new(&cli.output);
                let install_result = crate::install::install_index_providers_with_reporter(
                    &workspace,
                    &IndexProviderInstallOptions {
                        languages: effective_languages.clone(),
                        dry_run: *dry_run,
                        force: *force,
                    },
                    &mut install_progress,
                );
                if let Ok((_, code)) = &install_result {
                    install_progress.finish(*code == 0);
                } else {
                    install_progress.finish(false);
                }
                let (results, code) = install_result?;
                exit_code = code;
                output::response(
                    "index-provider install",
                    "index-provider install",
                    json!({ "languages": effective_languages, "dryRun": dry_run, "force": force }),
                    &workspace.snapshot_id,
                    output::freshness(),
                    results,
                    Vec::new(),
                )
            }
        },
        Command::Skill { command } => match command {
            SkillCommand::Install {
                target,
                scope,
                path,
                dry_run,
                force,
            } => {
                let target = resolve_skill_install_target(target.as_deref(), &cli.output)?;
                output::response(
                    "skill install",
                    "skill install",
                    json!({ "target": &target, "scope": format!("{scope:?}").to_lowercase(), "path": path, "dryRun": dry_run, "force": force }),
                    &workspace.snapshot_id,
                    output::freshness(),
                    crate::install::install_skill(
                        &workspace,
                        &SkillInstallOptions {
                            target,
                            scope: *scope,
                            path: path.clone(),
                            dry_run: *dry_run,
                            force: *force,
                        },
                    )?,
                    Vec::new(),
                )
            }
        },
        Command::Hooks { command } => match command {
            HooksCommand::Install => output::response(
                "hooks install",
                "hooks install",
                json!({}),
                &workspace.snapshot_id,
                output::freshness(),
                index::hooks_install(&workspace)?,
                Vec::new(),
            ),
            HooksCommand::Uninstall => output::response(
                "hooks uninstall",
                "hooks uninstall",
                json!({}),
                &workspace.snapshot_id,
                output::freshness(),
                index::hooks_uninstall(&workspace)?,
                Vec::new(),
            ),
            HooksCommand::Status => output::response(
                "hooks status",
                "hooks status",
                json!({}),
                &workspace.snapshot_id,
                output::freshness(),
                index::hooks_status(&workspace)?,
                Vec::new(),
            ),
        },
        Command::Completions { .. } => unreachable!("handled before workspace discovery"),
    };

    let mut value = output::with_workspace_root(value, &workspace.root);
    attach_saved_query(&mut value, &workspace, cli.save_query.as_deref())?;
    output::emit(&cli.output, &value)?;
    Ok(exit_code)
}

fn graph_call_hierarchy(
    workspace: &Workspace,
    scan_opts: &ScanOptions,
    identifier: &str,
    direction: java_semantic::CallHierarchyDirection,
    depth: usize,
) -> AppResult<Option<(Value, Value)>> {
    let store = graph::GraphStore::open(workspace)?;
    if !store.freshness_check().unwrap_or(false) {
        return Ok(None);
    }
    let hierarchy_direction = match direction {
        java_semantic::CallHierarchyDirection::Incoming => {
            graph::schema::HierarchyDirection::Incoming
        }
        java_semantic::CallHierarchyDirection::Outgoing => {
            graph::schema::HierarchyDirection::Outgoing
        }
        java_semantic::CallHierarchyDirection::Both => graph::schema::HierarchyDirection::Both,
    };
    let results =
        store.query_call_hierarchy(workspace, scan_opts, identifier, hierarchy_direction, depth)?;
    Ok(Some((store.index_meta(true), Value::Array(results))))
}

fn emit_response(
    format: &crate::cli::OutputFormat,
    value: serde_json::Value,
    workspace: &Workspace,
    save_query: Option<&str>,
) -> AppResult<i32> {
    let mut value = output::with_workspace_root(value, &workspace.root);
    attach_saved_query(&mut value, workspace, save_query)?;
    let exit_code = output::no_match_exit(&value["results"]);
    output::emit(format, &value)?;
    Ok(exit_code)
}

fn resolve_skill_install_target(target: Option<&str>, format: &OutputFormat) -> AppResult<String> {
    if let Some(target) = target {
        return Ok(target.to_string());
    }
    if *format == OutputFormat::Text && io::stdin().is_terminal() && io::stderr().is_terminal() {
        return prompt_skill_install_target();
    }
    Err(anyhow::anyhow!(
        "skill target is required in non-interactive mode; pass one of: {}",
        skill_target_id_list()
    ))
}

fn prompt_skill_install_target() -> AppResult<String> {
    let options = crate::install::skill_target_options();
    eprintln!("Select target agent for CodeTrail skill install:");
    for (index, option) in options.iter().enumerate() {
        eprintln!("  {}. {} ({})", index + 1, option.label, option.id);
    }
    eprint!("Target agent: ");
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    parse_skill_target_selection(&input).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid skill target selection; pass one of: {}",
            skill_target_id_list()
        )
    })
}

fn parse_skill_target_selection(input: &str) -> Option<String> {
    let selection = input.trim();
    if selection.is_empty() {
        return None;
    }
    let options = crate::install::skill_target_options();
    if let Ok(index) = selection.parse::<usize>() {
        if (1..=options.len()).contains(&index) {
            return Some(options[index - 1].id.to_string());
        }
        return None;
    }
    options
        .iter()
        .find(|option| {
            option.id.eq_ignore_ascii_case(selection)
                || option.label.eq_ignore_ascii_case(selection)
        })
        .map(|option| option.id.to_string())
}

fn skill_target_id_list() -> String {
    crate::install::skill_target_options()
        .iter()
        .map(|option| option.id)
        .collect::<Vec<_>>()
        .join(", ")
}

struct TtyIndexProviderInstallProgress<'a> {
    format: &'a OutputFormat,
    active: Option<output::ProgressIndicator>,
    enabled: bool,
    saw_work: bool,
}

impl<'a> TtyIndexProviderInstallProgress<'a> {
    fn new(format: &'a OutputFormat) -> Self {
        Self {
            format,
            active: None,
            enabled: *format == OutputFormat::Text && io::stderr().is_terminal(),
            saw_work: false,
        }
    }

    fn finish(&mut self, success: bool) {
        self.finish_active_command();
        if self.enabled && self.saw_work && success {
            let _ = writeln!(io::stderr(), "Index provider install complete");
        }
    }

    fn finish_active_command(&mut self) {
        if let Some(progress) = self.active.take() {
            progress.finish("");
        }
    }
}

impl IndexProviderInstallReporter for TtyIndexProviderInstallProgress<'_> {
    fn command_started(&mut self, step: IndexProviderInstallStep<'_>) {
        self.finish_active_command();
        self.saw_work = true;
        let message = format!(
            "Installing {} provider ({}) with {}",
            step.language,
            step.provider,
            short_progress_command(step.command)
        );
        self.active = Some(output::ProgressIndicator::start(self.format, message));
    }

    fn command_finished(&mut self) {
        self.finish_active_command();
    }
}

fn short_progress_command(command: &str) -> String {
    const MAX_CHARS: usize = 72;
    let mut chars = command.chars();
    let shortened = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{shortened}...")
    } else {
        shortened
    }
}

fn with_progress<T, F>(
    format: &OutputFormat,
    start_message: &str,
    finish_message: &str,
    work: F,
) -> AppResult<T>
where
    F: FnOnce() -> AppResult<T>,
{
    let progress = output::ProgressIndicator::start(format, start_message);
    let result = work();
    progress.finish(if result.is_ok() { finish_message } else { "" });
    result
}

fn provider_install_lines_from_result(result: &Value) -> Vec<String> {
    let Some(help) = result
        .pointer("/index/semantic/providerInstallHelp")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    help.iter()
        .map(|item| {
            let language = item
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let provider = item
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let env_key = item
                .get("envKey")
                .and_then(Value::as_str)
                .unwrap_or("CODETRAIL_SCIP_*");
            let install = item
                .pointer(provider_install_pointer())
                .and_then(Value::as_str)
                .unwrap_or(provider);
            format!(
                "codetrail: semantic provider missing for {language} ({provider}). Install: {install}. Fallback: tree-sitter parser. Override with {env_key}."
            )
        })
        .collect()
}

fn provider_install_pointer() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "/install/macos/0"
    }
    #[cfg(target_os = "linux")]
    {
        "/install/linux/0"
    }
    #[cfg(target_os = "windows")]
    {
        "/install/windows/0"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "/install/macos/0"
    }
}

fn attach_saved_query(
    value: &mut Value,
    workspace: &Workspace,
    save_query: Option<&str>,
) -> AppResult<()> {
    if let Some(name) = save_query {
        value["savedQuery"] = saved_query::save_from_response(workspace, name, value)?;
    }
    Ok(())
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

fn result_reliability(parser_had_results: bool, results: &Value) -> output::Reliability {
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

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Find { .. } => "find",
        Command::Grep { .. } => "grep",
        Command::Files { .. } => "files",
        Command::FindPath { .. } => "find-path",
        Command::Glob { .. } => "glob",
        Command::Refs { .. } => "refs",
        Command::Symbols { .. } => "symbols",
        Command::Defs { .. } => "defs",
        Command::Routes { .. } => "routes",
        Command::Calls { .. } => "calls",
        Command::Callers { .. } => "callers",
        Command::CallHierarchy { .. } => "call-hierarchy",
        Command::Explore { .. } => "explore",
        Command::Changed => "changed",
        Command::Status => "status",
        Command::Mcp => "mcp",
        Command::Watch { .. } => "watch",
        Command::Serve { .. } => "serve",
        Command::Query { .. } => "query",
        Command::Index { command } => match command {
            IndexCommand::Build { .. } => "index build",
            IndexCommand::Update => "index update",
            IndexCommand::Status { .. } => "index status",
            IndexCommand::Doctor => "index doctor",
            IndexCommand::Skipped { .. } => "index skipped",
            IndexCommand::Verify => "index verify",
            IndexCommand::Clean => "index clean",
            IndexCommand::Pack { .. } => "index pack",
            IndexCommand::Unpack { .. } => "index unpack",
        },
        Command::IndexProvider { command } => match command {
            IndexProviderCommand::Install { .. } => "index-provider install",
        },
        Command::Skill { command } => match command {
            SkillCommand::Install { .. } => "skill install",
        },
        Command::Hooks { .. } => "hooks",
        Command::Completions { .. } => "completions",
    }
}

fn scoped_query(mut query: Value, opts: &ScanOptions) -> Value {
    if let Some(object) = query.as_object_mut() {
        object.insert("scope".to_string(), search::scope_value(opts));
    }
    query
}

fn scope_warnings(workspace: &Workspace, opts: &ScanOptions) -> Vec<String> {
    if opts.changed && workspace.changed.is_empty() {
        vec!["changed scope is empty; no full-workspace fallback was used".to_string()]
    } else {
        Vec::new()
    }
}

fn merge_warnings(mut first: Vec<String>, second: Vec<String>) -> Vec<String> {
    first.extend(second);
    first
}

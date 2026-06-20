use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::{
    graph,
    query_input::{InputPlan, SymbolMatchMode},
    search, syntax,
    workspace::{ScanOptions, Workspace},
};

pub const DEFAULT_CODE_CONTEXT_LINES: u16 = 20;
pub const DEFAULT_CODE_MAX_LINES: usize = 200;
pub const MAX_CODE_MAX_LINES: usize = 2000;
const RELATION_LIMIT: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeContextOptions {
    pub include_code: bool,
    pub code_context: u16,
    pub code_max_lines: usize,
}

impl Default for CodeContextOptions {
    fn default() -> Self {
        Self {
            include_code: false,
            code_context: DEFAULT_CODE_CONTEXT_LINES,
            code_max_lines: DEFAULT_CODE_MAX_LINES,
        }
    }
}

impl CodeContextOptions {
    pub fn new(
        include_code: bool,
        code_context: Option<u16>,
        code_max_lines: Option<usize>,
    ) -> Self {
        Self {
            include_code,
            code_context: code_context.unwrap_or(DEFAULT_CODE_CONTEXT_LINES),
            code_max_lines: code_max_lines.unwrap_or(DEFAULT_CODE_MAX_LINES),
        }
    }
}

pub fn query_with_code_options(mut query: Value, options: &CodeContextOptions) -> Value {
    if options.include_code {
        if let Some(object) = query.as_object_mut() {
            object.insert("includeCode".to_string(), Value::Bool(true));
            object.insert("codeContext".to_string(), json!(options.code_context));
            object.insert("codeMaxLines".to_string(), json!(options.code_max_lines));
        }
    }
    query
}

pub fn options_from_query(query: &Value) -> CodeContextOptions {
    CodeContextOptions {
        include_code: query
            .get("includeCode")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        code_context: query
            .get("codeContext")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(DEFAULT_CODE_CONTEXT_LINES),
        code_max_lines: query
            .get("codeMaxLines")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .map(|value| value.clamp(1, MAX_CODE_MAX_LINES))
            .unwrap_or(DEFAULT_CODE_MAX_LINES),
    }
}

pub fn enrich_response(
    workspace: &Workspace,
    scan: &ScanOptions,
    mut response: Value,
    options: &CodeContextOptions,
) -> Result<Value> {
    if !options.include_code {
        return Ok(response);
    }

    let mut relation_context = RelationContext::default();
    let mut warnings = Vec::<Warning>::new();
    if let Some(results) = response.get_mut("results").and_then(Value::as_array_mut) {
        for result in results {
            enrich_result(
                workspace,
                scan,
                result,
                options,
                &mut relation_context,
                &mut warnings,
            )?;
        }
    }
    append_warnings(&mut response, warnings);
    Ok(response)
}

fn enrich_result(
    workspace: &Workspace,
    scan: &ScanOptions,
    result: &mut Value,
    options: &CodeContextOptions,
    relation_context: &mut RelationContext,
    warnings: &mut Vec<Warning>,
) -> Result<()> {
    let Some(object) = result.as_object_mut() else {
        return Ok(());
    };
    let Some(path) = object
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(());
    };
    let symbol_name = object
        .get("symbolName")
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some((range, range_kind)) = source_range_for_result(workspace, scan, object, options)
    else {
        return Ok(());
    };

    if range.fallback_used {
        push_warning(
            warnings,
            "source_context_fallback",
            "source_context_fallback: symbol body range unavailable; used occurrence range plus code context",
        );
    }
    if range.truncated {
        push_warning(
            warnings,
            "source_truncated",
            "source_truncated: source context was truncated by codeMaxLines",
        );
    }

    match read_source(
        workspace,
        &path,
        range.start_line,
        range.end_line,
        range_kind,
        range.truncated,
    ) {
        Ok(source) => {
            let relations = relations_for_result(
                workspace,
                scan,
                &path,
                range.start_line,
                range.end_line,
                symbol_name.as_deref(),
                relation_context,
                warnings,
            )?;
            object.insert("source".to_string(), source);
            object.insert("relations".to_string(), relations);
        }
        Err(error) => push_warning(
            warnings,
            "source_read_failed",
            format!("source_read_failed: {error}"),
        ),
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SelectedRange {
    start_line: usize,
    end_line: usize,
    truncated: bool,
    fallback_used: bool,
}

fn source_range_for_result(
    workspace: &Workspace,
    scan: &ScanOptions,
    object: &Map<String, Value>,
    options: &CodeContextOptions,
) -> Option<(SelectedRange, &'static str)> {
    if let Some((start, end)) = object.get("bodyRange").and_then(range_lines) {
        return Some((cap_range(start, end, options.code_max_lines, false), "body"));
    }

    if let Some((start, end)) = parser_body_range_for_result(workspace, scan, object) {
        return Some((cap_range(start, end, options.code_max_lines, false), "body"));
    }

    let (start, end) = object.get("range").and_then(range_lines)?;
    let context = usize::from(options.code_context);
    let start = start.saturating_sub(context).max(1);
    let end = end.saturating_add(context);
    Some((
        cap_range(start, end, options.code_max_lines, true),
        "context",
    ))
}

fn cap_range(
    start_line: usize,
    end_line: usize,
    code_max_lines: usize,
    fallback_used: bool,
) -> SelectedRange {
    let max_lines = code_max_lines.clamp(1, MAX_CODE_MAX_LINES);
    let requested_lines = end_line.saturating_sub(start_line).saturating_add(1);
    let truncated = requested_lines > max_lines;
    let end_line = if truncated {
        start_line.saturating_add(max_lines).saturating_sub(1)
    } else {
        end_line
    };
    SelectedRange {
        start_line,
        end_line,
        truncated,
        fallback_used,
    }
}

fn parser_body_range_for_result(
    workspace: &Workspace,
    scan: &ScanOptions,
    object: &Map<String, Value>,
) -> Option<(usize, usize)> {
    let path = object.get("path").and_then(Value::as_str)?;
    let symbol_name = object
        .get("symbolName")
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)?;
    let occurrence = object.get("range").and_then(range_lines);
    let mut scan = scan.clone();
    scan.limit = 0;
    let (defs, _) = syntax::defs(workspace, &scan, symbol_name).ok()?;
    let candidates = defs.as_array()?;
    let mut fallback = None;
    for candidate in candidates {
        if candidate.get("path").and_then(Value::as_str) != Some(path) {
            continue;
        }
        if candidate
            .get("symbolName")
            .or_else(|| candidate.get("name"))
            .and_then(Value::as_str)
            != Some(symbol_name)
        {
            continue;
        }
        let Some(body_range) = candidate.get("bodyRange").and_then(range_lines) else {
            continue;
        };
        if fallback.is_none() {
            fallback = Some(body_range);
        }
        if let (Some((occurrence_start, occurrence_end)), Some((candidate_start, candidate_end))) =
            (occurrence, candidate.get("range").and_then(range_lines))
        {
            if candidate_start <= occurrence_start && occurrence_end <= candidate_end {
                return Some(body_range);
            }
        }
    }
    fallback
}

fn read_source(
    workspace: &Workspace,
    path: &str,
    start_line: usize,
    end_line: usize,
    range_kind: &'static str,
    truncated: bool,
) -> Result<Value> {
    let target = format!("{path}:{start_line}-{end_line}");
    let read = search::read(workspace, &target)?;
    let actual_start = read
        .pointer("/range/start/line")
        .and_then(Value::as_u64)
        .unwrap_or(start_line as u64);
    let actual_end = read
        .pointer("/range/end/line")
        .and_then(Value::as_u64)
        .unwrap_or(end_line as u64);
    Ok(json!({
        "path": path,
        "range": read.get("range").cloned().unwrap_or_else(|| json!({
            "start": { "line": start_line, "column": 1 },
            "end": { "line": end_line, "column": 1 }
        })),
        "rangeKind": range_kind,
        "startLine": actual_start,
        "endLine": actual_end,
        "content": read.get("content").and_then(Value::as_str).unwrap_or("").to_string(),
        "truncated": truncated,
        "truncatedReason": if truncated { Value::String("code_max_lines".to_string()) } else { Value::Null }
    }))
}

fn relations_for_result(
    workspace: &Workspace,
    scan: &ScanOptions,
    path: &str,
    start_line: usize,
    end_line: usize,
    symbol_name: Option<&str>,
    relation_context: &mut RelationContext,
    warnings: &mut Vec<Warning>,
) -> Result<Value> {
    let Some(symbol_name) = symbol_name else {
        return Ok(json!({ "calls": [], "callers": [], "truncated": false }));
    };

    let mut calls = relation_calls(
        workspace,
        scan,
        relation_context,
        symbol_name,
        path,
        start_line,
        end_line,
    )?;
    let mut callers = relation_callers(workspace, scan, relation_context, symbol_name)?;
    if !callers.is_empty() {
        push_warning(
            warnings,
            "ambiguous_relations",
            "ambiguous_relations: callers are matched by symbol name and may include same-name definitions",
        );
    }
    let truncated = calls.len() > RELATION_LIMIT || callers.len() > RELATION_LIMIT;
    calls.truncate(RELATION_LIMIT);
    callers.truncate(RELATION_LIMIT);
    if truncated {
        push_warning(
            warnings,
            "relations_truncated",
            "relations_truncated: source relations were capped",
        );
    }
    if !calls.is_empty() || !callers.is_empty() {
        push_warning(
            warnings,
            "inferred_candidate",
            "inferred_candidate: source relations are candidate call graph evidence",
        );
    }
    Ok(json!({ "calls": calls, "callers": callers, "truncated": truncated }))
}

fn relation_calls(
    workspace: &Workspace,
    scan: &ScanOptions,
    relation_context: &mut RelationContext,
    symbol_name: &str,
    path: &str,
    start_line: usize,
    end_line: usize,
) -> Result<Vec<Value>> {
    if let Some(results) =
        graph_relation_candidates(workspace, scan, relation_context, symbol_name, true)?
    {
        let filtered = results
            .into_iter()
            .filter(|candidate| candidate_path(candidate) == Some(path))
            .filter(|candidate| {
                candidate_start_line(candidate)
                    .is_some_and(|line| start_line as u64 <= line && line <= end_line as u64)
            })
            .map(public_relation_candidate)
            .collect::<Vec<_>>();
        if !filtered.is_empty() {
            return Ok(filtered);
        }
    }

    Ok(relation_context
        .parser_calls(workspace, scan)?
        .iter()
        .filter(|candidate| candidate.path == path)
        .filter(|candidate| {
            candidate
                .range
                .pointer("/start/line")
                .and_then(Value::as_u64)
                .is_some_and(|line| start_line as u64 <= line && line <= end_line as u64)
        })
        .map(parser_call_candidate)
        .collect())
}

fn relation_callers(
    workspace: &Workspace,
    scan: &ScanOptions,
    relation_context: &mut RelationContext,
    symbol_name: &str,
) -> Result<Vec<Value>> {
    if let Some(results) =
        graph_relation_candidates(workspace, scan, relation_context, symbol_name, false)?
    {
        if !results.is_empty() {
            return Ok(results.into_iter().map(public_relation_candidate).collect());
        }
    }

    let plan = InputPlan::new(symbol_name, scan.input_mode);
    Ok(relation_context
        .parser_calls(workspace, scan)?
        .iter()
        .filter(|candidate| {
            plan.matched_variant(
                syntax::last_identifier(&candidate.target),
                scan.case_sensitive,
                SymbolMatchMode::Exact,
            )
            .is_some()
        })
        .map(parser_call_candidate)
        .collect())
}

fn graph_relation_candidates(
    workspace: &Workspace,
    scan: &ScanOptions,
    relation_context: &mut RelationContext,
    symbol_name: &str,
    outgoing: bool,
) -> Result<Option<Vec<Value>>> {
    let Some(store) = relation_context.graph_store(workspace) else {
        return Ok(None);
    };
    let plan = InputPlan::new(symbol_name, scan.input_mode);
    let mut relation_scan = scan.clone();
    relation_scan.limit = 0;
    let results = if outgoing {
        store.query_calls_with_input(&plan, scan.case_sensitive)?
    } else {
        store.query_callers_with_input(&plan, scan.case_sensitive)?
    };
    let results = graph::filter_candidates_by_scan_scope(workspace, &relation_scan, results)?;
    let value = serde_json::to_value(results)?;
    let Value::Array(items) = value else {
        return Ok(Some(Vec::new()));
    };
    Ok(Some(items))
}

#[derive(Default)]
struct RelationContext {
    graph_checked: bool,
    graph_store: Option<graph::GraphStore>,
    parser_calls: Option<Vec<syntax::CallCandidate>>,
}

impl RelationContext {
    fn graph_store(&mut self, workspace: &Workspace) -> Option<&graph::GraphStore> {
        if !self.graph_checked {
            self.graph_checked = true;
            self.graph_store = graph::GraphStore::open(workspace)
                .ok()
                .filter(|store| store.freshness_check().unwrap_or(false));
        }
        self.graph_store.as_ref()
    }

    fn parser_calls(
        &mut self,
        workspace: &Workspace,
        scan: &ScanOptions,
    ) -> Result<&[syntax::CallCandidate]> {
        if self.parser_calls.is_none() {
            let mut warnings = Vec::new();
            let mut relation_scan = scan.clone();
            relation_scan.limit = 0;
            self.parser_calls = Some(syntax::collect_calls(
                workspace,
                &relation_scan,
                &mut warnings,
            )?);
        }
        Ok(self.parser_calls.as_deref().unwrap_or(&[]))
    }
}

fn parser_call_candidate(candidate: &syntax::CallCandidate) -> Value {
    json!({
        "path": candidate.path,
        "target": candidate.target,
        "kind": "call",
        "enclosingSymbol": candidate.enclosing_symbol,
        "language": candidate.language,
        "rootId": candidate.root_id,
        "range": candidate.range,
        "layer": candidate.layer
    })
}

fn public_relation_candidate(candidate: Value) -> Value {
    let mut object = Map::new();
    copy_field(&candidate, &mut object, "path");
    copy_field(&candidate, &mut object, "target");
    copy_field(&candidate, &mut object, "kind");
    copy_field(&candidate, &mut object, "enclosingSymbol");
    copy_field(&candidate, &mut object, "language");
    copy_field(&candidate, &mut object, "rootId");
    copy_field(&candidate, &mut object, "range");
    copy_field(&candidate, &mut object, "layer");
    copy_field(&candidate, &mut object, "matchedInputVariant");
    if !object.contains_key("kind") {
        object.insert("kind".to_string(), Value::String("call".to_string()));
    }
    if !object.contains_key("layer") {
        object.insert(
            "layer".to_string(),
            Value::String("inferred_candidate".to_string()),
        );
    }
    Value::Object(object)
}

fn copy_field(source: &Value, target: &mut Map<String, Value>, field: &str) {
    if let Some(value) = source.get(field).filter(|value| !value.is_null()) {
        target.insert(field.to_string(), value.clone());
    }
}

fn candidate_path(candidate: &Value) -> Option<&str> {
    candidate.get("path").and_then(Value::as_str)
}

fn candidate_start_line(candidate: &Value) -> Option<u64> {
    candidate
        .pointer("/range/start/line")
        .and_then(Value::as_u64)
}

fn range_lines(range: &Value) -> Option<(usize, usize)> {
    let start = range.pointer("/start/line")?.as_u64()? as usize;
    let end = range.pointer("/end/line")?.as_u64()? as usize;
    (start > 0 && start <= end).then_some((start, end))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Warning {
    code: String,
    message: String,
}

fn push_warning(warnings: &mut Vec<Warning>, code: impl Into<String>, message: impl Into<String>) {
    let warning = Warning {
        code: code.into(),
        message: message.into(),
    };
    if !warnings
        .iter()
        .any(|existing| existing.code == warning.code)
    {
        warnings.push(warning);
    }
}

fn append_warnings(response: &mut Value, warnings: Vec<Warning>) {
    if warnings.is_empty() {
        return;
    }
    let existing = response.get_mut("warnings").and_then(Value::as_array_mut);
    if let Some(existing) = existing {
        let mut seen = existing
            .iter()
            .filter_map(|warning| warning.get("code").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();
        for warning in warnings {
            if seen.insert(warning.code.clone()) {
                existing.push(json!({ "code": warning.code, "message": warning.message }));
            }
        }
    } else {
        response["warnings"] = Value::Array(
            warnings
                .into_iter()
                .map(|warning| json!({ "code": warning.code, "message": warning.message }))
                .collect(),
        );
    }
}

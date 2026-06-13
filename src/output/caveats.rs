use serde_json::{json, Value};

pub(super) fn public_caveats(value: &Value) -> Vec<Value> {
    let mut caveats = Vec::new();
    let mut seen = std::collections::BTreeSet::<String>::new();

    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        let code = value
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        push_public_caveat_with(&mut caveats, &mut seen, code, message, "error", "error");
    }

    let guard_triggered = value.pointer("/guard/triggered").and_then(Value::as_bool) == Some(true);

    for warning in value
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let code = warning
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("warning");
        let message = warning
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(code);
        if guard_triggered && code == "broad_query_guard_triggered" {
            continue;
        }
        push_public_caveat(&mut caveats, &mut seen, code, message);
    }

    if let Some(level) = value.pointer("/reliability/level").and_then(Value::as_str) {
        match level {
            "parser_fact" => {
                if !seen.contains("precise_scip_index_unavailable") {
                    push_public_caveat(
                        &mut caveats,
                        &mut seen,
                        "parser_fact",
                        "parser fallback result; not semantic reference resolution",
                    );
                }
            }
            "inferred_candidate" => push_public_caveat(
                &mut caveats,
                &mut seen,
                "inferred_candidate",
                "call graph result is an inferred candidate",
            ),
            "source_fact" | "precise_fact" | "freshness" => {}
            other => push_public_caveat(
                &mut caveats,
                &mut seen,
                other,
                "result reliability is not exact",
            ),
        }
    }

    if public_output_truncated(value) {
        push_public_caveat(
            &mut caveats,
            &mut seen,
            "truncated_output",
            "output was truncated; narrow the query or increase limit/context",
        );
    }

    if guard_triggered {
        let message = broad_guard_public_message(value);
        push_public_caveat(&mut caveats, &mut seen, "broad_query_guard", &message);
    }

    caveats
}

fn push_public_caveat(
    caveats: &mut Vec<Value>,
    seen: &mut std::collections::BTreeSet<String>,
    code: &str,
    message: &str,
) {
    let (severity, category) = caveat_metadata(code);
    push_public_caveat_with(caveats, seen, code, message, severity, category);
}

fn push_public_caveat_with(
    caveats: &mut Vec<Value>,
    seen: &mut std::collections::BTreeSet<String>,
    code: &str,
    message: &str,
    severity: &str,
    category: &str,
) {
    if seen.insert(code.to_string()) {
        caveats.push(json!({
            "code": code,
            "message": message,
            "severity": severity,
            "category": category
        }));
    }
}

pub(super) fn caveat_metadata(code: &str) -> (&'static str, &'static str) {
    match code {
        "precise_scip_index_unavailable"
        | "parser_fact"
        | "refs_identifier_boundary_text_search_unless_a_precise_occurrence_index_is_available"
        | "query_input_expanded"
        | "inferred_candidate" => ("info", "capability"),
        "unknown_tool" | "invalid_mcp_argument" | "unsupported_mcp_scope" | "cli_usage_error" => {
            ("error", "error")
        }
        _ => ("warning", "risk"),
    }
}

fn results_contain_truncation(value: &Value) -> bool {
    value
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|result| {
            result.get("truncated").and_then(Value::as_bool) == Some(true)
                || result.get("previewTruncated").and_then(Value::as_bool) == Some(true)
                || result
                    .get("context")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|line| line.get("truncated").and_then(Value::as_bool) == Some(true))
        })
}

pub(super) fn public_page_truncated(value: &Value) -> bool {
    if parser_candidate_budget_exceeded(value) {
        return true;
    }
    if public_output_truncated(value) {
        return true;
    }
    if value.pointer("/guard/triggered").and_then(Value::as_bool) == Some(true) {
        return value
            .pointer("/guard/suppressedResults")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0;
    }
    false
}

fn public_output_truncated(value: &Value) -> bool {
    if results_contain_truncation(value) {
        return true;
    }
    let has_next_cursor = value.get("nextCursor").and_then(Value::as_str).is_some();
    let guard_triggered = value.pointer("/guard/triggered").and_then(Value::as_bool) == Some(true);
    value.get("truncated").and_then(Value::as_bool) == Some(true)
        && !has_next_cursor
        && !guard_triggered
}

fn parser_candidate_budget_exceeded(value: &Value) -> bool {
    value
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|warning| {
            warning.get("code").and_then(Value::as_str)
                == Some("tree_sitter_candidate_budget_exceeded")
        })
}

fn broad_guard_public_message(value: &Value) -> String {
    let reason = value
        .pointer("/guard/reason")
        .and_then(Value::as_str)
        .unwrap_or("broad_query");
    let suppressed = value
        .pointer("/guard/suppressedResults")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if suppressed > 0 {
        format!(
            "broad query guard triggered: {reason}; showing sample results and suppressing {suppressed}; narrow the query or rerun with --allow-broad and an explicit --limit"
        )
    } else {
        format!(
            "broad query guard triggered: {reason}; narrow the query or rerun with --allow-broad and an explicit --limit"
        )
    }
}

pub(super) fn stable_code(message: &str) -> String {
    if message.starts_with("failed to read ") {
        return "read_failed".to_string();
    }
    if message.starts_with("invalid line range: ") {
        return "invalid_line_range".to_string();
    }
    if message.starts_with("path escapes workspace root: ") {
        return "path_escapes_workspace_root".to_string();
    }
    if message == "binary_file_not_displayed" {
        return "binary_file_not_displayed".to_string();
    }
    if message == "large_file_truncated" {
        return "large_file_truncated".to_string();
    }
    if message.starts_with("no_match: ") {
        return "no_match".to_string();
    }
    if message.starts_with("precise_scip_index_unavailable") {
        return "precise_scip_index_unavailable".to_string();
    }
    if message.starts_with("failed to parse native SCIP index ") {
        return "failed_to_parse_native_scip_index".to_string();
    }
    if message.starts_with("failed to resolve path ") {
        return "workspace_path_resolve_failed".to_string();
    }
    if message.starts_with("partial parse with syntax errors: ") {
        return "partial_parse_syntax_errors".to_string();
    }
    if message.starts_with("unsupported search mode: ") {
        return "unsupported_search_mode".to_string();
    }
    if message.starts_with("query_input_expanded:") {
        return "query_input_expanded".to_string();
    }
    if message
        .starts_with("refs is identifier-boundary text search unless a precise occurrence index")
    {
        return "refs_identifier_boundary_text_search_unless_a_precise_occurrence_index_is_available"
            .to_string();
    }
    if let Some((prefix, _details)) = message.split_once(':') {
        return slug_code(prefix);
    }

    slug_code(message)
}

fn slug_code(message: &str) -> String {
    let mut code = String::new();
    let mut last_was_sep = false;
    for ch in message.chars() {
        if ch.is_ascii_alphanumeric() {
            code.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep && !code.is_empty() {
            code.push('_');
            last_was_sep = true;
        }
    }
    while code.ends_with('_') {
        code.pop();
    }
    if code.is_empty() {
        "warning".to_string()
    } else {
        code
    }
}

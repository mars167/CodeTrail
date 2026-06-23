use serde_json::Value;

pub(super) fn caveat_metadata(code: &str) -> (&'static str, &'static str) {
    match code {
        "precise_scip_index_unavailable"
        | "parser_fact"
        | "query_input_expanded"
        | "source_context_fallback"
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
                || result.pointer("/source/truncated").and_then(Value::as_bool) == Some(true)
                || result
                    .pointer("/relations/truncated")
                    .and_then(Value::as_bool)
                    == Some(true)
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

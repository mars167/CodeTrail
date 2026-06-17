use serde_json::{json, Value};

use super::caveats::{caveat_metadata, stable_code};

mod command;
mod guidance;
mod read_actions;

pub(super) use guidance::{attach_ambiguity, attach_no_match, supports_no_match};
pub use read_actions::with_workspace_root;
pub(in crate::output) use read_actions::{
    enrich_results, next_actions_from_results, suggested_reads,
};

pub fn no_match_exit(results: &Value) -> i32 {
    match results.as_array() {
        Some(values) if values.is_empty() => 2,
        _ => 0,
    }
}

pub(super) fn normalized_query(query: Value) -> Value {
    match query {
        Value::Object(mut object) => {
            object
                .entry("normalized")
                .or_insert_with(|| Value::Bool(true));
            Value::Object(object)
        }
        other => json!({
            "normalized": true,
            "value": other
        }),
    }
}

pub(super) fn structured_warnings(warnings: Vec<String>) -> Value {
    Value::Array(
        warnings
            .into_iter()
            .map(|message| {
                let code = stable_code(&message);
                let (severity, category) = caveat_metadata(&code);
                json!({
                    "code": code,
                    "message": message,
                    "severity": severity,
                    "category": category
                })
            })
            .collect(),
    )
}

pub(super) fn response_summary(results: &Value, warnings: &[String], index: &Value) -> Value {
    let result_count = results.as_array().map(Vec::len).unwrap_or(0);
    let truncated_count = results
        .as_array()
        .into_iter()
        .flatten()
        .filter(|result| {
            result.get("truncated").and_then(Value::as_bool) == Some(true)
                || result.get("previewTruncated").and_then(Value::as_bool) == Some(true)
                || result
                    .get("context")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|line| line.get("truncated").and_then(Value::as_bool) == Some(true))
        })
        .count();
    let skipped_count = warnings
        .iter()
        .filter(|warning| {
            matches!(
                warning.as_str(),
                "binary_file_not_displayed" | "unreadable_file_skipped"
            )
        })
        .count();
    let scan_skipped_count = index
        .pointer("/scanSummary/skippedCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let scan_summary = index
        .get("scanSummary")
        .cloned()
        .unwrap_or_else(|| json!({}));
    json!({
        "resultCount": result_count,
        "truncatedCount": truncated_count,
        "skippedCount": skipped_count as u64 + scan_skipped_count,
        "scan": scan_summary
    })
}

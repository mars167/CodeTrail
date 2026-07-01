use std::{fs, path::Path};

use serde_json::{json, Value};

use super::command::command_string_from_argv;
use crate::navigation;

pub(in crate::output) fn enrich_results(results: Value) -> Value {
    let Value::Array(values) = results else {
        return results;
    };

    Value::Array(values.into_iter().map(enrich_result).collect())
}

fn enrich_result(result: Value) -> Value {
    let Value::Object(mut object) = result else {
        return result;
    };
    if is_readable_path_result(&object) && !object.contains_key("sourceTarget") {
        if let Some(path) = object.get("path").and_then(Value::as_str) {
            let target = read_target(path, object.get("range"));
            object.insert("sourceTarget".to_string(), Value::String(target));
        }
    }
    let mut value = Value::Object(object);
    navigation::attach_navigation_metadata(&mut value);
    value
}

pub fn with_workspace_root(mut value: Value, root: &Path) -> Value {
    if let Some(results) = value.get_mut("results").and_then(Value::as_array_mut) {
        for result in results {
            enrich_result_with_root(result, root);
        }
    }
    if let Some(actions) = value.get_mut("nextActions").and_then(Value::as_array_mut) {
        let root = root.to_string_lossy().to_string();
        for action in actions {
            enrich_action_with_root(action, &root);
        }
    }
    let suggested_reads = suggested_reads(&value["results"]);
    let mut next_actions = non_read_next_actions(&value["nextActions"]);
    next_actions.extend(
        next_actions_from_results(&value["results"])
            .as_array()
            .into_iter()
            .flatten()
            .cloned(),
    );
    value["suggestedReads"] = suggested_reads;
    value["nextActions"] = Value::Array(next_actions);
    value
}

fn enrich_action_with_root(action: &mut Value, root: &str) {
    if action.get("kind").and_then(Value::as_str) == Some("source_read") {
        return;
    }
    let Some(argv) = action.get("argv").and_then(Value::as_array) else {
        return;
    };
    let mut argv: Vec<String> = argv
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect();
    if argv.first().map(String::as_str) != Some("codetrail") || has_path_arg(&argv) {
        return;
    }
    argv.insert(1, "--path".to_string());
    argv.insert(2, root.to_string());
    action["argv"] = json!(argv);
    action["command"] = Value::String(command_string_from_argv(&action["argv"]));
}

fn has_path_arg(argv: &[String]) -> bool {
    argv.iter()
        .any(|arg| arg == "--path" || arg.starts_with("--path="))
}

fn enrich_result_with_root(result: &mut Value, root: &Path) {
    let Value::Object(object) = result else {
        return;
    };
    if !is_readable_path_result(object) {
        object.remove("sourceTarget");
        return;
    }
    let Some(path) = object.get("path").and_then(Value::as_str) else {
        return;
    };
    let target = read_target_with_root(root, path, object.get("range"));
    object.insert("sourceTarget".to_string(), Value::String(target));
}

fn is_readable_path_result(object: &serde_json::Map<String, Value>) -> bool {
    let Some(path) = object.get("path").and_then(Value::as_str) else {
        return false;
    };
    if !crate::path_compat::is_portable_relative_path(path)
        || path == ".codetrail"
        || path.starts_with(".codetrail/")
        || path.contains("/.codetrail/")
    {
        return false;
    }
    if object.get("indexStatus").and_then(Value::as_str) == Some("D")
        || object.get("worktreeStatus").and_then(Value::as_str) == Some("D")
    {
        return false;
    }
    let full_content_truncated = object.contains_key("content")
        && object.get("truncated").and_then(Value::as_bool) == Some(true);
    if object.get("binary").and_then(Value::as_bool) == Some(true) || full_content_truncated {
        return false;
    }
    object.get("kind").and_then(Value::as_str) != Some("directory")
}

fn read_target_with_root(root: &Path, path: &str, range: Option<&Value>) -> String {
    if range.is_some() && should_read_full_small_file(root, path) {
        path.to_string()
    } else {
        read_target(path, range)
    }
}

fn should_read_full_small_file(root: &Path, path: &str) -> bool {
    fs::metadata(root.join(path))
        .map(|metadata| {
            metadata.is_file() && metadata.len() <= crate::search::MAX_FULL_READ_BYTES as u64
        })
        .unwrap_or(false)
}

fn read_target(path: &str, range: Option<&Value>) -> String {
    let Some(range) = range else {
        return path.to_string();
    };
    let start = range
        .pointer("/start/line")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let end = range
        .pointer("/end/line")
        .and_then(Value::as_u64)
        .unwrap_or(start);
    if start == end {
        format!("{path}:{start}")
    } else {
        format!("{path}:{start}-{end}")
    }
}

pub(in crate::output) fn suggested_reads(results: &Value) -> Value {
    Value::Array(
        unique_source_targets(results)
            .into_iter()
            .map(Value::String)
            .collect(),
    )
}

pub(in crate::output) fn next_actions_from_results(results: &Value) -> Value {
    let mut seen = Vec::<String>::new();
    let actions = results
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|result| {
            let target = result.get("sourceTarget").and_then(Value::as_str)?;
            if seen.iter().any(|existing| existing == target) {
                return None;
            }
            seen.push(target.to_string());
            Some(json!({
                "kind": "source_read",
                "target": target,
                "reason": "verify_source_before_edit"
            }))
        })
        .take(5)
        .collect();
    Value::Array(actions)
}

fn unique_source_targets(results: &Value) -> Vec<String> {
    let mut targets = Vec::<String>::new();
    for result in results.as_array().into_iter().flatten() {
        let Some(target) = result.get("sourceTarget").and_then(Value::as_str) else {
            continue;
        };
        if targets.iter().any(|existing| existing == target) {
            continue;
        }
        targets.push(target.to_string());
        if targets.len() == 5 {
            break;
        }
    }
    targets
}

fn non_read_next_actions(actions: &Value) -> Vec<Value> {
    actions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|action| {
            !matches!(
                action.get("kind").and_then(Value::as_str),
                Some("read" | "source_read")
            )
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;

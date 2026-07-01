use serde::Serialize;
use serde_json::{json, Value};

use super::caveats::public_page_truncated;

#[derive(Debug, Serialize)]
pub(super) struct PublicResponse {
    pub(super) results: Value,
    pub(super) page: PublicPage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicPage {
    pub(super) truncated: bool,
    pub(super) next_cursor: Value,
}

pub(super) fn public_response(value: &Value) -> PublicResponse {
    PublicResponse {
        results: public_results(value),
        page: PublicPage {
            truncated: public_page_truncated(value),
            next_cursor: value.get("nextCursor").cloned().unwrap_or(Value::Null),
        },
        error: public_error(value),
    }
}

pub fn public_response_value(value: &Value) -> Value {
    serde_json::to_value(public_response(value)).unwrap_or_else(|_| {
        json!({
            "results": [],
            "page": {
                "truncated": false,
                "nextCursor": null
            },
            "error": {
                "code": "serialization_error",
                "message": "failed to serialize public response"
            }
        })
    })
}

fn public_error(value: &Value) -> Option<Value> {
    if value.get("ok").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    value.get("error").cloned()
}

fn public_results(value: &Value) -> Value {
    let Some(results) = value.get("results").and_then(Value::as_array) else {
        return Value::Array(Vec::new());
    };
    Value::Array(results.iter().map(public_result).collect())
}

fn public_result(result: &Value) -> Value {
    let Value::Object(object) = result else {
        return result.clone();
    };
    let mut object = object.clone();
    sanitize_public_object(&mut object);
    Value::Object(object)
}

fn sanitize_public_object(object: &mut serde_json::Map<String, Value>) {
    let is_source_object = object.contains_key("rangeKind") && object.contains_key("content");
    let is_relations_object = object.contains_key("calls") && object.contains_key("callers");
    for value in object.values_mut() {
        sanitize_public_value(value);
    }
    object
        .retain(|key, value| keep_public_field(key, value, is_source_object, is_relations_object));
}

fn sanitize_public_value(value: &mut Value) {
    match value {
        Value::Object(object) => sanitize_public_object(object),
        Value::Array(values) => {
            for value in values {
                sanitize_public_value(value);
            }
        }
        _ => {}
    }
}

fn keep_public_field(
    key: &str,
    value: &Value,
    is_source_object: bool,
    is_relations_object: bool,
) -> bool {
    if is_internal_public_field(key) {
        return false;
    }
    if value.is_null() {
        if is_source_object && key == "truncatedReason" {
            return true;
        }
        return false;
    }
    if matches!(key, "context" | "warnings") {
        return !value.as_array().is_some_and(Vec::is_empty);
    }
    if matches!(key, "previewTruncated" | "truncated" | "binary") {
        if key == "truncated" && (is_source_object || is_relations_object) {
            return true;
        }
        return value.as_bool().unwrap_or(true);
    }
    if key == "warning" {
        return value.as_str().is_some_and(|warning| !warning.is_empty());
    }
    if matches!(key, "level" | "layer") && value.as_str().is_some_and(is_internal_reliability_label)
    {
        return false;
    }
    true
}

fn is_internal_public_field(key: &str) -> bool {
    matches!(
        key,
        "bodyHash"
            | "fileHash"
            | "readCommand"
            | "readCommandArgv"
            | "sourceTarget"
            | "symbol_id"
            | "producer"
            | "sourceReason"
            | "indexFresh"
            | "reliability"
            | "exact"
            | "knownBlindSpots"
            | "previewTruncatedReason"
    )
}

fn is_internal_reliability_label(value: &str) -> bool {
    matches!(
        value,
        "source_fact"
            | "precise_fact"
            | "parser_fact"
            | "inferred_candidate"
            | "freshness"
            | "remote_verified"
            | "remote_unverified"
    )
}

use serde_json::{json, Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolCoordinate {
    pub path: String,
    pub line: u64,
    pub symbol: String,
}

pub fn parse_symbol_coordinate(input: &str) -> Option<SymbolCoordinate> {
    let trimmed = input.trim();
    let (prefix, symbol) = trimmed.rsplit_once('#')?;
    let (path, line) = prefix.rsplit_once(':')?;
    let line = line.parse::<u64>().ok()?;
    if path.is_empty() || symbol.trim().is_empty() || line == 0 {
        return None;
    }
    Some(SymbolCoordinate {
        path: path.to_string(),
        line,
        symbol: symbol.trim().to_string(),
    })
}

pub fn attach_navigation_metadata(value: &mut Value) {
    attach_navigation_metadata_inner(value);
}

pub fn query_inputs_for(path: &str, range: &Value, name: &str) -> Option<Value> {
    let line = range.pointer("/start/line").and_then(Value::as_u64)?;
    if path.is_empty() || name.trim().is_empty() || line == 0 {
        return None;
    }
    let coordinate = format!("{path}:{line}#{}", name.trim());
    Some(json!({
        "defs": coordinate,
        "symbols": coordinate,
        "calls": coordinate,
        "callers": coordinate,
        "callHierarchy": coordinate,
    }))
}

pub fn query_inputs_for_object(object: &Map<String, Value>) -> Option<Value> {
    if object.contains_key("content") && object.contains_key("rangeKind") {
        return None;
    }
    if object.get("role").and_then(Value::as_str) == Some("reference") {
        return None;
    }
    let path = object.get("path").and_then(Value::as_str)?;
    let range = object.get("range")?;
    let name = navigation_name(object)?;
    query_inputs_for(path, range, name)
}

pub fn coordinate_unresolved_warning_for_query(query: &Value, results: &Value) -> Option<String> {
    let coordinate = coordinate_from_query(query)?;
    let results = results.as_array()?;
    if results.is_empty()
        || results
            .iter()
            .any(|value| coordinate_matches_tree(&coordinate, value))
    {
        return None;
    }
    Some(symbol_coordinate_unresolved_warning(&coordinate))
}

pub fn symbol_coordinate_unresolved_warning(coordinate: &SymbolCoordinate) -> String {
    format!(
        "symbol_coordinate_unresolved: no exact match for {}:{}#{}; fell back to compatible symbol input",
        coordinate.path, coordinate.line, coordinate.symbol
    )
}

fn reference_input_for_object(object: &Map<String, Value>) -> Option<String> {
    let path = object.get("path").and_then(Value::as_str)?;
    let range = object.get("range")?;
    let line = range.pointer("/start/line").and_then(Value::as_u64)?;
    let name = navigation_name(object)?;
    Some(format!("{path}:{line}#{name}"))
}

fn coordinate_from_query(query: &Value) -> Option<SymbolCoordinate> {
    [
        query.get("identifier"),
        query.get("query"),
        query.get("symbol"),
        query.get("value"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find_map(parse_symbol_coordinate)
}

fn coordinate_matches_tree(coord: &SymbolCoordinate, value: &Value) -> bool {
    if coordinate_matches_value(coord, value) {
        return true;
    }
    match value {
        Value::Object(object) => object
            .values()
            .any(|child| coordinate_matches_tree(coord, child)),
        Value::Array(values) => values
            .iter()
            .any(|child| coordinate_matches_tree(coord, child)),
        _ => false,
    }
}

pub fn coordinate_matches_value(coord: &SymbolCoordinate, value: &Value) -> bool {
    let Value::Object(object) = value else {
        return false;
    };
    let Some(path) = object.get("path").and_then(Value::as_str) else {
        return false;
    };
    if path != coord.path {
        return false;
    }
    let Some(range) = object.get("range") else {
        return false;
    };
    if !range_contains_line(range, coord.line) {
        return false;
    }
    navigation_names(object).into_iter().any(|name| {
        name == coord.symbol
            || last_identifier(name) == coord.symbol
            || strip_signature(name) == coord.symbol
            || last_identifier(&strip_signature(name)) == coord.symbol
    })
}

pub fn coordinate_matches_parts(
    coord: &SymbolCoordinate,
    path: Option<&str>,
    start_line: Option<u64>,
    end_line: Option<u64>,
    names: &[&str],
) -> bool {
    if path != Some(coord.path.as_str()) {
        return false;
    }
    let Some(start_line) = start_line else {
        return false;
    };
    let end_line = end_line.unwrap_or(start_line);
    if coord.line < start_line || coord.line > end_line {
        return false;
    }
    names.iter().any(|name| {
        *name == coord.symbol
            || last_identifier(name) == coord.symbol
            || strip_signature(name) == coord.symbol
            || last_identifier(&strip_signature(name)) == coord.symbol
    })
}

fn attach_navigation_metadata_inner(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.get("role").and_then(Value::as_str) == Some("reference")
                && !object.contains_key("referenceInput")
            {
                if let Some(input) = reference_input_for_object(object) {
                    object.insert("referenceInput".to_string(), Value::String(input));
                }
            }
            if !object.contains_key("queryInputs") {
                if let Some(inputs) = query_inputs_for_object(object) {
                    object.insert("queryInputs".to_string(), inputs);
                }
            }
            for value in object.values_mut() {
                attach_navigation_metadata_inner(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                attach_navigation_metadata_inner(value);
            }
        }
        _ => {}
    }
}

fn navigation_name(object: &Map<String, Value>) -> Option<&str> {
    for field in ["symbolName", "name", "signature", "detail", "qualifiedName"] {
        if let Some(value) = object.get(field).and_then(Value::as_str) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn navigation_names(object: &Map<String, Value>) -> Vec<&str> {
    ["symbolName", "name", "signature", "detail", "qualifiedName"]
        .into_iter()
        .filter_map(|field| object.get(field).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .collect()
}

fn range_contains_line(range: &Value, line: u64) -> bool {
    let Some(start) = range.pointer("/start/line").and_then(Value::as_u64) else {
        return false;
    };
    let end = range
        .pointer("/end/line")
        .and_then(Value::as_u64)
        .unwrap_or(start);
    start <= line && line <= end
}

fn strip_signature(value: &str) -> String {
    value
        .find('(')
        .map(|idx| value[..idx].trim().to_string())
        .unwrap_or_else(|| value.trim().to_string())
}

fn last_identifier(value: &str) -> &str {
    value
        .rsplit(['.', ':', '#', '$'])
        .find(|part| !part.is_empty())
        .unwrap_or(value)
        .trim()
}

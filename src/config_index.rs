use std::collections::HashSet;

use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    lancedb_store::{ConfigFactRow, LanceDbStore},
    query_input::{attach_matched_input, InputPlan, InputVariant, SymbolMatchMode},
    search::line_range_for_node,
    workspace::{ScanOptions, Workspace},
};

pub struct ConfigQueryOutput {
    pub results: Value,
    pub index: Value,
}

pub fn symbols(
    workspace: &Workspace,
    opts: &ScanOptions,
    query: &str,
) -> Result<Option<ConfigQueryOutput>> {
    let plan = InputPlan::new(query, opts.input_mode);
    let rows = matching_mybatis_facts(workspace, opts, &plan, SymbolMatchMode::Contains)?;
    Ok(output_from_rows(rows, opts, "symbol"))
}

pub fn defs(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<ConfigQueryOutput>> {
    let plan = InputPlan::new(identifier, opts.input_mode);
    let rows = matching_mybatis_facts(workspace, opts, &plan, SymbolMatchMode::Exact)?
        .into_iter()
        .filter(|(row, _)| is_mybatis_definition(&row.fact_kind))
        .collect();
    Ok(output_from_rows(rows, opts, "definition"))
}

pub fn refs(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<ConfigQueryOutput>> {
    let plan = InputPlan::new(identifier, opts.input_mode);
    let rows = matching_mybatis_facts(workspace, opts, &plan, SymbolMatchMode::Exact)?
        .into_iter()
        .filter(|(row, _)| row.fact_kind == "my_batis_reference")
        .collect();
    Ok(output_from_rows(rows, opts, "reference"))
}

fn matching_mybatis_facts<'a>(
    workspace: &Workspace,
    opts: &ScanOptions,
    plan: &'a InputPlan,
    mode: SymbolMatchMode,
) -> Result<Vec<(ConfigFactRow, &'a InputVariant)>> {
    let store = match LanceDbStore::open_or_create(&workspace.root) {
        Ok(store) => store,
        Err(_) => return Ok(Vec::new()),
    };
    let rows = match store.read_config_facts(&workspace.snapshot_id) {
        Ok(rows) => rows,
        Err(_) => return Ok(Vec::new()),
    };
    let allowed_paths = allowed_scan_paths(workspace, opts)?;
    let mut matches = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        if !allowed_paths.contains(&row.file_path) || !is_mybatis_fact(&row.fact_kind) {
            continue;
        }
        if let Some(variant) = matched_variant(&row, plan, opts.case_sensitive, mode) {
            let key = (
                row.file_path.clone(),
                row.fact_kind.clone(),
                row.key_path.clone(),
                row.name.clone(),
                row.range_start_line,
                row.range_start_col,
                row.range_end_line,
                row.range_end_col,
            );
            if !seen.insert(key) {
                continue;
            }
            matches.push((row, variant));
        }
    }
    matches.sort_by(|(left, _), (right, _)| {
        left.file_path
            .cmp(&right.file_path)
            .then(left.range_start_line.cmp(&right.range_start_line))
            .then(left.name.cmp(&right.name))
    });
    Ok(matches)
}

fn output_from_rows(
    mut rows: Vec<(ConfigFactRow, &InputVariant)>,
    opts: &ScanOptions,
    role: &str,
) -> Option<ConfigQueryOutput> {
    if opts.limit > 0 && rows.len() > opts.limit {
        rows.truncate(opts.limit);
    }
    if rows.is_empty() {
        return None;
    }
    let results = rows
        .into_iter()
        .map(|(row, variant)| attach_matched_input(row_to_json(row, role), variant))
        .collect::<Vec<_>>();
    Some(ConfigQueryOutput {
        results: Value::Array(results),
        index: json!({
            "used": true,
            "fresh": true,
            "source": "config_facts",
            "fallback": false
        }),
    })
}

fn row_to_json(row: ConfigFactRow, role: &str) -> Value {
    let name = row
        .name
        .clone()
        .or_else(|| row.key_path.clone())
        .unwrap_or_else(|| row.fact_kind.clone());
    json!({
        "path": row.file_path,
        "name": name,
        "symbolName": name,
        "kind": mybatis_kind(&row.fact_kind),
        "candidateKind": mybatis_kind(&row.fact_kind),
        "language": "xml",
        "container": mybatis_container(&name),
        "role": role,
        "range": line_range_for_node(
            row.range_start_line as usize,
            row.range_start_col as usize,
            row.range_end_line as usize,
            row.range_end_col as usize,
        ),
        "fileHash": row.file_hash,
        "producer": row.producer,
        "reliability": row.reliability,
        "layer": "config_fact",
        "exact": false,
        "valuePreview": row.value_preview,
        "previewMasked": row.preview_masked,
        "warning": "config_fact: MyBatis XML is configuration evidence, not precise semantic reference resolution"
    })
}

fn matched_variant<'a>(
    row: &ConfigFactRow,
    plan: &'a InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Option<&'a InputVariant> {
    row.name
        .as_deref()
        .into_iter()
        .chain(row.key_path.as_deref())
        .chain(row.name.as_deref().and_then(simple_name))
        .chain(row.key_path.as_deref().and_then(simple_name))
        .find_map(|candidate| plan.matched_variant(candidate, case_sensitive, mode))
}

fn allowed_scan_paths(workspace: &Workspace, opts: &ScanOptions) -> Result<HashSet<String>> {
    let mut scan_opts = opts.clone();
    scan_opts.limit = 0;
    Ok(workspace
        .scan_catalog(&scan_opts)?
        .into_iter()
        .map(|file| file.path)
        .collect())
}

fn is_mybatis_fact(kind: &str) -> bool {
    matches!(
        kind,
        "my_batis_namespace"
            | "my_batis_statement"
            | "my_batis_result_map"
            | "my_batis_sql_fragment"
            | "my_batis_reference"
    )
}

fn is_mybatis_definition(kind: &str) -> bool {
    matches!(
        kind,
        "my_batis_namespace"
            | "my_batis_statement"
            | "my_batis_result_map"
            | "my_batis_sql_fragment"
    )
}

fn mybatis_kind(kind: &str) -> &'static str {
    match kind {
        "my_batis_namespace" => "mapper_namespace",
        "my_batis_statement" => "mapper_statement",
        "my_batis_result_map" => "mapper_result_map",
        "my_batis_sql_fragment" => "mapper_sql_fragment",
        "my_batis_reference" => "mapper_reference",
        _ => "config_fact",
    }
}

fn mybatis_container(name: &str) -> Option<String> {
    name.rsplit_once('.')
        .map(|(container, _)| container.to_string())
}

fn simple_name(value: &str) -> Option<&str> {
    value.rsplit('.').find(|part| !part.is_empty())
}

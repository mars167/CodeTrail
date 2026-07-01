use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::Path,
};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    index, lancedb_store,
    lsp::scip_gen,
    navigation,
    query_input::{attach_matched_input, InputPlan, InputVariant, SymbolMatchMode},
    scip,
    scip::store::{OccurrenceResult, SymbolResult},
    search,
    workspace::{ScanOptions, Workspace},
};

const ROLE_DEFINITION: i32 = 1;
const OCCURRENCES_MAGIC: &[u8; 8] = b"CSOCC1\0\0";

#[derive(Clone, Debug)]
struct PreciseOccurrenceRecord {
    path: String,
    language: String,
    symbol: String,
    name: String,
    kind: String,
    role: String,
    range: PreciseRange,
    file_hash: String,
    producer: String,
}

#[derive(Clone, Debug)]
struct PreciseRange {
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScipJsonIndex {
    documents: Vec<ScipDocument>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScipDocument {
    #[serde(alias = "relative_path")]
    relative_path: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    occurrences: Vec<ScipOccurrence>,
    #[serde(default)]
    symbols: Vec<ScipSymbol>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScipOccurrence {
    range: Vec<usize>,
    symbol: String,
    #[serde(default, alias = "symbol_roles")]
    symbol_roles: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScipSymbol {
    symbol: String,
    #[serde(default, alias = "display_name")]
    display_name: String,
    #[serde(default)]
    kind: Value,
}

pub struct PreciseQueryOutput {
    pub results: Value,
    pub index: Value,
}

pub fn import_scip_json(workspace: &Workspace, path: impl AsRef<Path>) -> Result<Value> {
    let source_path = path.as_ref();
    let input = fs::read(source_path)
        .with_context(|| format!("failed to read SCIP JSON {}", source_path.display()))?;
    let parsed: ScipJsonIndex = serde_json::from_slice(&input)
        .with_context(|| "failed to parse SCIP JSON; binary index.scip protobuf import is not available in this build")?;

    let root = index::scip_root(workspace);
    fs::create_dir_all(&root)?;

    let mut records = Vec::new();
    for document in parsed.documents {
        let symbols = document
            .symbols
            .iter()
            .map(|symbol| (symbol.symbol.as_str(), symbol))
            .collect::<HashMap<_, _>>();
        let file_hash = current_file_hash(workspace, &document.relative_path).unwrap_or_default();
        for occurrence in document.occurrences {
            if occurrence.symbol.is_empty() || occurrence.range.is_empty() {
                continue;
            }
            let Some(range) = scip_range(&occurrence.range) else {
                continue;
            };
            let symbol_info = symbols.get(occurrence.symbol.as_str());
            let name = symbol_info
                .and_then(|info| (!info.display_name.is_empty()).then(|| info.display_name.clone()))
                .unwrap_or_else(|| display_name_from_symbol(&occurrence.symbol));
            let kind = symbol_info
                .map(|info| kind_to_string(&info.kind))
                .filter(|kind| !kind.is_empty())
                .unwrap_or_else(|| "symbol".to_string());
            records.push(PreciseOccurrenceRecord {
                path: document.relative_path.clone(),
                language: document.language.clone(),
                symbol: occurrence.symbol,
                name,
                kind,
                role: if occurrence.symbol_roles & ROLE_DEFINITION != 0 {
                    "definition".to_string()
                } else {
                    "reference".to_string()
                },
                range,
                file_hash: file_hash.clone(),
                producer: "scip".to_string(),
            });
        }
    }

    import_to_lancedb(workspace, &records)?;

    Ok(json!({
        "index": {
            "used": true,
            "fresh": true,
            "source": "scip_json",
            "path": root,
            "storageBackend": "lancedb",
            "recordCount": records.len(),
            "definitionCount": records.iter().filter(|record| record.role == "definition").count()
        }
    }))
}

/// Import a native SCIP binary protobuf index (index.scip) and build the occurrence DB.
pub fn import_native_scip(workspace: &Workspace, path: impl AsRef<Path>) -> Result<Value> {
    let source_path = path.as_ref();
    let scip_index = scip::parse_native_scip(source_path).with_context(|| {
        format!(
            "failed to parse native SCIP index {}",
            source_path.display()
        )
    })?;

    let snapshot_hash = &workspace.snapshot_id;
    let db_path = native_db_path(workspace);

    scip::build_occurrences_db(&scip_index, &db_path, snapshot_hash, &workspace.root)
        .with_context(|| "failed to build occurrence database")?;

    let occ_count: usize = scip_index
        .documents
        .iter()
        .map(|d| d.occurrences.len())
        .sum();
    let sym_count: usize = scip_index.documents.iter().map(|d| d.symbols.len()).sum();

    Ok(json!({
        "index": {
            "used": true,
            "fresh": true,
            "source": "scip_native_protobuf",
            "path": db_path,
            "recordCount": occ_count,
            "definitionCount": sym_count
        }
    }))
}

pub fn native_db_path(workspace: &Workspace) -> std::path::PathBuf {
    index::scip_root(workspace).join("occurrences.db")
}

pub fn symbols(
    workspace: &Workspace,
    opts: &ScanOptions,
    query: &str,
) -> Result<Option<PreciseQueryOutput>> {
    if let Some(output) = query_native_symbols(workspace, opts, query)? {
        return Ok(Some(output));
    }
    query_precise_with_input(
        workspace,
        opts,
        query,
        |record| record.role == "definition",
        SymbolMatchMode::Contains,
    )
}

pub fn defs(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<PreciseQueryOutput>> {
    if let Some(output) = query_native_defs(workspace, opts, identifier)? {
        return Ok(Some(output));
    }
    query_precise_with_input(
        workspace,
        opts,
        identifier,
        |record| record.role == "definition",
        SymbolMatchMode::Exact,
    )
}

pub fn refs(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<PreciseQueryOutput>> {
    if let Some(output) = query_native_refs(workspace, opts, identifier)? {
        return Ok(Some(output));
    }
    query_precise_refs_with_input(workspace, opts, identifier)
}

// ---------------------------------------------------------------------------
// Native occurrence DB query helpers
// ---------------------------------------------------------------------------

fn query_native_defs(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<PreciseQueryOutput>> {
    let db_path = native_db_path(workspace);
    if !scip::occurrence_db_fresh(&db_path, &workspace.snapshot_id, &workspace.root) {
        return Ok(None);
    }
    if !scip_gen::generation_manifests_allow_precise_use(workspace).unwrap_or(false) {
        return Ok(None);
    }
    let plan = InputPlan::new(identifier, opts.input_mode);
    let candidates =
        if opts.input_mode == crate::query_input::InputMode::Strict && plan.coordinate.is_none() {
            scip::query_defs(&db_path, identifier)?
        } else {
            scip::query_all_defs(&db_path)?
        };
    let mut results = matched_occurrence_results_with_fallback(
        candidates,
        &plan,
        opts.case_sensitive,
        SymbolMatchMode::Exact,
    );
    filter_and_limit_occurrence_matches(
        workspace,
        &mut results,
        opts,
        Some((&plan, SymbolMatchMode::Exact)),
    )?;
    if results.is_empty() {
        return Ok(Some(PreciseQueryOutput {
            results: Value::Array(Vec::new()),
            index: native_db_index_meta(&db_path, true),
        }));
    }
    let json_results = results
        .iter()
        .map(|(result, variant)| attach_matched_input(scip::occurrence_to_json(result), variant))
        .collect();
    Ok(Some(PreciseQueryOutput {
        results: Value::Array(json_results),
        index: native_db_index_meta(&db_path, true),
    }))
}

fn native_reference_to_json(db_path: &Path, result: &OccurrenceResult) -> Result<Value> {
    let mut value = scip::occurrence_to_json(result);
    if let Some(object) = value.as_object_mut() {
        if let Some(definition) = scip::query_defs_by_symbol_key(db_path, &result.symbol_key)?
            .into_iter()
            .next()
        {
            object.insert(
                "definition".to_string(),
                scip::occurrence_to_json(&definition),
            );
        }
    }
    Ok(value)
}

fn precise_reference_to_json(
    record: PreciseOccurrenceRecord,
    definitions: &HashMap<String, PreciseOccurrenceRecord>,
) -> Value {
    let symbol = record.symbol.clone();
    let mut value = record_to_json(record);
    if let Some(object) = value.as_object_mut() {
        if let Some(definition) = definitions.get(&symbol) {
            object.insert("definition".to_string(), record_to_json(definition.clone()));
        }
    }
    value
}

fn query_precise_refs_with_input(
    workspace: &Workspace,
    opts: &ScanOptions,
    input: &str,
) -> Result<Option<PreciseQueryOutput>> {
    let Some((records, index_meta)) = fresh_records(workspace, opts)? else {
        return Ok(None);
    };
    let plan = InputPlan::new(input, opts.input_mode);
    let definitions = records
        .iter()
        .filter(|record| record.role == "definition")
        .map(|record| (record.symbol.clone(), record.clone()))
        .collect::<HashMap<_, _>>();
    let mut matched_records = matched_precise_records_with_fallback(
        records
            .into_iter()
            .filter(|record| record.role != "definition")
            .collect(),
        &plan,
        opts.case_sensitive,
        SymbolMatchMode::Exact,
    );
    let mut results = Vec::new();
    for (record, variant) in matched_records.drain(..) {
        results.push(attach_matched_input(
            precise_reference_to_json(record, &definitions),
            &variant,
        ));
        if opts.limit > 0 && results.len() >= opts.limit {
            break;
        }
    }
    Ok(Some(PreciseQueryOutput {
        results: Value::Array(results),
        index: index_meta,
    }))
}

fn query_native_refs(
    workspace: &Workspace,
    opts: &ScanOptions,
    identifier: &str,
) -> Result<Option<PreciseQueryOutput>> {
    let db_path = native_db_path(workspace);
    if !scip::occurrence_db_fresh(&db_path, &workspace.snapshot_id, &workspace.root) {
        return Ok(None);
    }
    if !scip_gen::generation_manifests_allow_precise_use(workspace).unwrap_or(false) {
        return Ok(None);
    }
    let plan = InputPlan::new(identifier, opts.input_mode);
    let candidates =
        if opts.input_mode == crate::query_input::InputMode::Strict && plan.coordinate.is_none() {
            scip::query_refs(&db_path, identifier)?
        } else {
            scip::query_all_refs(&db_path)?
        };
    let mut results = matched_occurrence_results_with_fallback(
        candidates,
        &plan,
        opts.case_sensitive,
        SymbolMatchMode::Exact,
    );
    filter_and_limit_occurrence_matches(workspace, &mut results, opts, None)?;
    if results.is_empty() {
        return Ok(Some(PreciseQueryOutput {
            results: Value::Array(Vec::new()),
            index: native_db_index_meta(&db_path, true),
        }));
    }
    let json_results = results
        .iter()
        .map(|(result, variant)| {
            Ok(attach_matched_input(
                native_reference_to_json(&db_path, result)?,
                variant,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(PreciseQueryOutput {
        results: Value::Array(json_results),
        index: native_db_index_meta(&db_path, true),
    }))
}

fn query_native_symbols(
    workspace: &Workspace,
    opts: &ScanOptions,
    query: &str,
) -> Result<Option<PreciseQueryOutput>> {
    let db_path = native_db_path(workspace);
    if !scip::occurrence_db_fresh(&db_path, &workspace.snapshot_id, &workspace.root) {
        return Ok(None);
    }
    if !scip_gen::generation_manifests_allow_precise_use(workspace).unwrap_or(false) {
        return Ok(None);
    }
    let plan = InputPlan::new(query, opts.input_mode);
    let candidates =
        if opts.input_mode == crate::query_input::InputMode::Strict && plan.coordinate.is_none() {
            scip::query_symbols(&db_path, query)?
        } else {
            scip::query_all_symbols(&db_path)?
        };
    let mut results = matched_symbol_results_with_fallback(
        candidates,
        &plan,
        opts.case_sensitive,
        SymbolMatchMode::Contains,
    );
    filter_and_limit_symbol_matches(
        workspace,
        &mut results,
        opts,
        &plan,
        SymbolMatchMode::Contains,
    )?;
    if results.is_empty() {
        return Ok(Some(PreciseQueryOutput {
            results: Value::Array(Vec::new()),
            index: native_db_index_meta(&db_path, true),
        }));
    }
    let json_results = results
        .iter()
        .map(|(result, variant)| attach_matched_input(scip::symbol_to_json(result), variant))
        .collect();
    Ok(Some(PreciseQueryOutput {
        results: Value::Array(json_results),
        index: native_db_index_meta(&db_path, true),
    }))
}

fn native_db_index_meta(db_path: &std::path::Path, fresh: bool) -> Value {
    json!({
        "used": true,
        "fresh": fresh,
        "source": "scip_native",
        "fallback": false,
        "path": db_path
    })
}

fn filter_and_limit_occurrence_matches(
    workspace: &Workspace,
    results: &mut Vec<(OccurrenceResult, InputVariant)>,
    opts: &ScanOptions,
    relevance: Option<(&InputPlan, SymbolMatchMode)>,
) -> Result<()> {
    let allowed_paths = allowed_scan_paths(workspace, opts)?;
    results.retain(|(result, _variant)| allowed_paths.contains(&result.path));
    if let Some((plan, mode)) = relevance {
        results.sort_by_cached_key(|(result, _variant)| {
            search::code_result_sort_key(&scip::occurrence_to_json(result), &plan.raw, opts, mode)
        });
    }
    if opts.limit > 0 && results.len() > opts.limit {
        results.truncate(opts.limit);
    }
    Ok(())
}

fn filter_and_limit_symbol_matches(
    workspace: &Workspace,
    results: &mut Vec<(SymbolResult, InputVariant)>,
    opts: &ScanOptions,
    plan: &InputPlan,
    mode: SymbolMatchMode,
) -> Result<()> {
    let allowed_paths = allowed_scan_paths(workspace, opts)?;
    results.retain(|(result, _variant)| allowed_paths.contains(&result.path));
    results.sort_by_cached_key(|(result, _variant)| {
        search::code_result_sort_key(&scip::symbol_to_json(result), &plan.raw, opts, mode)
    });
    if opts.limit > 0 && results.len() > opts.limit {
        results.truncate(opts.limit);
    }
    Ok(())
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

// ---------------------------------------------------------------------------
// LEGACY: Old occurrence.idx binary format (compatibility path)
// ---------------------------------------------------------------------------

fn matched_occurrence_results(
    candidates: Vec<OccurrenceResult>,
    plan: &InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Vec<(OccurrenceResult, InputVariant)> {
    candidates
        .into_iter()
        .filter_map(|result| {
            occurrence_variant(&result, plan, case_sensitive, mode)
                .cloned()
                .map(|variant| (result, variant))
        })
        .collect()
}

fn matched_occurrence_results_with_fallback(
    candidates: Vec<OccurrenceResult>,
    plan: &InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Vec<(OccurrenceResult, InputVariant)> {
    if plan.coordinate.is_none() {
        return matched_occurrence_results(candidates, plan, case_sensitive, mode);
    }
    let results = matched_occurrence_results(candidates.clone(), plan, case_sensitive, mode);
    if !results.is_empty() {
        return results;
    }
    plan.coordinate_fallback_plan()
        .map(|fallback| matched_occurrence_results(candidates, &fallback, case_sensitive, mode))
        .unwrap_or(results)
}

fn matched_symbol_results(
    candidates: Vec<SymbolResult>,
    plan: &InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Vec<(SymbolResult, InputVariant)> {
    candidates
        .into_iter()
        .filter_map(|result| {
            symbol_variant(&result, plan, case_sensitive, mode)
                .cloned()
                .map(|variant| (result, variant))
        })
        .collect()
}

fn matched_symbol_results_with_fallback(
    candidates: Vec<SymbolResult>,
    plan: &InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Vec<(SymbolResult, InputVariant)> {
    if plan.coordinate.is_none() {
        return matched_symbol_results(candidates, plan, case_sensitive, mode);
    }
    let results = matched_symbol_results(candidates.clone(), plan, case_sensitive, mode);
    if !results.is_empty() {
        return results;
    }
    plan.coordinate_fallback_plan()
        .map(|fallback| matched_symbol_results(candidates, &fallback, case_sensitive, mode))
        .unwrap_or(results)
}

fn occurrence_variant<'a>(
    result: &OccurrenceResult,
    plan: &'a InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Option<&'a InputVariant> {
    if let Some(coord) = &plan.coordinate {
        let names = [
            result.name.as_str(),
            result.symbol.as_str(),
            result.symbol_key.as_str(),
        ];
        if !navigation::coordinate_matches_parts(
            coord,
            Some(&result.path),
            Some(u64::from(result.start_line)),
            Some(u64::from(result.end_line)),
            &names,
        ) {
            return None;
        }
    }
    plan.matched_variant(&result.name, case_sensitive, mode)
        .or_else(|| plan.matched_variant(&result.symbol, case_sensitive, mode))
        .or_else(|| plan.matched_variant(&result.symbol_key, case_sensitive, mode))
}

fn symbol_variant<'a>(
    result: &SymbolResult,
    plan: &'a InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Option<&'a InputVariant> {
    if let Some(coord) = &plan.coordinate {
        let names = [
            result.name.as_str(),
            result.symbol.as_str(),
            result.symbol_key.as_str(),
        ];
        if !navigation::coordinate_matches_parts(
            coord,
            Some(&result.path),
            Some(u64::from(result.start_line)),
            Some(u64::from(result.end_line)),
            &names,
        ) {
            return None;
        }
    }
    plan.matched_variant(&result.name, case_sensitive, mode)
        .or_else(|| plan.matched_variant(&result.symbol, case_sensitive, mode))
        .or_else(|| plan.matched_variant(&result.symbol_key, case_sensitive, mode))
}

fn query_precise_with_input(
    workspace: &Workspace,
    opts: &ScanOptions,
    input: &str,
    role_matches: impl Fn(&PreciseOccurrenceRecord) -> bool,
    mode: SymbolMatchMode,
) -> Result<Option<PreciseQueryOutput>> {
    let Some((records, index_meta)) = fresh_records(workspace, opts)? else {
        return Ok(None);
    };
    let plan = InputPlan::new(input, opts.input_mode);
    let matched_records = matched_precise_records_with_fallback(
        records.into_iter().filter(role_matches).collect(),
        &plan,
        opts.case_sensitive,
        mode,
    );
    let mut results = Vec::new();
    for (record, variant) in matched_records {
        results.push(attach_matched_input(record_to_json(record), &variant));
        if opts.limit > 0 && results.len() >= opts.limit {
            break;
        }
    }
    Ok(Some(PreciseQueryOutput {
        results: Value::Array(results),
        index: index_meta,
    }))
}

fn matched_precise_records_with_fallback(
    records: Vec<PreciseOccurrenceRecord>,
    plan: &InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Vec<(PreciseOccurrenceRecord, InputVariant)> {
    if plan.coordinate.is_none() {
        return matched_precise_records(records, plan, case_sensitive, mode);
    }
    let results = matched_precise_records(records.clone(), plan, case_sensitive, mode);
    if !results.is_empty() {
        return results;
    }
    plan.coordinate_fallback_plan()
        .map(|fallback| matched_precise_records(records, &fallback, case_sensitive, mode))
        .unwrap_or(results)
}

fn matched_precise_records(
    records: Vec<PreciseOccurrenceRecord>,
    plan: &InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Vec<(PreciseOccurrenceRecord, InputVariant)> {
    records
        .into_iter()
        .filter_map(|record| {
            precise_record_variant(&record, plan, case_sensitive, mode)
                .cloned()
                .map(|variant| (record, variant))
        })
        .collect()
}

fn precise_record_variant<'a>(
    record: &PreciseOccurrenceRecord,
    plan: &'a InputPlan,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> Option<&'a InputVariant> {
    if let Some(coord) = &plan.coordinate {
        let names = [record.name.as_str(), record.symbol.as_str()];
        if !navigation::coordinate_matches_parts(
            coord,
            Some(&record.path),
            Some(u64::from(record.range.start_line)),
            Some(u64::from(record.range.end_line)),
            &names,
        ) {
            return None;
        }
    }
    plan.matched_variant(&record.name, case_sensitive, mode)
        .or_else(|| plan.matched_variant(&record.symbol, case_sensitive, mode))
}

fn fresh_records(
    workspace: &Workspace,
    opts: &ScanOptions,
) -> Result<Option<(Vec<PreciseOccurrenceRecord>, Value)>> {
    let root = index::scip_root(workspace);

    let mut scan_opts = opts.clone();
    scan_opts.limit = 0;
    let allowed_paths = workspace
        .scan_files(&scan_opts)?
        .into_iter()
        .map(|file| file.path)
        .collect::<HashSet<_>>();

    if lancedb_store::is_available(&workspace.root) {
        if let Ok(store) = lancedb_store::LanceDbStore::open_or_create(&workspace.root) {
            if let Ok(lance_records) = store.read_scip_occurrences(&workspace.snapshot_id) {
                if !lance_records.is_empty() {
                    let converted = convert_scip_occurrences(lance_records);
                    let mut fresh_rows = Vec::new();
                    for record in converted {
                        if !allowed_paths.contains(&record.path) {
                            continue;
                        }
                        let hash = match current_file_hash(workspace, &record.path) {
                            Ok(hash) => hash,
                            Err(_) => return Ok(None),
                        };
                        if hash != record.file_hash {
                            return Ok(None);
                        }
                        fresh_rows.push(record);
                    }
                    return Ok(Some((
                        fresh_rows,
                        json!({
                            "used": true,
                            "fresh": true,
                            "source": "scip_json",
                            "storageBackend": "lancedb",
                            "fallback": false,
                            "path": lancedb_store::lancedb_root(&workspace.root)
                        }),
                    )));
                }
            }
        }
    }

    let path = root.join("occurrences.idx");
    if !path.exists() {
        return Ok(None);
    }

    let records = read_occurrences(&path)?;
    let mut fresh_records = Vec::new();
    for record in records {
        if !allowed_paths.contains(&record.path) {
            continue;
        }
        let hash = match current_file_hash(workspace, &record.path) {
            Ok(hash) => hash,
            Err(_) => return Ok(None),
        };
        if hash != record.file_hash {
            return Ok(None);
        }
        fresh_records.push(record);
    }

    Ok(Some((
        fresh_records,
        json!({
            "used": true,
            "fresh": true,
            "source": "scip_json",
            "fallback": true,
            "storageBackend": "idx_binary",
            "path": path
        }),
    )))
}

fn current_file_hash(workspace: &Workspace, path: &str) -> Result<String> {
    let content = fs::read(workspace.abs_path(path))?;
    Ok(format!("blake3:{}", blake3::hash(&content).to_hex()))
}

fn import_to_lancedb(workspace: &Workspace, records: &[PreciseOccurrenceRecord]) -> Result<()> {
    let store = lancedb_store::LanceDbStore::open_or_create(&workspace.root)
        .with_context(|| "failed to open LanceDB store")?;
    store
        .ensure_tables()
        .with_context(|| "failed to ensure LanceDB tables")?;
    let scip_records: Vec<lancedb_store::ScipOccurrence> = records
        .iter()
        .map(|r| lancedb_store::ScipOccurrence {
            snapshot_id: workspace.snapshot_id.clone(),
            symbol: r.symbol.clone(),
            file_path: r.path.clone(),
            language: r.language.clone(),
            name: r.name.clone(),
            kind: r.kind.clone(),
            role: r.role.clone(),
            range_start_line: r.range.start_line,
            range_start_col: r.range.start_column,
            range_end_line: r.range.end_line,
            range_end_col: r.range.end_column,
            is_definition: r.role == "definition",
            file_hash: r.file_hash.clone(),
            enclosing_symbol: None,
            producer: r.producer.clone(),
        })
        .collect();
    store
        .write_scip_occurrences(&workspace.snapshot_id, &scip_records)
        .with_context(|| "failed to write scip occurrences")?;
    Ok(())
}

fn convert_scip_occurrences(
    records: Vec<lancedb_store::ScipOccurrence>,
) -> Vec<PreciseOccurrenceRecord> {
    records
        .into_iter()
        .map(|r| PreciseOccurrenceRecord {
            path: r.file_path,
            language: r.language,
            symbol: r.symbol,
            name: r.name,
            kind: r.kind,
            role: r.role,
            range: PreciseRange {
                start_line: r.range_start_line,
                start_column: r.range_start_col,
                end_line: r.range_end_line,
                end_column: r.range_end_col,
            },
            file_hash: r.file_hash,
            producer: r.producer,
        })
        .collect()
}

fn scip_range(range: &[usize]) -> Option<PreciseRange> {
    match range {
        [start_line, start_col, end_col] => Some(PreciseRange {
            start_line: to_one_based_u32(*start_line)?,
            start_column: to_one_based_u32(*start_col)?,
            end_line: to_one_based_u32(*start_line)?,
            end_column: to_one_based_u32(*end_col)?,
        }),
        [start_line, start_col, end_line, end_col] => Some(PreciseRange {
            start_line: to_one_based_u32(*start_line)?,
            start_column: to_one_based_u32(*start_col)?,
            end_line: to_one_based_u32(*end_line)?,
            end_column: to_one_based_u32(*end_col)?,
        }),
        _ => None,
    }
}

fn display_name_from_symbol(symbol: &str) -> String {
    symbol
        .split(|ch: char| ch == '/' || ch == '#' || ch == '.' || ch.is_whitespace())
        .rfind(|part| !part.is_empty())
        .unwrap_or(symbol)
        .trim_end_matches("().")
        .to_string()
}

fn kind_to_string(kind: &Value) -> String {
    match kind {
        Value::String(value) => value.to_ascii_lowercase(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    }
}

fn record_to_json(record: PreciseOccurrenceRecord) -> Value {
    json!({
        "path": record.path,
        "name": record.name,
        "symbolName": record.name,
        "kind": record.kind,
        "symbol": record.symbol,
        "role": record.role,
        "language": record.language,
        "container": Value::Null,
        "range": {
            "start": { "line": record.range.start_line, "column": record.range.start_column },
            "end": { "line": record.range.end_line, "column": record.range.end_column }
        },
        "fileHash": record.file_hash,
        "producer": record.producer,
        "reliability": "precise_fact",
        "exact": true
    })
}

fn read_occurrences(path: &Path) -> Result<Vec<PreciseOccurrenceRecord>> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    read_magic(&mut file, OCCURRENCES_MAGIC)?;
    let count = read_u32(&mut file)? as usize;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let path = read_string(&mut file)?;
        let language = read_string(&mut file)?;
        let symbol = read_string(&mut file)?;
        let name = read_string(&mut file)?;
        let kind = read_string(&mut file)?;
        let role = read_string(&mut file)?;
        let range = PreciseRange {
            start_line: read_u32(&mut file)?,
            start_column: read_u32(&mut file)?,
            end_line: read_u32(&mut file)?,
            end_column: read_u32(&mut file)?,
        };
        let file_hash = read_string(&mut file)?;
        let producer = read_string(&mut file)?;
        records.push(PreciseOccurrenceRecord {
            path,
            language,
            symbol,
            name,
            kind,
            role,
            range,
            file_hash,
            producer,
        });
    }
    Ok(records)
}

fn to_one_based_u32(value: usize) -> Option<u32> {
    value.checked_add(1)?.try_into().ok()
}

fn read_magic(file: &mut File, expected: &[u8; 8]) -> Result<()> {
    let mut actual = [0u8; 8];
    file.read_exact(&mut actual)?;
    if &actual != expected {
        return Err(anyhow!("invalid SCIP occurrence magic"));
    }
    Ok(())
}

fn read_string(file: &mut File) -> Result<String> {
    let len = read_u32(file)? as usize;
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes)?;
    Ok(String::from_utf8(bytes)?)
}

fn read_u32(file: &mut File) -> Result<u32> {
    let mut bytes = [0u8; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

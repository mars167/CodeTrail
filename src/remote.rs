//! Remote index dispatch layer.
//!
//! When local snapshots are stale or missing, this module queries
//! remote-unpacked snapshots in `.codetrail/remote/`. Results from
//! remote sources carry `remote_verified` / `remote_unverified` reliability
//! annotations and NEVER override local state.

use std::path::PathBuf;

use anyhow::Result;
use serde_json::{json, Value};

use crate::{
    index,
    workspace::{FileRecord, ScanOptions, Workspace},
};

/// Outcome of attempting to use a remote text snapshot.
pub struct RemoteTextOutcome {
    pub records: Vec<FileRecord>,
    pub index_meta: Value,
    pub remote_verified: bool,
    pub snapshot_key: Option<String>,
}

/// Try to load a fresh text snapshot (local first, then remote).
/// Returns (records, index_meta) with appropriate reliability.
pub fn fresh_or_remote_text_records(
    workspace: &Workspace,
    opts: &ScanOptions,
) -> Result<Option<RemoteTextOutcome>> {
    // 1. Try local fresh snapshot
    if let Some((records, index_meta)) = index::fresh_file_records(workspace, opts)? {
        return Ok(Some(RemoteTextOutcome {
            records,
            index_meta,
            remote_verified: true, // local = always verified
            snapshot_key: None,
        }));
    }

    // 2. Try remote snapshots
    let remote_snapshots = index::discover_remote_snapshots(workspace)?;
    for (snapshot_key, remote_dir) in &remote_snapshots {
        let text_dir = index::remote_text_dir(workspace, snapshot_key);
        let docs_path = text_dir.join("docs.idx");
        if !docs_path.exists() {
            continue;
        }

        // Read docs.idx
        let records = match crate::text_index::read_docs(&docs_path) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Verify against local files
        let verified = index::remote_snapshot_matches_local(workspace, remote_dir).unwrap_or(false);

        let index_meta = remote_text_index_meta(snapshot_key, &text_dir, verified);

        return Ok(Some(RemoteTextOutcome {
            records,
            index_meta,
            remote_verified: verified,
            snapshot_key: Some(snapshot_key.clone()),
        }));
    }

    // 3. No index available
    Ok(None)
}

/// Try to load fresh text records with trigram prefilter, local first then remote.
pub fn fresh_or_remote_text_with_filter(
    workspace: &Workspace,
    opts: &ScanOptions,
    pattern: &str,
    mode: &str,
) -> Result<Option<RemoteTextOutcome>> {
    // 1. Try local fresh snapshot with prefilter
    if let Some((records, index_meta)) = index::fresh_text_records(workspace, opts, pattern, mode)?
    {
        return Ok(Some(RemoteTextOutcome {
            records,
            index_meta,
            remote_verified: true,
            snapshot_key: None,
        }));
    }

    // 2. Try remote snapshots
    let remote_snapshots = index::discover_remote_snapshots(workspace)?;
    for (snapshot_key, remote_dir) in &remote_snapshots {
        let text_dir = index::remote_text_dir(workspace, snapshot_key);
        let docs_path = text_dir.join("docs.idx");
        let grams_path = text_dir.join("grams.idx");
        if !docs_path.exists() || !grams_path.exists() {
            continue;
        }

        let records = match crate::text_index::read_docs(&docs_path) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Apply trigram prefilter
        let candidate_ids = crate::text_index::candidate_ids(&grams_path, pattern, mode)?;
        let filtered = match &candidate_ids {
            Some(ids) => records
                .into_iter()
                .enumerate()
                .filter_map(|(doc_id, record)| ids.contains(&doc_id).then_some(record))
                .collect::<Vec<_>>(),
            None => records,
        };

        let verified = index::remote_snapshot_matches_local(workspace, remote_dir).unwrap_or(false);

        let mut index_meta = remote_text_index_meta(snapshot_key, &text_dir, verified);
        if candidate_ids.is_some() {
            index_meta["prefilter"] = json!("trigram");
        }

        return Ok(Some(RemoteTextOutcome {
            records: filtered,
            index_meta,
            remote_verified: verified,
            snapshot_key: Some(snapshot_key.clone()),
        }));
    }

    Ok(None)
}

/// Query remote occurrence DB for defs, refs, or symbols.
/// Returns (results, index_meta, remote_verified) if available.
pub fn remote_occurrence_query<F>(
    workspace: &Workspace,
    opts: &ScanOptions,
    _local_query: F,
    remote_query: fn(&PathBuf, &str) -> Result<Vec<Value>>,
    identifier: &str,
) -> Result<Option<(Vec<Value>, Value, bool)>> {
    use crate::workspace::matches_filters;

    // Try local native DB first (handled by existing scip_index path)
    // This function is for the remote fallback path only.

    let remote_snapshots = index::discover_remote_snapshots(workspace)?;
    for (snapshot_key, remote_dir) in &remote_snapshots {
        let db_path = remote_dir.join("scip").join("occurrences.db");
        if !db_path.exists() {
            continue;
        }

        let mut results = match remote_query(&db_path, identifier) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Filter
        results.retain(|r| {
            r.get("path")
                .and_then(|p| p.as_str())
                .is_some_and(|path| matches_filters(path, &opts.include, &opts.exclude))
        });
        if opts.limit > 0 && results.len() > opts.limit {
            results.truncate(opts.limit);
        }

        let verified = index::remote_snapshot_matches_local(workspace, remote_dir).unwrap_or(false);

        let index_meta = json!({
            "used": true,
            "fresh": verified,
            "source": "scip:remote",
            "fallback": false,
            "remote_verified": verified,
            "remote_snapshot_key": snapshot_key,
            "path": db_path,
        });

        return Ok(Some((results, index_meta, verified)));
    }

    Ok(None)
}

/// Remote reliability level based on whether local files match remote snapshot.
pub fn remote_reliability(remote_verified: bool) -> Value {
    if remote_verified {
        json!({
            "level": "precise_fact",
            "source": "remote_verified",
            "exact": true,
            "remote_verified": true,
            "llm_instruction": "These results come from a remote index verified against local files. Verification is still advisory."
        })
    } else {
        json!({
            "level": "remote_unverified",
            "source": "remote_unverified",
            "exact": false,
            "remote_verified": false,
            "llm_instruction": "These results come from a remote index that does NOT match current local files. Verify every result with codetrail read before relying on them."
        })
    }
}

fn remote_text_index_meta(snapshot_key: &str, text_dir: &PathBuf, verified: bool) -> Value {
    json!({
        "used": true,
        "fresh": verified,
        "source": "text_index:remote",
        "fallback": false,
        "remote_verified": verified,
        "remote_snapshot_key": snapshot_key,
        "path": text_dir,
    })
}

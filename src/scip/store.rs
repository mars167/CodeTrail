use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::scip_proto::proto;

/// A single occurrence result from the store.
#[derive(Clone, Debug)]
pub struct OccurrenceResult {
    pub path: String,
    pub language: String,
    pub symbol: String,
    pub name: String,
    pub kind: String,
    pub role: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub file_hash: String,
}

/// A symbol result from the store.
#[derive(Clone, Debug)]
pub struct SymbolResult {
    pub name: String,
    pub kind: String,
    pub language: String,
    pub path: String,
    pub role: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// Build the occurrence database from a native SCIP Index.
pub fn build_occurrences_db(
    scip_index: &proto::Index,
    db_path: &Path,
    snapshot: &str,
) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Remove existing DB
    if db_path.exists() {
        std::fs::remove_file(db_path)?;
    }

    let conn = Connection::open(db_path)
        .with_context(|| format!("failed to open occurrence DB at {}", db_path.display()))?;

    create_schema(&conn)?;

    let mut inserted_symbols = 0usize;
    let mut inserted_occurrences = 0usize;

    let tx = conn.unchecked_transaction()?;

    for document in &scip_index.documents {
        let language = document.language.clone();
        let path = document.relative_path.clone();

        // Build symbol lookup: symbol string -> (display_name, kind)
        let symbols: std::collections::HashMap<&str, (&str, &str)> = document
            .symbols
            .iter()
            .map(|sym| {
                let kind = kind_name(sym);
                (sym.symbol.as_str(), (sym.display_name.as_str(), kind))
            })
            .collect();

        for occurrence in &document.occurrences {
            if occurrence.symbol.is_empty() || occurrence.range.is_empty() {
                continue;
            }

            let range = match occurrence.range.len() {
                3 => (
                    occurrence.range[0],
                    occurrence.range[1],
                    occurrence.range[0],
                    occurrence.range[2],
                ),
                4 => (
                    occurrence.range[0],
                    occurrence.range[1],
                    occurrence.range[2],
                    occurrence.range[3],
                ),
                _ => continue,
            };

            let role = if occurrence.symbol_roles & 0x1 != 0 {
                "definition"
            } else {
                "reference"
            };

            let default_name = display_name_from_symbol(&occurrence.symbol);
            let (display_name, kind) = symbols
                .get(occurrence.symbol.as_str())
                .copied()
                .unwrap_or_else(|| (default_name.as_str(), "symbol"));

            let display_name = if display_name.is_empty() {
                display_name_from_symbol(&occurrence.symbol)
            } else {
                display_name.to_string()
            };

            // Upsert symbol
            let symbol_id: i64 = tx
                .prepare("SELECT id FROM symbols WHERE name = ?1 AND kind = ?2 AND language = ?3")?
                .query_row(params![display_name, kind, language], |row| row.get(0))
                .unwrap_or_else(|_| {
                    tx.execute(
                        "INSERT INTO symbols (name, kind, language) VALUES (?1, ?2, ?3)",
                        params![display_name, kind, language],
                    )
                    .unwrap();
                    tx.last_insert_rowid()
                });

            // Insert occurrence with 1-based positions
            tx.execute(
                "INSERT INTO occurrences \
                 (symbol_id, symbol, file_path, start_line, start_column, end_line, end_column, role, language, file_hash) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    symbol_id,
                    occurrence.symbol,
                    path,
                    (range.0 + 1) as u32,
                    (range.1 + 1) as u32,
                    (range.2 + 1) as u32,
                    (range.3 + 1) as u32,
                    role,
                    language,
                    "", // file_hash is not in SCIP proto; verify from workspace instead
                ],
            )?;

            inserted_occurrences += 1;
        }
        inserted_symbols += document.symbols.len();
    }

    // Store snapshot hash for freshness tracking
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params!["snapshot_hash", snapshot],
    )?;

    tx.commit()?;

    log_index_meta(db_path, snapshot, inserted_symbols, inserted_occurrences)?;

    Ok(())
}

/// Check if the occurrence DB is fresh for the given snapshot hash.
pub fn occurrence_db_fresh(db_path: &Path, snapshot: &str) -> bool {
    if !db_path.exists() {
        return false;
    }
    match check_snapshot_hash(db_path, snapshot) {
        Ok(true) => true,
        _ => false,
    }
}

/// Delete the occurrence DB (force rebuild).
pub fn invalidate_db(db_path: &Path) -> Result<()> {
    if db_path.exists() {
        std::fs::remove_file(db_path)?;
    }
    Ok(())
}

/// Query definitions for a given identifier name.
pub fn query_defs(db_path: &Path, identifier: &str) -> Result<Vec<OccurrenceResult>> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT o.file_path, o.language, o.symbol, s.name, s.kind, o.role, \
                o.start_line, o.start_column, o.end_line, o.end_column, o.file_hash \
         FROM occurrences o \
         JOIN symbols s ON o.symbol_id = s.id \
         WHERE o.role = 'definition' AND s.name = ?1 \
         ORDER BY o.file_path, o.start_line",
    )?;

    let results = stmt
        .query_map(params![identifier], map_occurrence_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Query references for a given identifier name.
pub fn query_refs(db_path: &Path, identifier: &str) -> Result<Vec<OccurrenceResult>> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT o.file_path, o.language, o.symbol, s.name, s.kind, o.role, \
                o.start_line, o.start_column, o.end_line, o.end_column, o.file_hash \
         FROM occurrences o \
         JOIN symbols s ON o.symbol_id = s.id \
         WHERE o.role = 'reference' AND s.name = ?1 \
         ORDER BY o.file_path, o.start_line",
    )?;

    let results = stmt
        .query_map(params![identifier], map_occurrence_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Query symbols with name containing the query string.
pub fn query_symbols(db_path: &Path, query: &str) -> Result<Vec<SymbolResult>> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare(
        "SELECT DISTINCT s.name, s.kind, s.language, \
                o.file_path, o.role, o.start_line, o.start_column, o.end_line, o.end_column \
         FROM symbols s \
         JOIN occurrences o ON o.symbol_id = s.id \
         WHERE o.role = 'definition' AND s.name LIKE ?1 \
         ORDER BY s.kind, s.name, o.file_path",
    )?;

    let like_pattern = format!("%{}%", query);
    let results = stmt
        .query_map(params![like_pattern], |row| {
            Ok(SymbolResult {
                name: row.get(0)?,
                kind: row.get(1)?,
                language: row.get(2)?,
                path: row.get(3)?,
                role: row.get(4)?,
                start_line: row.get(5)?,
                start_column: row.get(6)?,
                end_line: row.get(7)?,
                end_column: row.get(8)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(results)
}

/// Convert an OccurrenceResult to JSON.
pub fn occurrence_to_json(result: &OccurrenceResult) -> Value {
    json!({
        "path": result.path,
        "name": result.name,
        "kind": result.kind,
        "symbol": result.symbol,
        "role": result.role,
        "language": result.language,
        "range": {
            "start": { "line": result.start_line, "column": result.start_column },
            "end": { "line": result.end_line, "column": result.end_column }
        },
        "fileHash": result.file_hash,
        "producer": "scip",
        "reliability": "precise_fact",
        "exact": true
    })
}

/// Convert a SymbolResult to JSON.
pub fn symbol_to_json(result: &SymbolResult) -> Value {
    json!({
        "name": result.name,
        "kind": result.kind,
        "language": result.language,
        "path": result.path,
        "role": result.role,
        "range": {
            "start": { "line": result.start_line, "column": result.start_column },
            "end": { "line": result.end_line, "column": result.end_column }
        },
        "producer": "scip",
        "reliability": "precise_fact",
        "exact": true
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS symbols (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT '',
            language TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE IF NOT EXISTS occurrences (
            id INTEGER PRIMARY KEY,
            symbol_id INTEGER NOT NULL REFERENCES symbols(id),
            symbol TEXT NOT NULL,
            file_path TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            start_column INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            end_column INTEGER NOT NULL,
            role TEXT NOT NULL,
            language TEXT NOT NULL DEFAULT '',
            file_hash TEXT NOT NULL DEFAULT ''
        );

        CREATE INDEX IF NOT EXISTS idx_occurrences_symbol_id ON occurrences(symbol_id);
        CREATE INDEX IF NOT EXISTS idx_occurrences_role ON occurrences(role);
        CREATE INDEX IF NOT EXISTS idx_occurrences_symbol ON occurrences(symbol);
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);

        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    Ok(())
}

fn check_snapshot_hash(db_path: &Path, snapshot: &str) -> Result<bool> {
    let conn = Connection::open(db_path)?;
    let stored: String = conn.query_row(
        "SELECT value FROM meta WHERE key = 'snapshot_hash'",
        [],
        |row| row.get(0),
    )?;
    Ok(stored == snapshot)
}

fn log_index_meta(
    db_path: &Path,
    snapshot: &str,
    symbol_count: usize,
    occurrence_count: usize,
) -> Result<()> {
    let manifest_path = db_path.with_file_name("manifest.json");
    let value = json!({
        "source": "scip_native",
        "snapshot": snapshot,
        "dbPath": db_path.to_string_lossy(),
        "symbolCount": symbol_count,
        "occurrenceCount": occurrence_count,
    });
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

fn map_occurrence_row(
    row: &rusqlite::Row<'_>,
) -> std::result::Result<OccurrenceResult, rusqlite::Error> {
    Ok(OccurrenceResult {
        path: row.get(0)?,
        language: row.get(1)?,
        symbol: row.get(2)?,
        name: row.get(3)?,
        kind: row.get(4)?,
        role: row.get(5)?,
        start_line: row.get(6)?,
        start_column: row.get(7)?,
        end_line: row.get(8)?,
        end_column: row.get(9)?,
        file_hash: row.get(10)?,
    })
}

fn kind_name(sym: &proto::SymbolInformation) -> &str {
    match proto::symbol_information::Kind::try_from(sym.kind) {
        Ok(proto::symbol_information::Kind::Function) => "function",
        Ok(proto::symbol_information::Kind::Method) => "method",
        Ok(proto::symbol_information::Kind::Struct) => "struct",
        Ok(proto::symbol_information::Kind::Class) => "class",
        Ok(proto::symbol_information::Kind::Interface) => "interface",
        Ok(proto::symbol_information::Kind::Enum) => "enum",
        Ok(proto::symbol_information::Kind::Trait) => "trait",
        Ok(proto::symbol_information::Kind::TypeAlias) => "type_alias",
        Ok(proto::symbol_information::Kind::Module) => "module",
        Ok(proto::symbol_information::Kind::Constant) => "constant",
        Ok(proto::symbol_information::Kind::Variable) => "variable",
        Ok(proto::symbol_information::Kind::Field) => "field",
        Ok(proto::symbol_information::Kind::TypeParameter) => "type_parameter",
        Ok(proto::symbol_information::Kind::Parameter) => "parameter",
        Ok(proto::symbol_information::Kind::Property) => "property",
        Ok(proto::symbol_information::Kind::Constructor) => "constructor",
        _ => "symbol",
    }
}

fn display_name_from_symbol(symbol: &str) -> String {
    symbol
        .split(|ch: char| ch == '/' || ch == '#' || ch == '.' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .next_back()
        .unwrap_or(symbol)
        .trim_end_matches("().")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scip_proto::proto;
    use tempfile::tempdir;

    fn build_test_index() -> proto::Index {
        proto::Index {
            metadata: Some(proto::Metadata {
                version: proto::ProtocolVersion::UnspecifiedProtocolVersion as i32,
                tool_info: Some(proto::ToolInfo {
                    name: "test".to_string(),
                    version: "0.1.0".to_string(),
                    arguments: vec![],
                }),
                project_root: "file:///test".to_string(),
                text_document_encoding: proto::TextEncoding::Utf8 as i32,
            }),
            documents: vec![proto::Document {
                language: "rust".to_string(),
                relative_path: "src/lib.rs".to_string(),
                occurrences: vec![
                    proto::Occurrence {
                        range: vec![0, 3, 0, 9],
                        symbol: "local 1".to_string(),
                        symbol_roles: 1,
                        ..Default::default()
                    },
                    proto::Occurrence {
                        range: vec![1, 12, 1, 18],
                        symbol: "local 1".to_string(),
                        symbol_roles: 0,
                        ..Default::default()
                    },
                ],
                symbols: vec![proto::SymbolInformation {
                    symbol: "local 1".to_string(),
                    kind: proto::symbol_information::Kind::Function as i32,
                    display_name: "needle".to_string(),
                    ..Default::default()
                }],
                position_encoding: proto::PositionEncoding::Utf8CodeUnitOffsetFromLineStart as i32,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn build_and_query_full_cycle() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("occurrences.db");

        let index = build_test_index();
        build_occurrences_db(&index, &db_path, "snapshot-v1").unwrap();

        assert!(occurrence_db_fresh(&db_path, "snapshot-v1"));
        assert!(!occurrence_db_fresh(&db_path, "snapshot-v2"));

        // defs
        let defs = query_defs(&db_path, "needle").unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "needle");
        assert_eq!(defs[0].role, "definition");
        assert_eq!(defs[0].path, "src/lib.rs");
        assert_eq!(defs[0].start_line, 1);
        assert_eq!(defs[0].start_column, 4);

        // refs
        let refs = query_refs(&db_path, "needle").unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].role, "reference");
        assert_eq!(refs[0].start_line, 2);

        // symbols
        let symbols = query_symbols(&db_path, "needle").unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "needle");

        // unknown identifier
        let no_defs = query_defs(&db_path, "nonexistent").unwrap();
        assert!(no_defs.is_empty());

        // JSON output
        let json = occurrence_to_json(&defs[0]);
        assert_eq!(json["reliability"], "precise_fact");
        assert_eq!(json["exact"], true);
        assert_eq!(json["producer"], "scip");
    }

    #[test]
    fn freshness_detects_hash_mismatch() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("occurrences.db");

        let index = build_test_index();
        build_occurrences_db(&index, &db_path, "commit:abc123").unwrap();

        assert!(occurrence_db_fresh(&db_path, "commit:abc123"));
        assert!(!occurrence_db_fresh(&db_path, "worktree:abc123"));

        let nonexistent = dir.path().join("nonexistent.db");
        assert!(!occurrence_db_fresh(&nonexistent, "any"));
    }

    #[test]
    fn defs_returns_empty_for_missing_db() {
        let dir = tempdir().unwrap();
        let nonexistent = dir.path().join("no-such.db");
        assert!(query_defs(&nonexistent, "anything").unwrap().is_empty());
        assert!(query_refs(&nonexistent, "anything").unwrap().is_empty());
        assert!(query_symbols(&nonexistent, "anything").unwrap().is_empty());
    }
}

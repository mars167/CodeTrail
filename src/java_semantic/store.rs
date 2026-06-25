use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql};
use serde_json::{json, Value};

use crate::{
    java_semantic::{
        hierarchy::CallHierarchyOptions,
        model::{
            DispatchKind, JavaSemanticData, JavaSymbolKind, ResolveConfidence, ResolveStatus,
            SourceRange, SymbolOrigin,
        },
    },
    query_input::{attach_matched_input, style_key, InputPlan, InputVariant, SymbolMatchMode},
    workspace::{ScanOptions, Workspace},
};

const SCHEMA_VERSION: u32 = 1;
const PRODUCER: &str = "java_semantic_resolver";

pub fn db_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".codetrail")
        .join("java-semantic.sqlite")
}

pub struct JavaSemanticStore {
    conn: Connection,
    path: PathBuf,
}

#[derive(Clone, Debug)]
struct MatchedSymbol {
    symbol: SymbolRow,
    variant: InputVariant,
}

#[derive(Clone, Debug)]
struct SymbolRow {
    symbol_id: String,
    name: String,
    public_kind: String,
    qualified_name: String,
    path: Option<String>,
    range: Option<SourceRange>,
    selection_range: Option<SourceRange>,
    signature: String,
    root_id: String,
}

#[derive(Clone, Debug)]
struct CallEdgeRow {
    edge_id: i64,
    caller_symbol: String,
    callee_symbol: Option<String>,
    target_name: String,
    path: String,
    range: SourceRange,
    file_hash: String,
    dispatch_kind: String,
    status: String,
    confidence: String,
    caller: Option<SymbolRow>,
    callee: Option<SymbolRow>,
    variant: Option<InputVariant>,
}

impl JavaSemanticStore {
    pub fn open_or_create(workspace_root: &Path) -> Result<Self> {
        let path = db_path(workspace_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("failed to open Java semantic DB {}", path.display()))?;
        let store = Self { conn, path };
        store.configure()?;
        store.ensure_schema()?;
        Ok(store)
    }

    pub fn open_existing(workspace_root: &Path) -> Result<Option<Self>> {
        let path = db_path(workspace_root);
        if !path.exists() {
            return Ok(None);
        }
        Self::open_or_create(workspace_root).map(Some)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_snapshot(
        &mut self,
        data: &JavaSemanticData,
        classpath_symbol_count: usize,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM java_semantic_snapshots WHERE snapshot_id = ?1",
            params![data.manifest.snapshot_id],
        )?;
        for table in [
            "symbols",
            "occurrences",
            "call_edges",
            "possible_callees",
            "type_edges",
            "file_contributions",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE snapshot_id = ?1"),
                params![data.manifest.snapshot_id],
            )?;
        }

        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default();
        tx.execute(
            "INSERT INTO java_semantic_snapshots \
             (snapshot_id, snapshot_key, schema_version, tool_version, source, file_count, \
              symbol_count, occurrence_count, call_edge_count, type_edge_count, \
              classpath_symbol_count, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                data.manifest.snapshot_id,
                data.manifest.snapshot_key,
                data.manifest.schema_version as i64,
                data.manifest.tool_version,
                data.manifest.source,
                data.manifest.file_count as i64,
                data.manifest.symbol_count as i64,
                data.manifest.occurrence_count as i64,
                data.manifest.call_edge_count as i64,
                data.manifest.type_edge_count as i64,
                classpath_symbol_count as i64,
                created_at_ms,
            ],
        )?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO symbols \
                 (snapshot_id, symbol_id, symbol_id_lc, symbol_id_style, name, name_lc, name_style, \
                  kind, public_kind, package, qualified_name, qualified_name_lc, qualified_name_style, \
                  signature, signature_lc, signature_style, owner_symbol, path, range_start_line, \
                  range_start_col, range_end_line, range_end_col, selection_start_line, \
                  selection_start_col, selection_end_line, selection_end_col, descriptor, return_type, \
                  origin, confidence, root_id, file_hash) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
                         ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32)",
            )?;
            for symbol in &data.symbols {
                let signature = symbol.display_signature();
                let range = symbol.range.as_ref();
                let selection = symbol.selection_range.as_ref();
                stmt.execute(params![
                    data.manifest.snapshot_id,
                    symbol.symbol_id,
                    symbol.symbol_id.to_lowercase(),
                    style_key(&symbol.symbol_id),
                    symbol.name,
                    symbol.name.to_lowercase(),
                    style_key(&symbol.name),
                    symbol_kind_code(symbol.kind),
                    symbol.kind.public_kind(),
                    symbol.package,
                    symbol.qualified_name,
                    symbol.qualified_name.to_lowercase(),
                    style_key(&symbol.qualified_name),
                    signature,
                    signature.to_lowercase(),
                    style_key(&signature),
                    symbol.owner_symbol,
                    symbol.path,
                    range.map(|range| range.start_line as i64),
                    range.map(|range| range.start_column as i64),
                    range.map(|range| range.end_line as i64),
                    range.map(|range| range.end_column as i64),
                    selection.map(|range| range.start_line as i64),
                    selection.map(|range| range.start_column as i64),
                    selection.map(|range| range.end_line as i64),
                    selection.map(|range| range.end_column as i64),
                    symbol.descriptor,
                    symbol.return_type,
                    symbol_origin_code(symbol.origin),
                    confidence_code(symbol.confidence),
                    symbol.root_id,
                    symbol.file_hash,
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO occurrences \
                 (snapshot_id, path, range_start_line, range_start_col, range_end_line, range_end_col, \
                  role, symbol_id, enclosing_symbol, syntax_kind, source, confidence) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?;
            for occurrence in &data.occurrences {
                stmt.execute(params![
                    data.manifest.snapshot_id,
                    occurrence.path,
                    occurrence.range.start_line as i64,
                    occurrence.range.start_column as i64,
                    occurrence.range.end_line as i64,
                    occurrence.range.end_column as i64,
                    format!("{:?}", occurrence.role),
                    occurrence.symbol_id,
                    occurrence.enclosing_symbol,
                    occurrence.syntax_kind,
                    occurrence.source,
                    confidence_code(occurrence.confidence),
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO call_edges \
                 (snapshot_id, edge_id, caller_symbol, callee_symbol, target_name, target_name_lc, \
                  target_name_style, path, range_start_line, range_start_col, range_end_line, \
                  range_end_col, file_hash, dispatch_kind, receiver_type, status, confidence) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            )?;
            let mut possible_stmt = tx.prepare(
                "INSERT OR IGNORE INTO possible_callees (snapshot_id, edge_id, callee_symbol) VALUES (?1, ?2, ?3)",
            )?;
            for (idx, edge) in data.call_edges.iter().enumerate() {
                let edge_id = idx as i64 + 1;
                stmt.execute(params![
                    data.manifest.snapshot_id,
                    edge_id,
                    edge.caller_symbol,
                    edge.callee_symbol,
                    edge.target_name,
                    edge.target_name.to_lowercase(),
                    style_key(&edge.target_name),
                    edge.path,
                    edge.range.start_line as i64,
                    edge.range.start_column as i64,
                    edge.range.end_line as i64,
                    edge.range.end_column as i64,
                    edge.file_hash,
                    dispatch_kind_code(edge.dispatch_kind),
                    edge.receiver_type,
                    status_code(edge.status),
                    confidence_code(edge.confidence),
                ])?;
                let mut possible = edge.possible_callees.clone();
                if let Some(callee) = &edge.callee_symbol {
                    possible.push(callee.clone());
                }
                possible.sort();
                possible.dedup();
                for callee in possible {
                    possible_stmt.execute(params![data.manifest.snapshot_id, edge_id, callee])?;
                }
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO type_edges (snapshot_id, subtype, supertype, supertype_lc, relation) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for edge in &data.type_edges {
                stmt.execute(params![
                    data.manifest.snapshot_id,
                    edge.subtype,
                    edge.supertype,
                    edge.supertype.to_lowercase(),
                    edge.relation,
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO file_contributions \
                 (snapshot_id, path, file_hash, symbol_count, occurrence_count, call_edge_count) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for contribution in &data.file_contributions {
                stmt.execute(params![
                    data.manifest.snapshot_id,
                    contribution.path,
                    contribution.file_hash,
                    contribution.symbol_count as i64,
                    contribution.occurrence_count as i64,
                    contribution.call_edge_count as i64,
                ])?;
            }
        }

        tx.execute(
            "INSERT INTO java_semantic_active (id, snapshot_id) VALUES (1, ?1) \
             ON CONFLICT(id) DO UPDATE SET snapshot_id = excluded.snapshot_id",
            params![data.manifest.snapshot_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn is_fresh(&self, snapshot_id: &str) -> Result<bool> {
        let active: Option<String> = self
            .conn
            .query_row(
                "SELECT snapshot_id FROM java_semantic_active WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if active.as_deref() != Some(snapshot_id) {
            return Ok(false);
        }
        let row: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM java_semantic_snapshots \
                 WHERE snapshot_id = ?1 AND schema_version = ?2",
                params![snapshot_id, SCHEMA_VERSION as i64],
                |row| row.get(0),
            )
            .optional()?;
        Ok(row.is_some())
    }

    pub fn calls(
        &mut self,
        workspace: &Workspace,
        opts: &ScanOptions,
        identifier: &str,
    ) -> Result<Option<(Value, Value)>> {
        if !self.is_fresh(&workspace.snapshot_id)? {
            return Ok(None);
        }
        let plan = InputPlan::new(identifier, opts.input_mode);
        let has_scope = self.install_allowed_paths(workspace, opts)?;
        let symbols = self.find_symbols(
            &workspace.snapshot_id,
            &plan,
            opts.case_sensitive,
            &["method", "constructor", "synthetic_method"],
            has_scope,
            opts.limit,
        )?;
        if symbols.is_empty() {
            return Ok(None);
        }
        self.install_matched_symbols(&symbols)?;
        let mut results =
            self.query_call_edges_from_matched_callers(&workspace.snapshot_id, has_scope)?;
        finalize_results(&mut results, opts.limit);
        if results.is_empty() {
            return Ok(None);
        }
        Ok(Some((
            index_meta_for_path(self.path(), &workspace.snapshot_id),
            Value::Array(results),
        )))
    }

    pub fn callers(
        &mut self,
        workspace: &Workspace,
        opts: &ScanOptions,
        identifier: &str,
    ) -> Result<Option<(Value, Value)>> {
        if !self.is_fresh(&workspace.snapshot_id)? {
            return Ok(None);
        }
        let plan = InputPlan::new(identifier, opts.input_mode);
        let has_scope = self.install_allowed_paths(workspace, opts)?;
        let symbols = self.find_symbols(
            &workspace.snapshot_id,
            &plan,
            opts.case_sensitive,
            &["method", "constructor", "synthetic_method"],
            has_scope,
            opts.limit,
        )?;
        self.install_matched_symbols(&symbols)?;
        let mut results =
            self.query_call_edges_to_matched_callees(&workspace.snapshot_id, has_scope)?;
        results.extend(self.query_call_edges_by_target_name(
            &workspace.snapshot_id,
            &plan,
            opts.case_sensitive,
            has_scope,
        )?);
        finalize_results(&mut results, opts.limit);
        if results.is_empty() {
            return Ok(None);
        }
        Ok(Some((
            index_meta_for_path(self.path(), &workspace.snapshot_id),
            Value::Array(results),
        )))
    }

    pub fn call_hierarchy(
        &mut self,
        workspace: &Workspace,
        opts: &ScanOptions,
        identifier: &str,
        options: CallHierarchyOptions,
    ) -> Result<Option<(Value, Value)>> {
        if !self.is_fresh(&workspace.snapshot_id)? {
            return Ok(None);
        }
        let plan = InputPlan::new(identifier, opts.input_mode);
        let has_scope = self.install_allowed_paths(workspace, opts)?;
        let mut roots = self
            .find_symbols(
                &workspace.snapshot_id,
                &plan,
                opts.case_sensitive,
                &["method", "constructor", "synthetic_method"],
                has_scope,
                opts.limit,
            )?
            .into_iter()
            .map(|matched| matched.symbol)
            .collect::<Vec<_>>();
        roots.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        roots.dedup_by(|a, b| a.symbol_id == b.symbol_id);
        if opts.limit > 0 && roots.len() > opts.limit {
            roots.truncate(opts.limit);
        }
        if roots.is_empty() {
            return Ok(None);
        }

        let mut values = Vec::new();
        for root in roots {
            let mut value = json!({
                "root": symbol_item_json(&root),
                "incomingCalls": [],
                "outgoingCalls": [],
            });
            if options.direction.include_incoming() {
                value["incomingCalls"] = Value::Array(self.expand_incoming(
                    &workspace.snapshot_id,
                    &root.symbol_id,
                    options.depth.max(1),
                    opts.limit,
                    options.include_overrides,
                    has_scope,
                    &mut BTreeSet::new(),
                )?);
            }
            if options.direction.include_outgoing() {
                value["outgoingCalls"] = Value::Array(self.expand_outgoing(
                    &workspace.snapshot_id,
                    &root.symbol_id,
                    options.depth.max(1),
                    opts.limit,
                    options.include_overrides,
                    has_scope,
                    &mut BTreeSet::new(),
                )?);
            }
            values.push(value);
            if opts.limit > 0 && values.len() >= opts.limit {
                break;
            }
        }

        Ok(Some((
            index_meta_for_path(self.path(), &workspace.snapshot_id),
            Value::Array(values),
        )))
    }

    fn configure(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;",
        )?;
        Ok(())
    }

    fn ensure_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS java_semantic_snapshots (
                snapshot_id TEXT PRIMARY KEY,
                snapshot_key TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                tool_version TEXT NOT NULL,
                source TEXT NOT NULL,
                file_count INTEGER NOT NULL,
                symbol_count INTEGER NOT NULL,
                occurrence_count INTEGER NOT NULL,
                call_edge_count INTEGER NOT NULL,
                type_edge_count INTEGER NOT NULL,
                classpath_symbol_count INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS java_semantic_active (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                snapshot_id TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS symbols (
                snapshot_id TEXT NOT NULL,
                symbol_id TEXT NOT NULL,
                symbol_id_lc TEXT NOT NULL,
                symbol_id_style TEXT NOT NULL,
                name TEXT NOT NULL,
                name_lc TEXT NOT NULL,
                name_style TEXT NOT NULL,
                kind TEXT NOT NULL,
                public_kind TEXT NOT NULL,
                package TEXT NOT NULL,
                qualified_name TEXT NOT NULL,
                qualified_name_lc TEXT NOT NULL,
                qualified_name_style TEXT NOT NULL,
                signature TEXT NOT NULL,
                signature_lc TEXT NOT NULL,
                signature_style TEXT NOT NULL,
                owner_symbol TEXT,
                path TEXT,
                range_start_line INTEGER,
                range_start_col INTEGER,
                range_end_line INTEGER,
                range_end_col INTEGER,
                selection_start_line INTEGER,
                selection_start_col INTEGER,
                selection_end_line INTEGER,
                selection_end_col INTEGER,
                descriptor TEXT,
                return_type TEXT,
                origin TEXT NOT NULL,
                confidence TEXT NOT NULL,
                root_id TEXT NOT NULL,
                file_hash TEXT NOT NULL,
                PRIMARY KEY(snapshot_id, symbol_id)
            );

            CREATE TABLE IF NOT EXISTS occurrences (
                snapshot_id TEXT NOT NULL,
                path TEXT NOT NULL,
                range_start_line INTEGER NOT NULL,
                range_start_col INTEGER NOT NULL,
                range_end_line INTEGER NOT NULL,
                range_end_col INTEGER NOT NULL,
                role TEXT NOT NULL,
                symbol_id TEXT NOT NULL,
                enclosing_symbol TEXT,
                syntax_kind TEXT NOT NULL,
                source TEXT NOT NULL,
                confidence TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS call_edges (
                snapshot_id TEXT NOT NULL,
                edge_id INTEGER NOT NULL,
                caller_symbol TEXT NOT NULL,
                callee_symbol TEXT,
                target_name TEXT NOT NULL,
                target_name_lc TEXT NOT NULL,
                target_name_style TEXT NOT NULL,
                path TEXT NOT NULL,
                range_start_line INTEGER NOT NULL,
                range_start_col INTEGER NOT NULL,
                range_end_line INTEGER NOT NULL,
                range_end_col INTEGER NOT NULL,
                file_hash TEXT NOT NULL,
                dispatch_kind TEXT NOT NULL,
                receiver_type TEXT,
                status TEXT NOT NULL,
                confidence TEXT NOT NULL,
                PRIMARY KEY(snapshot_id, edge_id)
            );

            CREATE TABLE IF NOT EXISTS possible_callees (
                snapshot_id TEXT NOT NULL,
                edge_id INTEGER NOT NULL,
                callee_symbol TEXT NOT NULL,
                PRIMARY KEY(snapshot_id, edge_id, callee_symbol)
            );

            CREATE TABLE IF NOT EXISTS type_edges (
                snapshot_id TEXT NOT NULL,
                subtype TEXT NOT NULL,
                supertype TEXT NOT NULL,
                supertype_lc TEXT NOT NULL,
                relation TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS file_contributions (
                snapshot_id TEXT NOT NULL,
                path TEXT NOT NULL,
                file_hash TEXT NOT NULL,
                symbol_count INTEGER NOT NULL,
                occurrence_count INTEGER NOT NULL,
                call_edge_count INTEGER NOT NULL,
                PRIMARY KEY(snapshot_id, path)
            );

            CREATE INDEX IF NOT EXISTS symbols_name_idx ON symbols(snapshot_id, name);
            CREATE INDEX IF NOT EXISTS symbols_name_lc_idx ON symbols(snapshot_id, name_lc);
            CREATE INDEX IF NOT EXISTS symbols_name_style_idx ON symbols(snapshot_id, name_style);
            CREATE INDEX IF NOT EXISTS symbols_qualified_idx ON symbols(snapshot_id, qualified_name);
            CREATE INDEX IF NOT EXISTS symbols_qualified_lc_idx ON symbols(snapshot_id, qualified_name_lc);
            CREATE INDEX IF NOT EXISTS symbols_signature_idx ON symbols(snapshot_id, signature);
            CREATE INDEX IF NOT EXISTS symbols_signature_lc_idx ON symbols(snapshot_id, signature_lc);
            CREATE INDEX IF NOT EXISTS symbols_path_idx ON symbols(snapshot_id, path);
            CREATE INDEX IF NOT EXISTS call_edges_caller_idx ON call_edges(snapshot_id, caller_symbol);
            CREATE INDEX IF NOT EXISTS call_edges_callee_idx ON call_edges(snapshot_id, callee_symbol);
            CREATE INDEX IF NOT EXISTS call_edges_target_idx ON call_edges(snapshot_id, target_name);
            CREATE INDEX IF NOT EXISTS call_edges_target_lc_idx ON call_edges(snapshot_id, target_name_lc);
            CREATE INDEX IF NOT EXISTS call_edges_target_style_idx ON call_edges(snapshot_id, target_name_style);
            CREATE INDEX IF NOT EXISTS call_edges_path_idx ON call_edges(snapshot_id, path);
            CREATE INDEX IF NOT EXISTS possible_callees_callee_idx ON possible_callees(snapshot_id, callee_symbol);
            CREATE INDEX IF NOT EXISTS possible_callees_edge_idx ON possible_callees(snapshot_id, edge_id);
            CREATE INDEX IF NOT EXISTS type_edges_super_idx ON type_edges(snapshot_id, supertype);
            CREATE INDEX IF NOT EXISTS type_edges_super_lc_idx ON type_edges(snapshot_id, supertype_lc);",
        )?;
        Ok(())
    }

    fn install_allowed_paths(&self, workspace: &Workspace, opts: &ScanOptions) -> Result<bool> {
        self.conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS java_semantic_allowed_paths(path TEXT PRIMARY KEY);
             DELETE FROM java_semantic_allowed_paths;",
        )?;
        if !scope_restricts_paths(opts) {
            return Ok(false);
        }
        let mut scope_opts = opts.clone();
        scope_opts.limit = 0;
        let records = workspace.scan_catalog(&scope_opts)?;
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt =
                tx.prepare("INSERT OR IGNORE INTO java_semantic_allowed_paths(path) VALUES (?1)")?;
            for record in records {
                stmt.execute(params![record.path])?;
            }
        }
        tx.commit()?;
        Ok(true)
    }

    fn find_symbols(
        &self,
        snapshot_id: &str,
        plan: &InputPlan,
        case_sensitive: bool,
        kinds: &[&str],
        has_scope: bool,
        limit: usize,
    ) -> Result<Vec<MatchedSymbol>> {
        let mut matched = BTreeMap::<String, MatchedSymbol>::new();
        let kind_clause = sql_in_literals(kinds);
        let query_limit = if limit > 0 {
            (limit.saturating_mul(16)).max(128)
        } else {
            10_000
        } as i64;

        for variant in &plan.variants {
            let (columns, value) = match_columns_for_variant(
                variant,
                case_sensitive || plan.mode == crate::query_input::InputMode::Strict,
            );
            let where_clause = columns
                .iter()
                .map(|column| format!("s.{column} = ?2"))
                .collect::<Vec<_>>()
                .join(" OR ");
            let sql = format!(
                "SELECT {} FROM symbols s \
                 WHERE s.snapshot_id = ?1 AND s.kind IN ({kind_clause}) AND ({where_clause}) \
                   AND (?3 = 0 OR s.path IS NULL OR s.path IN (SELECT path FROM java_semantic_allowed_paths)) \
                 ORDER BY s.qualified_name, s.symbol_id LIMIT ?4",
                symbol_select_columns("s")
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt
                .query_map(
                    params![snapshot_id, value, has_scope_i64(has_scope), query_limit],
                    |row| map_symbol_row(row, 0),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for symbol in rows {
                let Some(actual_variant) = matched_symbol_variant(&symbol, plan, case_sensitive)
                else {
                    continue;
                };
                matched
                    .entry(symbol.symbol_id.clone())
                    .or_insert_with(|| MatchedSymbol {
                        symbol,
                        variant: actual_variant.clone(),
                    });
                if limit > 0 && matched.len() >= query_limit as usize {
                    break;
                }
            }
        }
        Ok(matched.into_values().collect())
    }

    fn install_matched_symbols(&self, symbols: &[MatchedSymbol]) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS java_semantic_matched_symbols(
                symbol_id TEXT PRIMARY KEY,
                variant_kind TEXT NOT NULL,
                variant_value TEXT NOT NULL
             );
             DELETE FROM java_semantic_matched_symbols;",
        )?;
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO java_semantic_matched_symbols(symbol_id, variant_kind, variant_value) \
                 VALUES (?1, ?2, ?3)",
            )?;
            for symbol in symbols {
                stmt.execute(params![
                    symbol.symbol.symbol_id,
                    symbol.variant.kind,
                    symbol.variant.value,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn query_call_edges_from_matched_callers(
        &self,
        snapshot_id: &str,
        has_scope: bool,
    ) -> Result<Vec<Value>> {
        let sql = format!(
            "SELECT {}, m.variant_kind, m.variant_value \
             FROM call_edges e \
             JOIN java_semantic_matched_symbols m ON m.symbol_id = e.caller_symbol \
             LEFT JOIN symbols caller ON caller.snapshot_id = e.snapshot_id AND caller.symbol_id = e.caller_symbol \
             LEFT JOIN symbols callee ON callee.snapshot_id = e.snapshot_id AND callee.symbol_id = e.callee_symbol \
             WHERE e.snapshot_id = ?1 AND (?2 = 0 OR e.path IN (SELECT path FROM java_semantic_allowed_paths)) \
             ORDER BY e.path, e.range_start_line, e.range_start_col",
            call_edge_select_columns()
        );
        self.query_call_values(&sql, params![snapshot_id, has_scope_i64(has_scope)])
    }

    fn query_call_edges_to_matched_callees(
        &self,
        snapshot_id: &str,
        has_scope: bool,
    ) -> Result<Vec<Value>> {
        let sql = format!(
            "SELECT {}, m.variant_kind, m.variant_value \
             FROM call_edges e \
             JOIN java_semantic_matched_symbols m ON m.symbol_id = e.callee_symbol \
             LEFT JOIN symbols caller ON caller.snapshot_id = e.snapshot_id AND caller.symbol_id = e.caller_symbol \
             LEFT JOIN symbols callee ON callee.snapshot_id = e.snapshot_id AND callee.symbol_id = e.callee_symbol \
             WHERE e.snapshot_id = ?1 AND (?2 = 0 OR e.path IN (SELECT path FROM java_semantic_allowed_paths)) \
             ORDER BY e.path, e.range_start_line, e.range_start_col",
            call_edge_select_columns()
        );
        self.query_call_values(&sql, params![snapshot_id, has_scope_i64(has_scope)])
    }

    fn query_call_edges_by_target_name(
        &self,
        snapshot_id: &str,
        plan: &InputPlan,
        case_sensitive: bool,
        has_scope: bool,
    ) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        for variant in &plan.variants {
            let (columns, value) = target_columns_for_variant(
                variant,
                case_sensitive || plan.mode == crate::query_input::InputMode::Strict,
            );
            let where_clause = columns
                .iter()
                .map(|column| format!("e.{column} = ?2"))
                .collect::<Vec<_>>()
                .join(" OR ");
            let sql = format!(
                "SELECT {}, ?4 AS variant_kind, ?5 AS variant_value \
                 FROM call_edges e \
                 LEFT JOIN symbols caller ON caller.snapshot_id = e.snapshot_id AND caller.symbol_id = e.caller_symbol \
                 LEFT JOIN symbols callee ON callee.snapshot_id = e.snapshot_id AND callee.symbol_id = e.callee_symbol \
                 WHERE e.snapshot_id = ?1 AND ({where_clause}) \
                   AND (?3 = 0 OR e.path IN (SELECT path FROM java_semantic_allowed_paths)) \
                 ORDER BY e.path, e.range_start_line, e.range_start_col",
                call_edge_select_columns()
            );
            values.extend(self.query_call_values(
                &sql,
                params![
                    snapshot_id,
                    value,
                    has_scope_i64(has_scope),
                    variant.kind,
                    variant.value,
                ],
            )?);
        }
        Ok(values)
    }

    fn query_call_values<P>(&self, sql: &str, params: P) -> Result<Vec<Value>>
    where
        P: rusqlite::Params,
    {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(params, |row| {
                let edge = map_call_edge_row(row, 0)?;
                Ok(edge)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|edge| {
                let value = call_candidate_json(&edge);
                if let Some(variant) = &edge.variant {
                    attach_matched_input(value, variant)
                } else {
                    value
                }
            })
            .collect())
    }

    fn expand_incoming(
        &self,
        snapshot_id: &str,
        symbol_id: &str,
        depth: usize,
        limit: usize,
        include_overrides: bool,
        has_scope: bool,
        seen: &mut BTreeSet<String>,
    ) -> Result<Vec<Value>> {
        if depth == 0 || !seen.insert(format!("incoming:{symbol_id}")) {
            return Ok(Vec::new());
        }
        let edges = self.incoming_edges(snapshot_id, symbol_id, include_overrides, has_scope)?;
        let mut calls = Vec::new();
        for edge in edges {
            let Some(caller) = edge.caller.as_ref() else {
                continue;
            };
            let mut value = json!({
                "from": symbol_item_json(caller),
                "fromRanges": [edge.range.to_lsp_json()],
                "dispatchKind": edge.dispatch_kind.to_lowercase(),
            });
            if depth > 1 {
                value["children"] = Value::Array(self.expand_incoming(
                    snapshot_id,
                    &caller.symbol_id,
                    depth - 1,
                    limit,
                    include_overrides,
                    has_scope,
                    seen,
                )?);
            }
            calls.push(value);
            if limit > 0 && calls.len() >= limit {
                break;
            }
        }
        seen.remove(&format!("incoming:{symbol_id}"));
        Ok(calls)
    }

    fn expand_outgoing(
        &self,
        snapshot_id: &str,
        symbol_id: &str,
        depth: usize,
        limit: usize,
        include_overrides: bool,
        has_scope: bool,
        seen: &mut BTreeSet<String>,
    ) -> Result<Vec<Value>> {
        if depth == 0 || !seen.insert(format!("outgoing:{symbol_id}")) {
            return Ok(Vec::new());
        }
        let edges = self.outgoing_edges(snapshot_id, symbol_id, has_scope)?;
        let possible = if include_overrides {
            self.possible_callees_for_edges(snapshot_id, edges.iter().map(|edge| edge.edge_id))?
        } else {
            BTreeMap::new()
        };
        let mut calls = Vec::new();
        for edge in edges {
            let mut targets = edge.callee_symbol.clone().into_iter().collect::<Vec<_>>();
            if include_overrides {
                targets.extend(possible.get(&edge.edge_id).cloned().unwrap_or_default());
            }
            targets.sort();
            targets.dedup();
            for target in targets {
                let Some(callee) = self.symbol_by_id(snapshot_id, &target)? else {
                    continue;
                };
                let mut value = json!({
                    "to": symbol_item_json(&callee),
                    "fromRanges": [edge.range.to_lsp_json()],
                    "dispatchKind": edge.dispatch_kind.to_lowercase(),
                });
                if depth > 1 {
                    value["children"] = Value::Array(self.expand_outgoing(
                        snapshot_id,
                        &callee.symbol_id,
                        depth - 1,
                        limit,
                        include_overrides,
                        has_scope,
                        seen,
                    )?);
                }
                calls.push(value);
                if limit > 0 && calls.len() >= limit {
                    break;
                }
            }
            if limit > 0 && calls.len() >= limit {
                break;
            }
        }
        seen.remove(&format!("outgoing:{symbol_id}"));
        Ok(calls)
    }

    fn incoming_edges(
        &self,
        snapshot_id: &str,
        symbol_id: &str,
        include_overrides: bool,
        has_scope: bool,
    ) -> Result<Vec<CallEdgeRow>> {
        let predicate = if include_overrides {
            "EXISTS (
                SELECT 1 FROM possible_callees pc
                WHERE pc.snapshot_id = e.snapshot_id
                  AND pc.edge_id = e.edge_id
                  AND pc.callee_symbol = ?2
             )"
        } else {
            "e.callee_symbol = ?2"
        };
        let sql = format!(
            "SELECT {}, NULL AS variant_kind, NULL AS variant_value \
             FROM call_edges e \
             LEFT JOIN symbols caller ON caller.snapshot_id = e.snapshot_id AND caller.symbol_id = e.caller_symbol \
             LEFT JOIN symbols callee ON callee.snapshot_id = e.snapshot_id AND callee.symbol_id = e.callee_symbol \
             WHERE e.snapshot_id = ?1 AND {predicate} \
               AND (?3 = 0 OR e.path IN (SELECT path FROM java_semantic_allowed_paths)) \
             ORDER BY e.path, e.range_start_line, e.range_start_col",
            call_edge_select_columns()
        );
        let mut rows = self.query_call_edge_rows(
            &sql,
            params![snapshot_id, symbol_id, has_scope_i64(has_scope)],
        )?;
        dedup_edges(&mut rows);
        Ok(rows)
    }

    fn outgoing_edges(
        &self,
        snapshot_id: &str,
        symbol_id: &str,
        has_scope: bool,
    ) -> Result<Vec<CallEdgeRow>> {
        let sql = format!(
            "SELECT {}, NULL AS variant_kind, NULL AS variant_value \
             FROM call_edges e \
             LEFT JOIN symbols caller ON caller.snapshot_id = e.snapshot_id AND caller.symbol_id = e.caller_symbol \
             LEFT JOIN symbols callee ON callee.snapshot_id = e.snapshot_id AND callee.symbol_id = e.callee_symbol \
             WHERE e.snapshot_id = ?1 AND e.caller_symbol = ?2 \
               AND (?3 = 0 OR e.path IN (SELECT path FROM java_semantic_allowed_paths)) \
             ORDER BY e.path, e.range_start_line, e.range_start_col",
            call_edge_select_columns()
        );
        self.query_call_edge_rows(
            &sql,
            params![snapshot_id, symbol_id, has_scope_i64(has_scope)],
        )
    }

    fn query_call_edge_rows<P>(&self, sql: &str, params: P) -> Result<Vec<CallEdgeRow>>
    where
        P: rusqlite::Params,
    {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(params, |row| map_call_edge_row(row, 0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?;
        Ok(rows)
    }

    fn possible_callees_for_edges<I>(
        &self,
        snapshot_id: &str,
        edge_ids: I,
    ) -> Result<BTreeMap<i64, Vec<String>>>
    where
        I: IntoIterator<Item = i64>,
    {
        let edge_ids = edge_ids.into_iter().collect::<Vec<_>>();
        if edge_ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let placeholders = std::iter::repeat("?")
            .take(edge_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT edge_id, callee_symbol FROM possible_callees \
             WHERE snapshot_id = ? AND edge_id IN ({placeholders}) \
             ORDER BY edge_id, callee_symbol"
        );
        let mut params: Vec<&dyn ToSql> = Vec::with_capacity(edge_ids.len() + 1);
        params.push(&snapshot_id);
        for edge_id in &edge_ids {
            params.push(edge_id);
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let mut map = BTreeMap::<i64, Vec<String>>::new();
        for row in stmt.query_map(params_from_iter(params), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })? {
            let (edge_id, callee) = row?;
            map.entry(edge_id).or_default().push(callee);
        }
        Ok(map)
    }

    fn symbol_by_id(&self, snapshot_id: &str, symbol_id: &str) -> Result<Option<SymbolRow>> {
        let sql = format!(
            "SELECT {} FROM symbols s WHERE s.snapshot_id = ?1 AND s.symbol_id = ?2",
            symbol_select_columns("s")
        );
        self.conn
            .query_row(&sql, params![snapshot_id, symbol_id], |row| {
                map_symbol_row(row, 0)
            })
            .optional()
            .map_err(anyhow::Error::from)
    }
}

pub fn is_fresh(workspace: &Workspace) -> bool {
    JavaSemanticStore::open_existing(&workspace.root)
        .ok()
        .flatten()
        .and_then(|store| store.is_fresh(&workspace.snapshot_id).ok())
        .unwrap_or(false)
}

pub fn index_meta(workspace: &Workspace, fresh: bool) -> Value {
    json!({
        "used": true,
        "fresh": fresh,
        "source": "java_semantic",
        "fallback": false,
        "path": db_path(&workspace.root),
        "snapshot_id": workspace.snapshot_id,
    })
}

pub fn index_meta_for_path(path: &Path, snapshot_id: &str) -> Value {
    json!({
        "used": true,
        "fresh": true,
        "source": "java_semantic",
        "fallback": false,
        "path": path,
        "snapshot_id": snapshot_id,
    })
}

fn scope_restricts_paths(opts: &ScanOptions) -> bool {
    opts.changed
        || !opts.dirs.is_empty()
        || !opts.extensions.is_empty()
        || !opts.file_patterns.is_empty()
        || !opts.include.is_empty()
        || !opts.exclude.is_empty()
        || !opts.lang.is_empty()
}

fn match_columns_for_variant(
    variant: &InputVariant,
    case_sensitive: bool,
) -> (&'static [&'static str], String) {
    match variant.kind {
        "style_key" => (
            &[
                "symbol_id_style",
                "name_style",
                "qualified_name_style",
                "signature_style",
            ],
            variant.value.clone(),
        ),
        "case_fold" => (
            &[
                "symbol_id_lc",
                "name_lc",
                "qualified_name_lc",
                "signature_lc",
            ],
            variant.value.clone(),
        ),
        _ if case_sensitive => (
            &["symbol_id", "name", "qualified_name", "signature"],
            variant.value.clone(),
        ),
        _ => (
            &[
                "symbol_id_lc",
                "name_lc",
                "qualified_name_lc",
                "signature_lc",
            ],
            variant.value.to_lowercase(),
        ),
    }
}

fn target_columns_for_variant(
    variant: &InputVariant,
    case_sensitive: bool,
) -> (&'static [&'static str], String) {
    match variant.kind {
        "style_key" => (&["target_name_style"], variant.value.clone()),
        "case_fold" => (&["target_name_lc"], variant.value.clone()),
        _ if case_sensitive => (&["target_name"], variant.value.clone()),
        _ => (&["target_name_lc"], variant.value.to_lowercase()),
    }
}

fn matched_symbol_variant<'a>(
    symbol: &SymbolRow,
    plan: &'a InputPlan,
    case_sensitive: bool,
) -> Option<&'a InputVariant> {
    [
        symbol.symbol_id.as_str(),
        symbol.name.as_str(),
        symbol.qualified_name.as_str(),
        symbol.signature.as_str(),
    ]
    .into_iter()
    .find_map(|candidate| plan.matched_variant(candidate, case_sensitive, SymbolMatchMode::Exact))
}

fn sql_in_literals(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("'{}'", value.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn has_scope_i64(has_scope: bool) -> i64 {
    if has_scope {
        1
    } else {
        0
    }
}

fn symbol_select_columns(alias: &str) -> String {
    [
        "symbol_id",
        "name",
        "public_kind",
        "qualified_name",
        "path",
        "range_start_line",
        "range_start_col",
        "range_end_line",
        "range_end_col",
        "selection_start_line",
        "selection_start_col",
        "selection_end_line",
        "selection_end_col",
        "signature",
        "root_id",
    ]
    .into_iter()
    .map(|column| format!("{alias}.{column}"))
    .collect::<Vec<_>>()
    .join(", ")
}

fn call_edge_select_columns() -> String {
    format!(
        "e.edge_id, e.caller_symbol, e.callee_symbol, e.target_name, e.path, \
         e.range_start_line, e.range_start_col, e.range_end_line, e.range_end_col, \
         e.file_hash, e.dispatch_kind, e.status, e.confidence, {}, {}",
        symbol_select_columns("caller"),
        symbol_select_columns("callee")
    )
}

fn map_symbol_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<SymbolRow> {
    let range = optional_range(row, offset + 5)?;
    let selection_range = optional_range(row, offset + 9)?;
    Ok(SymbolRow {
        symbol_id: row.get(offset)?,
        name: row.get(offset + 1)?,
        public_kind: row.get(offset + 2)?,
        qualified_name: row.get(offset + 3)?,
        path: row.get(offset + 4)?,
        range,
        selection_range,
        signature: row.get(offset + 13)?,
        root_id: row.get(offset + 14)?,
    })
}

fn optional_range(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Option<SourceRange>> {
    let start_line = row.get::<_, Option<i64>>(offset)?;
    let start_col = row.get::<_, Option<i64>>(offset + 1)?;
    let end_line = row.get::<_, Option<i64>>(offset + 2)?;
    let end_col = row.get::<_, Option<i64>>(offset + 3)?;
    Ok(match (start_line, start_col, end_line, end_col) {
        (Some(start_line), Some(start_col), Some(end_line), Some(end_col)) => {
            Some(SourceRange::new(
                start_line as u32,
                start_col as u32,
                end_line as u32,
                end_col as u32,
            ))
        }
        _ => None,
    })
}

fn map_call_edge_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<CallEdgeRow> {
    let caller_offset = offset + 13;
    let callee_offset = caller_offset + 15;
    let caller = row
        .get::<_, Option<String>>(caller_offset)?
        .map(|_| map_symbol_row(row, caller_offset))
        .transpose()?;
    let callee = row
        .get::<_, Option<String>>(callee_offset)?
        .map(|_| map_symbol_row(row, callee_offset))
        .transpose()?;
    let variant_kind = row.get::<_, Option<String>>(callee_offset + 15)?;
    let variant_value = row.get::<_, Option<String>>(callee_offset + 16)?;
    Ok(CallEdgeRow {
        edge_id: row.get(offset)?,
        caller_symbol: row.get(offset + 1)?,
        callee_symbol: row.get(offset + 2)?,
        target_name: row.get(offset + 3)?,
        path: row.get(offset + 4)?,
        range: SourceRange::new(
            row.get::<_, i64>(offset + 5)? as u32,
            row.get::<_, i64>(offset + 6)? as u32,
            row.get::<_, i64>(offset + 7)? as u32,
            row.get::<_, i64>(offset + 8)? as u32,
        ),
        file_hash: row.get(offset + 9)?,
        dispatch_kind: row.get(offset + 10)?,
        status: row.get(offset + 11)?,
        confidence: row.get(offset + 12)?,
        caller,
        callee,
        variant: variant_kind
            .zip(variant_value)
            .map(|(kind, value)| InputVariant {
                kind: variant_kind_static(&kind),
                value,
            }),
    })
}

fn variant_kind_static(kind: &str) -> &'static str {
    match kind {
        "raw" => "raw",
        "trimmed" => "trimmed",
        "signature_base" => "signature_base",
        "qualified_tail" => "qualified_tail",
        "signature_tail" => "signature_tail",
        "style_key" => "style_key",
        "case_fold" => "case_fold",
        _ => "raw",
    }
}

fn call_candidate_json(edge: &CallEdgeRow) -> Value {
    json!({
        "path": edge.path,
        "target": edge.callee.as_ref().map(|symbol| symbol.name.clone()).unwrap_or_else(|| edge.target_name.clone()),
        "targetDetail": edge.callee.as_ref().map(|symbol| symbol.qualified_name.clone()),
        "targetSignature": edge.callee.as_ref().map(|symbol| symbol.signature.clone()),
        "targetSymbolId": edge.callee_symbol,
        "kind": "call",
        "enclosingSymbol": edge.caller.as_ref().map(|symbol| symbol.name.clone()),
        "enclosingSymbolDetail": edge.caller.as_ref().map(|symbol| symbol.qualified_name.clone()),
        "enclosingSymbolSignature": edge.caller.as_ref().map(|symbol| symbol.signature.clone()),
        "enclosingSymbolId": edge.caller_symbol,
        "language": "java",
        "rootId": edge.caller.as_ref().map(|symbol| symbol.root_id.clone()).unwrap_or_else(|| "java:.".to_string()),
        "range": edge.range.to_codetrail_json(),
        "fileHash": edge.file_hash,
        "producer": PRODUCER,
        "reliability": "inferred_candidate",
        "layer": "inferred_candidate",
        "exact": false,
        "source": "java_semantic",
        "level": "inferred_candidate",
        "dispatchKind": edge.dispatch_kind.to_lowercase(),
        "resolveStatus": edge.status,
        "confidence": edge.confidence.to_lowercase(),
    })
}

fn symbol_item_json(symbol: &SymbolRow) -> Value {
    json!({
        "symbol_id": symbol.symbol_id,
        "name": symbol.name,
        "signature": symbol.signature,
        "kind": symbol.public_kind,
        "path": symbol.path,
        "range": symbol.range.as_ref().map(|range| range.to_lsp_json()),
        "selectionRange": symbol.selection_range.as_ref().map(|range| range.to_lsp_json()),
        "detail": symbol.qualified_name,
    })
}

fn finalize_results(results: &mut Vec<Value>, limit: usize) {
    results.sort_by(|a, b| {
        let ap = a.get("path").and_then(Value::as_str).unwrap_or_default();
        let bp = b.get("path").and_then(Value::as_str).unwrap_or_default();
        let al = a["range"]["start"]["line"].as_u64().unwrap_or(0);
        let bl = b["range"]["start"]["line"].as_u64().unwrap_or(0);
        ap.cmp(bp).then(al.cmp(&bl))
    });
    results.dedup_by(|a, b| {
        a.get("path") == b.get("path")
            && a["range"]["start"] == b["range"]["start"]
            && a.get("target") == b.get("target")
            && a.get("enclosingSymbol") == b.get("enclosingSymbol")
    });
    if limit > 0 && results.len() > limit {
        results.truncate(limit);
    }
}

fn dedup_edges(edges: &mut Vec<CallEdgeRow>) {
    edges.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.range.start_line.cmp(&b.range.start_line))
            .then(a.range.start_column.cmp(&b.range.start_column))
            .then(a.caller_symbol.cmp(&b.caller_symbol))
            .then(a.callee_symbol.cmp(&b.callee_symbol))
    });
    edges.dedup_by(|a, b| {
        a.path == b.path
            && a.range.start_line == b.range.start_line
            && a.range.start_column == b.range.start_column
            && a.caller_symbol == b.caller_symbol
            && a.callee_symbol == b.callee_symbol
    });
}

fn symbol_kind_code(kind: JavaSymbolKind) -> &'static str {
    match kind {
        JavaSymbolKind::Type => "type",
        JavaSymbolKind::Method => "method",
        JavaSymbolKind::Constructor => "constructor",
        JavaSymbolKind::Field => "field",
        JavaSymbolKind::Local => "local",
        JavaSymbolKind::Parameter => "parameter",
        JavaSymbolKind::Annotation => "annotation",
        JavaSymbolKind::SyntheticMethod => "synthetic_method",
    }
}

fn symbol_origin_code(origin: SymbolOrigin) -> &'static str {
    match origin {
        SymbolOrigin::Source => "source",
        SymbolOrigin::Scip => "scip",
        SymbolOrigin::Classfile => "classfile",
        SymbolOrigin::GeneratedSource => "generated_source",
        SymbolOrigin::LombokSynthetic => "lombok_synthetic",
    }
}

fn confidence_code(confidence: ResolveConfidence) -> &'static str {
    match confidence {
        ResolveConfidence::Scip => "Scip",
        ResolveConfidence::SourceResolver => "SourceResolver",
        ResolveConfidence::GeneratedSource => "GeneratedSource",
        ResolveConfidence::ClassfileSummary => "ClassfileSummary",
        ResolveConfidence::SyntheticAnnotationModel => "SyntheticAnnotationModel",
        ResolveConfidence::SyntaxOnly => "SyntaxOnly",
        ResolveConfidence::Unresolved => "Unresolved",
        ResolveConfidence::Ambiguous => "Ambiguous",
        ResolveConfidence::IncompleteGeneratedSemantics => "IncompleteGeneratedSemantics",
    }
}

fn dispatch_kind_code(kind: DispatchKind) -> &'static str {
    match kind {
        DispatchKind::Static => "Static",
        DispatchKind::Virtual => "Virtual",
        DispatchKind::Interface => "Interface",
        DispatchKind::Constructor => "Constructor",
        DispatchKind::Super => "Super",
        DispatchKind::MethodReference => "MethodReference",
        DispatchKind::Unknown => "Unknown",
    }
}

fn status_code(status: ResolveStatus) -> &'static str {
    match status {
        ResolveStatus::Resolved => "Resolved",
        ResolveStatus::Ambiguous => "Ambiguous",
        ResolveStatus::Unresolved => "Unresolved",
        ResolveStatus::IncompleteGeneratedSemantics => "IncompleteGeneratedSemantics",
    }
}
